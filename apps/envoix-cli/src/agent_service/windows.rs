use std::io;
use std::path::Path;

#[cfg(windows)]
use std::env;
#[cfg(windows)]
use std::ffi::{OsStr, OsString};
#[cfg(windows)]
use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::io::Write as _;
#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::thread;
#[cfg(windows)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use envoix_client::product::{
    AGENT_SETTINGS_VERSION, AgentSettings, current_windows_user_sid, default_agent_state_directory,
};

#[cfg(windows)]
use super::{
    ConfigureOptions, InstallOptions, InstalledAgent, UninstallOptions, UninstalledAgent,
    UpdateOptions, clear_managed_state, remove_file_if_exists, require_file,
};

#[cfg(windows)]
const TASK_DEFINITION_FILE: &str = "agent-task-v1.xml";

fn task_name(user_sid: &str) -> String {
    format!("Envoix Agent {user_sid}")
}

fn render_task_xml(
    user_sid: &str,
    agent_binary: &Path,
    settings_file: &Path,
    state_directory: &Path,
) -> io::Result<String> {
    let command = xml_escape(path_text(agent_binary, "Agent binary")?);
    let settings = path_text(settings_file, "Agent settings")?;
    let state_directory = path_text(state_directory, "Agent state directory")?;
    let working_directory = xml_escape(path_text(
        agent_binary.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Agent binary has no parent")
        })?,
        "Agent binary directory",
    )?);
    let user_sid = xml_escape(user_sid);
    let arguments = xml_escape(&format!(
        "--settings \"{settings}\" --state-dir \"{state_directory}\""
    ));
    Ok(format!(
        r#"<?xml version="1.0"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Envoix per-user transfer Agent</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user_sid}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user_sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{working_directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    ))
}

fn path_text<'a>(path: &'a Path, label: &str) -> io::Result<&'a str> {
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{label} path must be valid Unicode for Task Scheduler"),
        )
    })
}

fn xml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            character => escaped.push(character),
        }
    }
    escaped
}

#[cfg(windows)]
struct ServiceLayout {
    agent_binary: PathBuf,
    cli_binary: PathBuf,
    settings_file: PathBuf,
    state_directory: PathBuf,
    task_definition: PathBuf,
    task_name: String,
    user_sid: String,
}

#[cfg(windows)]
impl ServiceLayout {
    fn discover() -> io::Result<Self> {
        let local_app_data = env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "cannot locate the Windows per-user installation directory; LOCALAPPDATA is not set",
                )
            })?;
        let root = absolute(local_app_data.join("Envoix"))?;
        let bin_directory = root.join("bin");
        let config_directory = root.join("config");
        let user_sid = current_windows_user_sid()?;
        let task_name = task_name(&user_sid);
        Ok(Self {
            agent_binary: bin_directory.join("envoix-agent.exe"),
            cli_binary: bin_directory.join("envoix.exe"),
            settings_file: config_directory.join("agent.json"),
            state_directory: absolute(default_agent_state_directory()?)?,
            task_definition: config_directory.join(TASK_DEFINITION_FILE),
            task_name,
            user_sid,
        })
    }

    fn installed(&self) -> InstalledAgent {
        InstalledAgent {
            agent_binary: self.agent_binary.clone(),
            cli_binary: self.cli_binary.clone(),
            service_definition: self.task_definition.clone(),
            settings_file: self.settings_file.clone(),
        }
    }
}

