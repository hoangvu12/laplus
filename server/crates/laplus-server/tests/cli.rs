use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct IsolatedInvocation {
    directory: tempfile::TempDir,
}

impl IsolatedInvocation {
    fn new() -> Self {
        Self {
            directory: tempfile::tempdir().expect("an isolated CLI environment"),
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn unit(&self) -> PathBuf {
        self.root()
            .join("home/.config/systemd/user/laplus.service")
    }

    fn systemctl_log(&self) -> PathBuf {
        self.root().join("systemctl.log")
    }

    #[cfg(unix)]
    fn install_fake_systemctl(&self) {
        use std::os::unix::fs::PermissionsExt;

        let bin = self.root().join("bin");
        fs::create_dir_all(&bin).expect("the fake command directory is created");
        let systemctl = bin.join("systemctl");
        fs::write(
            &systemctl,
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$LAPLUS_TEST_SYSTEMCTL_LOG\"\n",
        )
        .expect("the fake systemctl is written");
        let mut permissions = fs::metadata(&systemctl)
            .expect("the fake systemctl has metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(systemctl, permissions).expect("the fake systemctl is executable");
    }

    fn run(&self, arguments: &[&str]) -> Output {
        #[cfg(unix)]
        self.install_fake_systemctl();

        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut path_entries = vec![self.root().join("bin")];
        path_entries.extend(std::env::split_paths(&path));
        let joined_path = std::env::join_paths(path_entries).expect("the isolated PATH is valid");

        Command::new(env!("CARGO_BIN_EXE_laplus-server"))
            .args(arguments)
            .env("HOME", self.root().join("home"))
            .env("USERPROFILE", self.root().join("home"))
            .env("XDG_CONFIG_HOME", self.root().join("config"))
            .env("XDG_DATA_HOME", self.root().join("data"))
            .env("LOCALAPPDATA", self.root().join("local-data"))
            .env("APPDATA", self.root().join("app-data"))
            .env("PATH", joined_path)
            .env("LAPLUS_TEST_SYSTEMCTL_LOG", self.systemctl_log())
            .output()
            .expect("the laplus-server binary runs")
    }
}

fn laplus(arguments: &[&str]) -> Output {
    IsolatedInvocation::new().run(arguments)
}

#[cfg(target_os = "linux")]
#[test]
fn service_commands_are_confined_to_an_isolated_process_boundary() {
    let invocation = IsolatedInvocation::new();
    let unit = invocation.unit();
    fs::create_dir_all(unit.parent().expect("the unit has a parent"))
        .expect("the isolated systemd directory is created");
    fs::write(&unit, "an isolated unit").expect("the isolated unit is written");

    let output = invocation.run(&["service", "uninstall"]);

    assert_eq!(output.status.code(), Some(0));
    assert!(!unit.exists(), "the isolated unit should be removed");
    let calls = fs::read_to_string(invocation.systemctl_log())
        .expect("systemctl calls are recorded by the fake");
    assert!(calls.contains("--user disable --now laplus.service"));
    assert!(calls.contains("--user daemon-reload"));
}

#[test]
fn root_help_presents_the_supported_command_tree() {
    let output = laplus(&["--help"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in ["serve", "auth", "service"] {
        assert!(stdout.contains(command), "root help omitted {command}:\n{stdout}");
    }
    assert!(!stdout.contains("pair\n"), "unsupported shortcut leaked into help");
}

#[test]
fn nested_help_is_contextual() {
    let output = laplus(&["auth", "pairing", "create", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    for option in ["--ttl", "--label", "--base-url", "--json"] {
        assert!(stdout.contains(option), "create help omitted {option}:\n{stdout}");
    }
    assert!(!stdout.contains("service install"));
}

#[test]
fn version_is_the_compiled_product_version() {
    let output = laplus(&["--version"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        laplus_server::version::PRODUCT_VERSION
    );
}

#[test]
fn an_invalid_invocation_uses_the_usage_exit_code_and_stderr() {
    let output = laplus(&["service", "uninstall", "--port", "5000"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--port"));
}

#[test]
fn contradictory_exposure_flags_are_refused() {
    let output = laplus(&["serve", "--network", "--no-network"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}
