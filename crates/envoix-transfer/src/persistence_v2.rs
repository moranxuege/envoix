//! Durable replacement of private Manifest v2 state files.

#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

pub(crate) async fn replace_file(source: PathBuf, destination: PathBuf) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        tokio::task::spawn_blocking(move || replace_file_windows(&source, &destination))
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))??;
    }
    #[cfg(not(windows))]
    {
        tokio::fs::rename(&source, &destination).await?;
        sync_parent_directory(destination).await?;
    }
    Ok(())
}

#[cfg(not(windows))]
async fn sync_parent_directory(path: PathBuf) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || {
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("state path has no parent directory"))?;
        std::fs::File::open(parent)?.sync_all()
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))?
}

#[cfg(windows)]
fn replace_file_windows(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: both path buffers are live, NUL-terminated UTF-16 strings. The
    // flags request an atomic same-directory replacement and durable metadata
    // flush; no Rust-owned memory is exposed for mutation.
    let moved = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