#[cfg(windows)]
pub(super) fn install(options: InstallOptions) -> io::Result<InstalledAgent> {
    let layout = ServiceLayout::discover()?;
    let cli_source = fs::canonicalize(env::current_exe()?)?;
    let agent_source = resolve_agent_binary(options.agent_binary, &cli_source)?;
    let inbox_directory = absolute(
        options
            .inbox
            .unwrap_or_else(|| layout.state_directory.join("inbox")),
    )?;
    let settings = AgentSettings {
        version: AGENT_SETTINGS_VERSION,
        device_name: options.device_name,
        inbox_directory,
        broker: options.broker,
        relay: options.relay,
    };
    settings.validate()?;

    let end_error = end_task(&layout).err();
    wait_for_executable_release(&layout.agent_binary)
        .map_err(|error| lifecycle_error("install", end_error, error))?;
    install_executable(&agent_source, &layout.agent_binary)?;
    install_executable(&cli_source, &layout.cli_binary)?;
    let settings_bytes = serde_json::to_vec_pretty(&settings)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_file(&layout.settings_file, &settings_bytes)?;
    let task = render_task_xml(
        &layout.user_sid,
        &layout.agent_binary,
        &layout.settings_file,
        &layout.state_directory,
    )?;
    write_file(&layout.task_definition, task.as_bytes())?;
    register_task(&layout)?;
    run_task(&layout)?;
    Ok(layout.installed())
}

#[cfg(windows)]
pub(super) fn configure(options: ConfigureOptions) -> io::Result<InstalledAgent> {
    let layout = ServiceLayout::discover()?;
    require_file(&layout.settings_file, "Agent settings")?;
    require_file(&layout.task_definition, "Agent scheduled task")?;
    let bytes = fs::read(&layout.settings_file)?;
    let mut settings: AgentSettings = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    settings.version = AGENT_SETTINGS_VERSION;
    settings.broker = options.broker;
    settings.relay = options.relay;
    settings.validate()?;
    let settings_bytes = serde_json::to_vec_pretty(&settings)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_file(&layout.settings_file, &settings_bytes)?;
    end_task(&layout).ok();
    run_task(&layout)?;
    Ok(layout.installed())
}

#[cfg(windows)]
pub(super) fn start() -> io::Result<()> {
    let layout = ServiceLayout::discover()?;
    query_task(&layout)?;
    run_task(&layout)
}

#[cfg(windows)]
pub(super) fn stop() -> io::Result<()> {
    let layout = ServiceLayout::discover()?;
    query_task(&layout)?;
    end_task(&layout)?;
    wait_for_executable_release(&layout.agent_binary)
}

#[cfg(windows)]
pub(super) fn restart() -> io::Result<()> {
    let layout = ServiceLayout::discover()?;
    query_task(&layout)?;
    let end_error = end_task(&layout).err();
    wait_for_executable_release(&layout.agent_binary)
        .map_err(|error| lifecycle_error("restart", end_error, error))?;
    run_task(&layout)
}

#[cfg(windows)]
pub(super) fn update(options: UpdateOptions) -> io::Result<InstalledAgent> {
    let layout = ServiceLayout::discover()?;
    require_file(&layout.settings_file, "Agent settings")?;
    require_file(&layout.task_definition, "Agent Task Scheduler definition")?;
    query_task(&layout)?;
    let cli_source = fs::canonicalize(env::current_exe()?)?;
    let agent_source = resolve_agent_binary(options.agent_binary, &cli_source)?;
    let end_error = end_task(&layout).err();
    wait_for_executable_release(&layout.agent_binary)
        .map_err(|error| lifecycle_error("update", end_error, error))?;

    install_executable(&agent_source, &layout.agent_binary)?;
    install_executable(&cli_source, &layout.cli_binary)?;
    run_task(&layout)?;
    Ok(layout.installed())
}

#[cfg(windows)]
pub(super) fn uninstall(options: UninstallOptions) -> io::Result<UninstalledAgent> {
    let layout = ServiceLayout::discover()?;
    query_task(&layout)?;
    let end_error = end_task(&layout).err();
    wait_for_executable_release(&layout.agent_binary)
        .map_err(|error| lifecycle_error("uninstall", end_error, error))?;
    delete_task(&layout)?;
    remove_file_if_exists(&layout.agent_binary)?;
    remove_file_if_exists(&layout.task_definition)?;

    if options.delete_state {
        clear_managed_state(&layout.state_directory)?;
        remove_file_if_exists(&layout.settings_file)?;
    }
    remove_or_schedule_cli_removal(&layout.cli_binary)?;

    Ok(UninstalledAgent {
        state_directory: layout.state_directory,
        state_cleared: options.delete_state,
    })
}

