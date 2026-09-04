#[cfg(any(target_os = "linux", windows))]
use std::fs;
use std::io;
#[cfg(any(target_os = "linux", windows))]
use std::path::Path;
use std::path::PathBuf;

#[cfg(any(windows, test))]
mod windows;

#[cfg(any(target_os = "linux", windows))]
const MANAGED_STATE_ENTRIES: &[&str] = &[
    "agent.sock",
    "identity.key",
    "engine-state-v2.json",
    "engine-state-v2.previous.json",
    "engine-state-v1.json",
    "engine-state-v1.previous.json",
    "engine.lock",
    "migration",
    "vault",
    "product",
    "outbox",
    "transfer-state-v2",
];

#[cfg(any(target_os = "linux", windows))]
fn require_file(path: &Path, label: &str) -> io::Result<()> {
    if path.is_file() {
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!(
            "{label} is not installed at {}; run `envoix agent install` first",
            path.display()
        ),
    ))
}

#[cfg(any(target_os = "linux", windows))]
fn remove_file_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", windows))]
fn clear_managed_state(directory: &Path) -> io::Result<()> {
    for entry in MANAGED_STATE_ENTRIES {
        remove_managed_path(&directory.join(entry))?;
    }
    match fs::remove_dir(directory) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", windows))]
fn remove_managed_path(path: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

pub(crate) struct InstallOptions {
    pub(crate) inbox: Option<PathBuf>,
    pub(crate) device_name: String,
    pub(crate) agent_binary: Option<PathBuf>,
    pub(crate) broker: String,
    pub(crate) relay: Option<String>,
}

pub(crate) struct ConfigureOptions {
    pub(crate) broker: String,
    pub(crate) relay: Option<String>,
}

pub(crate) struct UpdateOptions {
    pub(crate) agent_binary: Option<PathBuf>,
}

pub(crate) struct UninstallOptions {
    pub(crate) delete_state: bool,
}

pub(crate) struct InstalledAgent {
    pub(crate) agent_binary: PathBuf,
    pub(crate) cli_binary: PathBuf,
    pub(crate) service_definition: PathBuf,
    pub(crate) settings_file: PathBuf,
}

pub(crate) struct UninstalledAgent {
    pub(crate) state_directory: PathBuf,
    pub(crate) state_cleared: bool,
}

#[cfg(target_os = "linux")]
pub(crate) fn install(options: InstallOptions) -> io::Result<InstalledAgent> {
    linux::install(options)
}

#[cfg(target_os = "linux")]
pub(crate) fn configure(options: ConfigureOptions) -> io::Result<InstalledAgent> {
    linux::configure(options)
}

#[cfg(windows)]
pub(crate) fn configure(options: ConfigureOptions) -> io::Result<InstalledAgent> {
    windows::configure(options)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn configure(options: ConfigureOptions) -> io::Result<InstalledAgent> {
    let ConfigureOptions { broker, relay } = options;
    let _ = (broker, relay);
    Err(unsupported())
}

#[cfg(windows)]
pub(crate) fn install(options: InstallOptions) -> io::Result<InstalledAgent> {
    windows::install(options)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn install(options: InstallOptions) -> io::Result<InstalledAgent> {
    let InstallOptions {
        inbox,
        device_name,
        agent_binary,
        broker,
        relay,
    } = options;
    let _ = (inbox, device_name, agent_binary, broker, relay);
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn start() -> io::Result<()> {
    linux::systemctl(&["start", linux::SERVICE_NAME])
}

#[cfg(windows)]
pub(crate) fn start() -> io::Result<()> {
    windows::start()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn start() -> io::Result<()> {
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn stop() -> io::Result<()> {
    linux::systemctl(&["stop", linux::SERVICE_NAME])
}

#[cfg(windows)]
pub(crate) fn stop() -> io::Result<()> {
    windows::stop()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn stop() -> io::Result<()> {
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn restart() -> io::Result<()> {
    linux::systemctl(&["restart", linux::SERVICE_NAME])
}

#[cfg(windows)]
pub(crate) fn restart() -> io::Result<()> {
    windows::restart()
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn restart() -> io::Result<()> {
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn update(options: UpdateOptions) -> io::Result<InstalledAgent> {
    linux::update(options)
}

#[cfg(windows)]
pub(crate) fn update(options: UpdateOptions) -> io::Result<InstalledAgent> {
    windows::update(options)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn update(options: UpdateOptions) -> io::Result<InstalledAgent> {
    let UpdateOptions { agent_binary } = options;
    let _ = agent_binary;
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn uninstall(options: UninstallOptions) -> io::Result<UninstalledAgent> {
    linux::uninstall(options)
}

#[cfg(windows)]
pub(crate) fn uninstall(options: UninstallOptions) -> io::Result<UninstalledAgent> {
    windows::uninstall(options)
}

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) fn uninstall(options: UninstallOptions) -> io::Result<UninstalledAgent> {
    let UninstallOptions { delete_state } = options;
    let _ = delete_state;
    Err(unsupported())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "managed Agent services are supported only on Linux, WSL, and Windows",
    )
}

#[cfg(target_os = "linux")]
mod linux {
    use std::env;
    use std::ffi::OsString;
    use std::fs::{self, OpenOptions};
    use std::io::{self, Write};
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use envoix_client::product::{
        AGENT_SETTINGS_VERSION, AgentSettings, default_agent_state_directory,
    };

    use super::{
        ConfigureOptions, InstallOptions, InstalledAgent, UninstallOptions, UninstalledAgent,
        UpdateOptions, clear_managed_state, remove_file_if_exists, require_file,
    };

    pub(super) const SERVICE_NAME: &str = "envoix-agent.service";

    struct ServiceLayout {
        agent_binary: PathBuf,
        cli_binary: PathBuf,
        settings_file: PathBuf,
        state_directory: PathBuf,
        unit_file: PathBuf,
    }

    impl ServiceLayout {
        fn discover() -> io::Result<Self> {
            let home = absolute(home_directory()?)?;
            let config_home = absolute(
                env::var_os("XDG_CONFIG_HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".config")),
            )?;
            let bin_directory = home.join(".local/bin");
            Ok(Self {
                agent_binary: bin_directory.join("envoix-agent"),
                cli_binary: bin_directory.join("envoix"),
                settings_file: config_home.join("envoix/agent.json"),
                state_directory: absolute(default_agent_state_directory()?)?,
                unit_file: config_home.join("systemd/user").join(SERVICE_NAME),
            })
        }

        fn installed(&self) -> InstalledAgent {
            InstalledAgent {
                agent_binary: self.agent_binary.clone(),
                cli_binary: self.cli_binary.clone(),
                service_definition: self.unit_file.clone(),
                settings_file: self.settings_file.clone(),
            }
        }
    }

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

        let bin_directory = layout
            .cli_binary
            .parent()
            .expect("installed CLI path has a parent");
        create_directory(bin_directory, 0o755)?;
        install_executable(&cli_source, &layout.cli_binary)?;
        install_executable(&agent_source, &layout.agent_binary)?;
        let settings_bytes = serde_json::to_vec_pretty(&settings)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_file(&layout.settings_file, &settings_bytes, 0o600)?;
        write_current_unit(&layout)?;

        let activation = systemctl(&["daemon-reload"])
            .and_then(|()| systemctl(&["enable", SERVICE_NAME]))
            .and_then(|()| systemctl(&["restart", SERVICE_NAME]));
        activation.map_err(|error| {
            io::Error::other(format!(
                "Agent files were installed, but the user service could not start: {error}; \
                 enable systemd for this WSL distribution or run {} --settings {} in a foreground shell",
                layout.agent_binary.display(),
                layout.settings_file.display()
            ))
        })?;

        Ok(layout.installed())
    }

    pub(super) fn configure(options: ConfigureOptions) -> io::Result<InstalledAgent> {
        let layout = ServiceLayout::discover()?;
        require_file(&layout.settings_file, "Agent settings")?;
        require_file(&layout.unit_file, "Agent systemd unit")?;
        let bytes = fs::read(&layout.settings_file)?;
        let mut settings: AgentSettings = serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        settings.version = AGENT_SETTINGS_VERSION;
        settings.broker = options.broker;
        settings.relay = options.relay;
        settings.validate()?;
        let settings_bytes = serde_json::to_vec_pretty(&settings)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_file(&layout.settings_file, &settings_bytes, 0o600)?;
        write_current_unit(&layout)?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["restart", SERVICE_NAME])?;
        Ok(layout.installed())
    }

    pub(super) fn update(options: UpdateOptions) -> io::Result<InstalledAgent> {
        let layout = ServiceLayout::discover()?;
        require_file(&layout.settings_file, "Agent settings")?;
        require_file(&layout.unit_file, "Agent systemd unit")?;
        let cli_source = fs::canonicalize(env::current_exe()?)?;
        let agent_source = resolve_agent_binary(options.agent_binary, &cli_source)?;

        install_executable(&agent_source, &layout.agent_binary)?;
        install_executable(&cli_source, &layout.cli_binary)?;
        write_current_unit(&layout)?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["restart", SERVICE_NAME])?;
        Ok(layout.installed())
    }

    pub(super) fn uninstall(options: UninstallOptions) -> io::Result<UninstalledAgent> {
        let layout = ServiceLayout::discover()?;
        require_file(&layout.unit_file, "Agent systemd unit")?;
        systemctl(&["disable", "--now", SERVICE_NAME])?;
        remove_file_if_exists(&layout.unit_file)?;
        systemctl(&["daemon-reload"])?;
        remove_file_if_exists(&layout.agent_binary)?;
        remove_file_if_exists(&layout.cli_binary)?;

        if options.delete_state {
            clear_managed_state(&layout.state_directory)?;
            remove_file_if_exists(&layout.settings_file)?;
        }

        Ok(UninstalledAgent {
            state_directory: layout.state_directory,
            state_cleared: options.delete_state,
        })
    }

    fn home_directory() -> io::Result<PathBuf> {
        env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))
    }

    fn absolute(path: PathBuf) -> io::Result<PathBuf> {
        if path.is_absolute() {
            Ok(path)
        } else {
            Ok(env::current_dir()?.join(path))
        }
    }

    fn resolve_agent_binary(explicit: Option<PathBuf>, cli: &Path) -> io::Result<PathBuf> {
        let mut candidates = Vec::new();
        if let Some(path) = explicit {
            candidates.push(path);
        } else {
            if let Some(parent) = cli.parent() {
                candidates.push(parent.join("envoix-agent"));
            }
            if let Some(path) = env::var_os("PATH") {
                candidates.extend(env::split_paths(&path).map(|path| path.join("envoix-agent")));
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
                    "cannot find a prebuilt envoix-agent; build it beside envoix or pass --agent-binary",
                )
            })
    }

    fn create_directory(path: &Path, mode: u32) -> io::Result<()> {
        let existed = path.exists();
        fs::create_dir_all(path)?;
        if !existed {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }

    fn install_executable(source: &Path, destination: &Path) -> io::Result<()> {
        if destination.exists() && fs::canonicalize(destination)? == source {
            return Ok(());
        }
        let parent = destination
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "binary has no parent"))?;
        create_directory(parent, 0o755)?;
        let temporary = temporary_path(destination)?;
        let result = (|| {
            fs::copy(source, &temporary)?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))?;
            fs::rename(&temporary, destination)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn write_file(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no parent"))?;
        create_directory(parent, 0o700)?;
        let temporary = temporary_path(path)?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn temporary_path(path: &Path) -> io::Result<PathBuf> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "file has no name"))?;
        let mut temporary_name = OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(".{}.tmp", std::process::id()));
        Ok(path.with_file_name(temporary_name))
    }

    fn render_unit(agent_binary: &Path, settings_file: &Path) -> io::Result<String> {
        Ok(format!(
            "[Unit]\n\
             Description=Envoix persistent receiver\n\
             \n\
             [Service]\n\
             Type=notify\n\
             NotifyAccess=main\n\
             ExecStart={} --settings {}\n\
             Restart=on-failure\n\
             RestartSec=3s\n\
             UMask=0077\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            systemd_argument(agent_binary)?,
            systemd_argument(settings_file)?
        ))
    }

    fn write_current_unit(layout: &ServiceLayout) -> io::Result<()> {
        let unit = render_unit(&layout.agent_binary, &layout.settings_file)?;
        write_file(&layout.unit_file, unit.as_bytes(), 0o644)
    }

    fn systemd_argument(path: &Path) -> io::Result<String> {
        let value = path.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "systemd service paths must be valid UTF-8",
            )
        })?;
        if value.chars().any(char::is_control) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "systemd service paths cannot contain control characters",
            ));
        }
        let mut escaped = String::with_capacity(value.len());
        for character in value.chars() {
            match character {
                '\\' => escaped.push_str("\\\\"),
                '"' => escaped.push_str("\\\""),
                '%' => escaped.push_str("%%"),
                '$' => escaped.push_str("$$"),
                character => escaped.push(character),
            }
        }
        Ok(format!("\"{escaped}\""))
    }

    pub(super) fn systemctl(arguments: &[&str]) -> io::Result<()> {
        let output = Command::new("systemctl")
            .arg("--user")
            .args(arguments)
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        Err(io::Error::other(if detail.is_empty() {
            format!("systemctl --user {} failed", arguments.join(" "))
        } else {
            format!("systemctl --user {} failed: {detail}", arguments.join(" "))
        }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::symlink;

        #[test]
        fn unit_quotes_spaces_and_systemd_expansion_characters() {
            let unit = render_unit(
                Path::new("/home/Test User/$bin/envoix-agent"),
                Path::new("/home/Test User/100%/agent.json"),
            )
            .unwrap();

            assert!(unit.contains(
                "ExecStart=\"/home/Test User/$$bin/envoix-agent\" --settings \
                 \"/home/Test User/100%%/agent.json\""
            ));
            assert!(unit.contains("Type=notify"));
            assert!(unit.contains("NotifyAccess=main"));
            assert!(unit.contains("Restart=on-failure"));
            assert!(unit.contains("WantedBy=default.target"));
        }

        #[test]
        fn unit_rejects_control_characters_in_paths() {
            assert!(
                render_unit(
                    Path::new("/tmp/envoix-agent\ninvalid"),
                    Path::new("/tmp/settings")
                )
                .is_err()
            );
        }

        #[test]
        fn state_cleanup_is_allowlisted_and_does_not_follow_symlinks() {
            let temporary = tempfile::tempdir().unwrap();
            let state = temporary.path().join("state");
            let external = temporary.path().join("external");
            fs::create_dir_all(state.join("inbox")).unwrap();
            fs::create_dir_all(&external).unwrap();
            fs::write(state.join("engine-state-v2.json"), "engine").unwrap();
            fs::write(state.join("engine-state-v1.json"), "legacy engine").unwrap();
            fs::create_dir_all(state.join("product")).unwrap();
            fs::write(
                state.join("product/product-state-v1.json"),
                "legacy product",
            )
            .unwrap();
            fs::create_dir_all(state.join("migration")).unwrap();
            fs::write(state.join("migration/backup.json"), "legacy backup").unwrap();
            fs::write(state.join("inbox/received.txt"), "received").unwrap();
            fs::write(state.join("unknown.txt"), "unknown").unwrap();
            fs::write(external.join("credential"), "external").unwrap();
            symlink(&external, state.join("vault")).unwrap();

            clear_managed_state(&state).unwrap();

            assert!(!state.join("engine-state-v2.json").exists());
            assert!(!state.join("engine-state-v1.json").exists());
            assert!(!state.join("product").exists());
            assert!(!state.join("migration").exists());
            assert!(!state.join("vault").exists());
            assert_eq!(
                fs::read_to_string(state.join("inbox/received.txt")).unwrap(),
                "received"
            );
            assert_eq!(
                fs::read_to_string(state.join("unknown.txt")).unwrap(),
                "unknown"
            );
            assert_eq!(
                fs::read_to_string(external.join("credential")).unwrap(),
                "external"
            );
        }
    }
}
