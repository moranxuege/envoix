#![cfg(target_os = "linux")]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Harness {
    _temporary: TempDir,
    cli: PathBuf,
    config_home: PathBuf,
    fake_bin: PathBuf,
    home: PathBuf,
    state_home: PathBuf,
    systemctl_log: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let home = root.join("home");
        let config_home = root.join("config");
        let state_home = root.join("state");
        let fake_bin = root.join("fake-bin");
        let systemctl_log = root.join("systemctl.log");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&fake_bin).unwrap();
        let systemctl = fake_bin.join("systemctl");
        fs::write(
            &systemctl,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ENVOIX_TEST_SYSTEMCTL_LOG\"\n",
        )
        .unwrap();
        fs::set_permissions(&systemctl, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _temporary: temporary,
            cli: PathBuf::from(env!("CARGO_BIN_EXE_envoix")),
            config_home,
            fake_bin,
            home,
            state_home,
            systemctl_log,
        }
    }

    fn run(&self, arguments: &[&str]) -> Output {
        let path = env::join_paths(
            std::iter::once(self.fake_bin.clone())
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        Command::new(&self.cli)
            .args(arguments)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_STATE_HOME", &self.state_home)
            .env("PATH", path)
            .env("ENVOIX_TEST_SYSTEMCTL_LOG", &self.systemctl_log)
            .env_remove("ENVOIX_STATE_DIR")
            .output()
            .unwrap()
    }

    fn assert_success(&self, arguments: &[&str]) {
        let output = self.run(arguments);
        assert!(
            output.status.success(),
            "command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn installed_cli(&self) -> PathBuf {
        self.home.join(".local/bin/envoix")
    }

    fn installed_agent(&self) -> PathBuf {
        self.home.join(".local/bin/envoix-agent")
    }

    fn settings(&self) -> PathBuf {
        self.config_home.join("envoix/agent.json")
    }

    fn unit(&self) -> PathBuf {
        self.config_home.join("systemd/user/envoix-agent.service")
    }

    fn state(&self) -> PathBuf {
        self.state_home.join("envoix")
    }
}

fn write(path: impl AsRef<Path>, contents: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn managed_lifecycle_updates_in_place_and_never_removes_inbox() {
    let harness = Harness::new();
    let source_agent = harness.home.join("build/envoix-agent");
    write(&source_agent, "agent-v1");

    harness.assert_success(&[
        "agent",
        "install",
        "--agent-binary",
        source_agent.to_str().unwrap(),
        "--device-name",
        "Test WSL",
    ]);
    assert!(harness.installed_cli().is_file());
    assert_eq!(
        fs::read_to_string(harness.installed_agent()).unwrap(),
        "agent-v1"
    );
    assert!(harness.unit().is_file());
    let settings = fs::read(harness.settings()).unwrap();

    let state = harness.state();
    write(state.join("engine-state-v2.json"), "engine");
    write(state.join("vault/credential"), "secret");
    write(state.join("outbox/jobs/job.json"), "pending");
    write(state.join("agent.sock"), "stale socket fixture");
    write(state.join("inbox/received.txt"), "received bytes");
    write(state.join("user-note.txt"), "not lifecycle-owned");

    write(&source_agent, "agent-v2");
    harness.assert_success(&[
        "agent",
        "update",
        "--agent-binary",
        source_agent.to_str().unwrap(),
    ]);
    assert_eq!(
        fs::read_to_string(harness.installed_agent()).unwrap(),
        "agent-v2"
    );
    assert_eq!(fs::read(harness.settings()).unwrap(), settings);
    assert!(state.join("engine-state-v2.json").is_file());
    assert!(state.join("vault/credential").is_file());
    assert!(state.join("inbox/received.txt").is_file());

    harness.assert_success(&["agent", "restart"]);
    harness.assert_success(&["agent", "uninstall"]);
    assert!(!harness.installed_cli().exists());
    assert!(!harness.installed_agent().exists());
    assert!(!harness.unit().exists());
    assert!(harness.settings().is_file());
    assert!(state.join("engine-state-v2.json").is_file());
    assert!(state.join("vault/credential").is_file());
    assert!(state.join("inbox/received.txt").is_file());

    harness.assert_success(&[
        "agent",
        "install",
        "--agent-binary",
        source_agent.to_str().unwrap(),
        "--device-name",
        "Test WSL",
    ]);
    harness.assert_success(&["agent", "uninstall", "--delete-state", "--yes"]);
    assert!(!harness.settings().exists());
    assert!(!state.join("engine-state-v2.json").exists());
    assert!(!state.join("vault").exists());
    assert!(!state.join("outbox").exists());
    assert!(!state.join("agent.sock").exists());
    assert_eq!(
        fs::read_to_string(state.join("inbox/received.txt")).unwrap(),
        "received bytes"
    );
    assert_eq!(
        fs::read_to_string(state.join("user-note.txt")).unwrap(),
        "not lifecycle-owned"
    );

    let systemctl = fs::read_to_string(&harness.systemctl_log).unwrap();
    assert!(systemctl.contains("restart envoix-agent.service"));
    assert!(systemctl.contains("disable --now envoix-agent.service"));
    assert!(systemctl.contains("daemon-reload"));
}