#[cfg(windows)]
fn lifecycle_error(operation: &str, stop_error: Option<io::Error>, error: io::Error) -> io::Error {
    let Some(stop_error) = stop_error else {
        return error;
    };
    io::Error::new(
        error.kind(),
        format!(
            "Agent task could not stop before {operation}: {stop_error}; {operation} failed: {error}"
        ),
    )
}

#[cfg(windows)]
fn absolute(path: PathBuf) -> io::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}

#[cfg(windows)]
fn wait_for_executable_release(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match OpenOptions::new().write(true).open(path) {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32)
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "Agent executable is still in use after stopping the task: {}",
                        path.display()
                    ),
                ));
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn remove_or_schedule_cli_removal(path: &Path) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let current = fs::canonicalize(env::current_exe()?)?;
    let installed = fs::canonicalize(path)?;
    if current != installed {
        return remove_file_if_exists(path);
    }
    schedule_self_delete(path)
}

#[cfg(windows)]
fn schedule_self_delete(path: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    const SCRIPT: &str = "@echo off\r\n\
setlocal\r\n\
for /L %%I in (1,1,50) do (\r\n\
  del /F /Q \"%~1\" >NUL 2>&1\r\n\
  if not exist \"%~1\" goto deleted\r\n\
  >NUL 2>&1 \"%SystemRoot%\\System32\\PING.EXE\" 127.0.0.1 -n 2\r\n\
)\r\n\
:deleted\r\n\
del /F /Q \"%~f0\" >NUL 2>&1\r\n";

    let command = system_executable("cmd.exe")?;
    let script = create_cleanup_script(SCRIPT.as_bytes())?;
    let result = Command::new(command)
        .args(["/D", "/Q", "/C", "call"])
        .arg(&script)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match result {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&script);
            Err(error)
        }
    }
}

#[cfg(windows)]
fn create_cleanup_script(contents: &[u8]) -> io::Result<PathBuf> {
    for suffix in 0..10 {
        let path = env::temp_dir().join(format!(
            "envoix-uninstall-{}-{suffix}.cmd",
            std::process::id()
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let result = file.write_all(contents).and_then(|()| file.sync_all());
                drop(file);
                if let Err(error) = result {
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot allocate a temporary Envoix uninstall script",
    ))
}

#[cfg(windows)]
fn resolve_agent_binary(explicit: Option<PathBuf>, cli: &Path) -> io::Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path);
    } else {
        if let Some(parent) = cli.parent() {
            candidates.push(parent.join("envoix-agent.exe"));
        }
        if let Some(path) = env::var_os("PATH") {
            candidates.extend(env::split_paths(&path).map(|path| path.join("envoix-agent.exe")));
        }
    }
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .map(fs::canonicalize)
        .transpose()?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot find a prebuilt envoix-agent.exe; place it beside envoix.exe or pass --agent-binary",
            )
        })
}

