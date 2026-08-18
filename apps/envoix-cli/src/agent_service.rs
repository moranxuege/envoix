use std::io;
use std::path::PathBuf;

pub(crate) struct InstallOptions {
    pub(crate) inbox: Option<PathBuf>,
    pub(crate) device_name: String,
    pub(crate) agent_binary: Option<PathBuf>,
}

pub(crate) struct InstalledAgent {
    pub(crate) agent_binary: PathBuf,
    pub(crate) cli_binary: PathBuf,
    pub(crate) settings_file: PathBuf,
    pub(crate) unit_file: PathBuf,
}

#[cfg(target_os = "linux")]
pub(crate) fn install(options: InstallOptions) -> io::Result<InstalledAgent> {
    linux::install(options)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn install(_options: InstallOptions) -> io::Result<InstalledAgent> {
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn start() -> io::Result<()> {
    linux::systemctl(&["start", linux::SERVICE_NAME])
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn start() -> io::Result<()> {
    Err(unsupported())
}

#[cfg(target_os = "linux")]
pub(crate) fn stop() -> io::Result<()> {
    linux::systemctl(&["stop", linux::SERVICE_NAME])
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn stop() -> io::Result<()> {
    Err(unsupported())
}

#[cfg(not(target_os = "linux"))]
fn unsupported() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "managed Agent services are currently supported only on Linux and WSL",
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

    use super::{InstallOptions, InstalledAgent};

    pub(super) const SERVICE_NAME: &str = "envoix-agent.service";

    pub(super) fn install(options: InstallOptions) -> io::Result<InstalledAgent> {
        let home = absolute(home_directory()?)?;
        let config_home = absolute(
            env::var_os("XDG_CONFIG_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config")),
        )?;
        let bin_directory = home.join(".local/bin");
        let settings_file = config_home.join("envoix/agent.json");
        let unit_file = config_home.join("systemd/user").join(SERVICE_NAME);
        let cli_source = fs::canonicalize(env::current_exe()?)?;
        let agent_source = resolve_agent_binary(options.agent_binary, &cli_source)?;
        let cli_binary = bin_directory.join("envoix");
        let agent_binary = bin_directory.join("envoix-agent");
        let state_directory = absolute(default_agent_state_directory()?)?;
        let inbox_directory = absolute(
            options
                .inbox
                .unwrap_or_else(|| state_directory.join("inbox")),
        )?;
        let settings = AgentSettings {
            version: AGENT_SETTINGS_VERSION,
            device_name: options.device_name,
            inbox_directory,
        };
        settings.validate()?;

        create_directory(&bin_directory, 0o755)?;
        install_executable(&cli_source, &cli_binary)?;
        install_executable(&agent_source, &agent_binary)?;
        let settings_bytes = serde_json::to_vec_pretty(&settings)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        write_file(&settings_file, &settings_bytes, 0o600)?;
        let unit = render_unit(&agent_binary, &settings_file)?;
        write_file(&unit_file, unit.as_bytes(), 0o644)?;

        let activation = systemctl(&["daemon-reload"])
            .and_then(|()| systemctl(&["enable", SERVICE_NAME]))
            .and_then(|()| systemctl(&["restart", SERVICE_NAME]));
        activation.map_err(|error| {
            io::Error::other(format!(
                "Agent files were installed, but the user service could not start: {error}; \
                 enable systemd for this WSL distribution or run {} --settings {} in a foreground shell",
                agent_binary.display(),
                settings_file.display()
            ))
        })?;

        Ok(InstalledAgent {
            agent_binary,
            cli_binary,
            settings_file,
            unit_file,
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
             Type=simple\n\
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
    }
}
