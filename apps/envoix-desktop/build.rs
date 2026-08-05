//! Windows cross-link shim.
//!
//! Linking `x86_64-pc-windows-gnu` from Linux, the link line carries both
//! `-lkernel32` and `-lKernel32`. Zig resolves the lowercase name from its
//! bundled mingw definitions, but on a case-sensitive filesystem the
//! capitalised one is looked up as a file and is not found, so the link fails
//! with "unable to find dynamic system library 'Kernel32'".
//!
//! Every symbol actually referenced is exported by the lowercase library, which
//! is already on the same link line, so an empty archive under the capitalised
//! name is enough to satisfy the lookup without contributing any symbols.
//!
//! This affects cross-compilation only; a native Windows build never takes this
//! path because its filesystem is case-insensitive.

use std::path::PathBuf;

/// The whole content of an empty `ar` archive: the global header, nothing else.
const EMPTY_ARCHIVE: &[u8] = b"!<arch>\n";

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.ends_with("pc-windows-gnu") || cfg!(windows) {
        return;
    }

    let shims = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("link-shims");
    std::fs::create_dir_all(&shims).expect("create link shim directory");
    std::fs::write(shims.join("libKernel32.a"), EMPTY_ARCHIVE).expect("write Kernel32 shim");

    println!("cargo:rustc-link-search=native={}", shims.display());
}