#[cfg(windows)]
fn install_executable(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() && fs::canonicalize(destination)? == source {
        return Ok(());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "binary has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(destination)?;
    let result = (|| {
        fs::copy(source, &temporary)?;
        OpenOptions::new()
            .write(true)
            .open(&temporary)?
            .sync_all()?;
        replace_file(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(path)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        replace_file(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(windows)]
fn temporary_path(path: &Path) -> io::Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no name"))?;
    let mut temporary_name = OsString::from(".");
    temporary_name.push(name);
    temporary_name.push(format!(".{}.tmp", std::process::id()));
    Ok(path.with_file_name(temporary_name))
}

#[cfg(windows)]
fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(iter::once(0))
        .collect::<Vec<_>>();
    // SAFETY: Both buffers are live, null-terminated UTF-16 paths. MoveFileExW
    // is the Windows primitive that can atomically replace an existing file;
    // std::fs::rename cannot provide those replacement semantics on Windows.
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(windows)]
fn register_task(layout: &ServiceLayout) -> io::Result<()> {
    schtasks(&[
        OsStr::new("/Create"),
        OsStr::new("/TN"),
        OsStr::new(&layout.task_name),
        OsStr::new("/XML"),
        layout.task_definition.as_os_str(),
        OsStr::new("/F"),
    ])
}

#[cfg(windows)]
fn query_task(layout: &ServiceLayout) -> io::Result<()> {
    schtasks(&[
        OsStr::new("/Query"),
        OsStr::new("/TN"),
        OsStr::new(&layout.task_name),
    ])
}

#[cfg(windows)]
fn run_task(layout: &ServiceLayout) -> io::Result<()> {
    schtasks(&[
        OsStr::new("/Run"),
        OsStr::new("/TN"),
        OsStr::new(&layout.task_name),
    ])
}

#[cfg(windows)]
fn end_task(layout: &ServiceLayout) -> io::Result<()> {
    schtasks(&[
        OsStr::new("/End"),
        OsStr::new("/TN"),
        OsStr::new(&layout.task_name),
    ])
}

#[cfg(windows)]
fn delete_task(layout: &ServiceLayout) -> io::Result<()> {
    schtasks(&[
        OsStr::new("/Delete"),
        OsStr::new("/TN"),
        OsStr::new(&layout.task_name),
        OsStr::new("/F"),
    ])
}

#[cfg(windows)]
fn schtasks(arguments: &[&OsStr]) -> io::Result<()> {
    let output = Command::new(system_executable("schtasks.exe")?)
        .args(arguments)
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    let operation = arguments
        .first()
        .map(|argument| argument.to_string_lossy())
        .unwrap_or_default();
    Err(io::Error::other(if detail.is_empty() {
        format!("schtasks.exe {operation} failed")
    } else {
        format!("schtasks.exe {operation} failed: {detail}")
    }))
}

#[cfg(windows)]
fn system_executable(name: &str) -> io::Result<PathBuf> {
    let system_root = env::var_os("SystemRoot")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "SystemRoot is not set"))?;
    let executable = system_root.join("System32").join(name);
    if executable.is_file() {
        Ok(executable)
    } else {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "cannot find Windows system executable {}",
                executable.display()
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_definition_is_user_scoped_least_privilege_and_path_safe() {
        let xml = render_task_xml(
            "S-1-5-21-1000",
            Path::new(r"C:\Users\Test & Dev\AppData\Local\Envoix\bin\envoix-agent.exe"),
            Path::new(r"C:\Users\Test & Dev\AppData\Local\Envoix\config\agent.json"),
            Path::new(r"C:\Users\Test & Dev\AppData\Local\Envoix"),
        )
        .unwrap();

        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.matches("<UserId>S-1-5-21-1000</UserId>").count() == 2);
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(xml.contains("<RestartOnFailure>"));
        assert!(xml.contains("<Interval>PT1M</Interval>"));
        assert!(xml.contains(r"C:\Users\Test &amp; Dev\AppData\Local\Envoix\bin\envoix-agent.exe"));
        assert!(xml.contains(
            r"--settings &quot;C:\Users\Test &amp; Dev\AppData\Local\Envoix\config\agent.json&quot; --state-dir &quot;C:\Users\Test &amp; Dev\AppData\Local\Envoix&quot;"
        ));
        assert!(!xml.contains("&amp;amp;"));
    }

    #[test]
    fn task_names_are_bounded_and_distinct_per_user() {
        let first = task_name("S-1-5-21-1000");
        let second = task_name("S-1-5-21-1001");
        assert_ne!(first, second);
        assert!(first.len() <= 238);
        assert!(!first.contains(['/', '\\']));
    }
}
