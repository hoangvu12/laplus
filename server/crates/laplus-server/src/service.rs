//! Run this server in the background, so a box reached over SSH keeps one after
//! the session closes.
//!
//! `laplus-server service install` writes a systemd **user** unit, enables it,
//! starts it and turns on lingering. `status` and `uninstall` are the other two
//! verbs. Linux with systemd only, which is the platform the question is asked
//! on: the desktop application is a window somebody opens, and a Mac laptop
//! wanting this wants launchd and a different file.
//!
//! Until this existed, `docs/running-headless.md` said under _Known gaps_ that
//! no unit ships with this and named the two things such a unit has to get
//! right. Both are settled here rather than in a snippet an operator pastes:
//!
//! - **An explicit `PATH`.** The one thing on this page that will cost you an
//!   hour. `claude` lives in `~/.local/bin`, node under `~/.nvm`, cargo under
//!   `~/.cargo`, and every one of those is wired in by `~/.profile` or
//!   `~/.bashrc` — neither of which systemd reads. [`installing_path`] takes the
//!   `PATH` of the shell that ran `service install`, which is the interactive
//!   one where the operator has already proved `claude` is findable, and bakes
//!   it into the unit. Upstream's unit sets no `PATH` at all
//!   (`apps/server/src/cloud/bootService.ts`), which is the same bug this
//!   project has already paid for three times.
//! - **Both streams captured.** `StandardOutput` and `StandardError` both append
//!   to one log, because the boot credential is printed on stdout and the
//!   complaint about a bundle that would not load is on stderr — an operator who
//!   cannot pair needs to read both in one place, in order.
//!
//! ## The binary the unit points at has to still be there tomorrow
//!
//! `npx laplus` runs out of `~/.npm/_npx/<hash>`, which npm may evict whenever
//! it likes. A unit pointing into that cache is a service that works until it
//! silently does not, at a moment nothing connects to the install. So
//! [`stage`] copies the binary and the UI bundle out to [`crate::config`]'s data
//! directory first, and the unit names the copies.
//!
//! Upstream has the same problem and a much harder version of it: `t3` is Node,
//! so pinning a runtime means a real `npm install --prefix` of the exact version
//! over the network, native modules and all — `apps/server/src/cloud/pinnedRuntime.ts`.
//! Ours is a static musl binary and a directory of files, so it is two copies
//! and no network. This is the second time ADR-0026's static linking has paid
//! for itself.
//!
//! **A binary that is already somewhere stable is left where it is.** A
//! developer running `cargo build --release && ./target/release/laplus-server
//! service install` wants the unit to follow their rebuilds, and a global
//! install is not going to be evicted either. Only an ephemeral cache path is
//! copied out — [`is_ephemeral`].

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What the unit file is called, under `~/.config/systemd/user`.
///
/// Not `laplus-server.service`: the operator types this in every `systemctl
/// --user` command they ever run against it, and the thing they are managing is
/// laplus.
pub const UNIT: &str = "laplus.service";

/// Which verb `service` was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// Write the unit and start it, or bring an out-of-date one up to what this
    /// binary would write now. One verb for both because the operator asking to
    /// install an already-installed service means "make it right", and a second
    /// spelling for a repair is a spelling nobody finds when they need it.
    Install,
    Status,
    Uninstall,
}

impl Verb {
    /// Parse the word after `service`.
    pub fn parse(word: &str) -> Result<Verb, String> {
        match word {
            "install" | "update" | "repair" => Ok(Verb::Install),
            "status" => Ok(Verb::Status),
            "uninstall" | "remove" => Ok(Verb::Uninstall),
            other => Err(format!(
                "unrecognised service command {other} — install, status or uninstall"
            )),
        }
    }
}

/// Everything the unit file needs, with every path already made absolute and
/// stable.
///
/// Pure data so [`render`] can be checked byte for byte without a systemd on the
/// machine running the test — which, on this project, is usually Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The server binary the unit runs. Absolute, and somewhere that outlives an
    /// npm cache eviction.
    pub binary: PathBuf,
    /// The UI bundle, if this install has one to serve.
    pub ui: Option<PathBuf>,
    /// The server flags the operator gave at install time, passed through to
    /// every start. `--network` is the one that matters: a background service on
    /// a VPS that binds loopback is a service nothing can reach.
    pub arguments: Vec<String>,
    /// `PATH` for the unit. See this module's header; this is the field that
    /// decides whether the agent works.
    pub path: String,
    pub unit_path: PathBuf,
    pub log_path: PathBuf,
}

/// What an install or uninstall did, so the caller can say so in one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Installed(Plan),
    /// The unit on disk already said exactly what this binary would write.
    Unchanged(Plan),
    Updated(Plan),
    Removed,
    NotInstalled,
}

/// Whether a service is installed, and whether it is the one this binary would
/// write now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Linux with a `HOME`. Everything else can read this page and no more.
    pub supported: bool,
    pub installed: bool,
    /// The unit on disk matches [`render`] for the plan this binary would make,
    /// **and** the binary it names is still there. Either half being false is a
    /// repair: `service install` again.
    pub current: bool,
    pub unit_path: PathBuf,
    pub log_path: PathBuf,
}

impl fmt::Display for Status {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.supported {
            return write!(
                form,
                "laplus service\n  status: unavailable here\n  supported on: Linux with systemd"
            );
        }
        if !self.installed {
            return write!(
                form,
                "laplus service\n  status: not installed\n  next: laplus-server service install"
            );
        }
        write!(
            form,
            "laplus service\n  status: {}\n  unit: {}\n  log: {}{}",
            if self.current {
                "installed"
            } else {
                "installed, out of date"
            },
            self.unit_path.display(),
            self.log_path.display(),
            if self.current {
                String::new()
            } else {
                "\n  next: laplus-server service install, to bring it up to this binary".to_string()
            }
        )
    }
}

/// The cache directories a package manager may empty without telling anybody.
///
/// Windows separators are in the list because the check is on a string this
/// binary was started from and there is no reason for it to be platform-aware;
/// a false negative here is a broken service and a false positive is one
/// harmless copy.
const EPHEMERAL: &[&str] = &[
    "/_npx/",
    "\\_npx\\",
    "/pnpm/dlx/",
    "/.pnpm/dlx/",
    "/.bun/install/cache/",
];

/// Is this path inside a package manager's scratch space?
///
/// The whole reason [`stage`] exists. `npx laplus service install` is the
/// expected way to run this, and the binary it is running from at that moment is
/// in a directory npm considers disposable.
pub fn is_ephemeral(path: &Path) -> bool {
    let shown = path.to_string_lossy();
    EPHEMERAL.iter().any(|segment| shown.contains(segment))
}

/// Double every `%`, which systemd would otherwise read as a specifier.
///
/// `%h` is the home directory, `%i` the instance name, and a data directory
/// with a literal `%` in it is a path systemd silently rewrites into a
/// different one. Applied to every value that reaches the file, including the
/// `append:` targets — those take the rest of the line literally and so must
/// **not** be quoted, which is why this is separate from [`quote`].
pub fn escape(value: &str) -> String {
    value.replace('%', "%%")
}

/// Quote a value systemd word-splits, escaping specifiers on the way.
///
/// `ExecStart` and `Environment` are split on whitespace, so a data directory
/// under `/home/some one/` is two arguments unless it is quoted.
pub fn quote(value: &str) -> String {
    let escaped = escape(value);
    if escaped
        .chars()
        .any(|character| character.is_whitespace() || character == '"' || character == '\\')
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}

/// The unit file, exactly as it lands on disk.
///
/// Pure, and the only place the file's text is decided — [`status`] compares the
/// file it finds against this rather than parsing it, so "is the installed
/// service the one this binary would write" is a string comparison and cannot
/// drift from what install does.
///
/// **No `After=network-online.target`.** That target does not exist in the
/// systemd *user* manager, so ordering on it is accepted and silently ignored;
/// `Restart=always` is what actually covers a server that started before the
/// box had an address. Upstream's unit carries the same note.
pub fn render(plan: &Plan) -> String {
    let mut execution = vec![quote(&plan.binary.to_string_lossy())];
    if let Some(ui) = &plan.ui {
        execution.push("--ui".to_string());
        execution.push(quote(&ui.to_string_lossy()));
    }
    execution.extend(plan.arguments.iter().map(|argument| quote(argument)));

    [
        "[Unit]".to_string(),
        "Description=laplus server".to_string(),
        // Five failures in five minutes and it stops trying. Without this a
        // permanently broken install — a deleted binary, a project directory
        // that went away — restarts every 5 seconds forever, and the append log
        // below has no rotation to save it.
        "StartLimitIntervalSec=300".to_string(),
        "StartLimitBurst=5".to_string(),
        String::new(),
        "[Service]".to_string(),
        "Type=simple".to_string(),
        "WorkingDirectory=%h".to_string(),
        format!("Environment=PATH={}", quote(&plan.path)),
        format!("ExecStart={}", execution.join(" ")),
        "Restart=always".to_string(),
        "RestartSec=5".to_string(),
        // Both streams, one file, in the order they happened. The header says
        // why: the credential is on one and the reason there is no page to use
        // it on is on the other.
        format!("StandardOutput=append:{}", escape(&plan.log_path.to_string_lossy())),
        format!("StandardError=append:{}", escape(&plan.log_path.to_string_lossy())),
        String::new(),
        "[Install]".to_string(),
        "WantedBy=default.target".to_string(),
        String::new(),
    ]
    .join("\n")
}

/// The `PATH` to bake into the unit, given the one this process was started
/// with.
///
/// The installing shell's `PATH` is the whole answer and the reason is worth
/// stating plainly: the operator has just run `laplus-server service install`
/// from a terminal where `claude` works. Copying that is the only way to be
/// right without a list of toolchain directories this file would then have to
/// keep up with — and a list is what `docs/running-headless.md` demonstrates
/// does not converge, having been surprised three separate times.
///
/// The system directories are appended if missing, because a `PATH` that
/// somehow lacks `/usr/bin` gives a service with no `git` and no shell.
pub fn installing_path(current: Option<&str>) -> String {
    let mut entries: Vec<String> = current
        .unwrap_or("")
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect();
    for fallback in ["/usr/local/bin", "/usr/bin", "/bin"] {
        if !entries.iter().any(|entry| entry == fallback) {
            entries.push(fallback.to_string());
        }
    }
    entries.join(":")
}

/// Where laplus keeps its files, for a caller outside this crate.
///
/// [`crate::config::data_dir`] is crate-private and stays that way; this is the
/// one question `main.rs` has to ask it, and asking it through here keeps the
/// staging directory a decision of this module.
pub fn data_directory() -> PathBuf {
    crate::config::data_dir()
}

/// Where a staged copy of the binary and the bundle live.
fn staged_in(data: &Path) -> (PathBuf, PathBuf) {
    (
        data.join("service").join("laplus-server"),
        data.join("service").join("ui"),
    )
}

/// Where the unit will point, without copying anything.
///
/// Split out of [`stage`] so `status` can work out what `install` *would* write
/// without touching the disk. Both asking the same function is what makes "is
/// the installed service current" answerable at all: a status that predicted the
/// paths differently from the install would report every service as out of date,
/// for ever.
pub fn destination(binary: &Path, ui: Option<&Path>, data: &Path) -> (PathBuf, Option<PathBuf>) {
    if !is_ephemeral(binary) {
        return (binary.to_path_buf(), ui.map(Path::to_path_buf));
    }
    let (staged_binary, staged_ui) = staged_in(data);
    (staged_binary, ui.map(|_| staged_ui))
}

/// Put the binary and the bundle somewhere that outlives an npm cache eviction,
/// and answer with where they ended up.
///
/// A stable original is returned untouched — see this module's header. The copy
/// is unconditional when it happens rather than skipped on a matching
/// modification time, because the cost is one file and the failure it prevents
/// is a service running a version nobody can identify.
pub fn stage(
    binary: &Path,
    ui: Option<&Path>,
    data: &Path,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    if !is_ephemeral(binary) {
        return Ok((binary.to_path_buf(), ui.map(Path::to_path_buf)));
    }
    let (staged_binary, staged_ui) = staged_in(data);
    let directory = staged_binary
        .parent()
        .ok_or_else(|| "the data directory has no parent".to_string())?;
    std::fs::create_dir_all(directory)
        .map_err(|failure| format!("cannot make {}: {failure}", directory.display()))?;

    // Remove first: copying onto a running binary is ETXTBSY, and the service
    // being replaced is exactly the process holding it open.
    let _ = std::fs::remove_file(&staged_binary);
    std::fs::copy(binary, &staged_binary).map_err(|failure| {
        format!(
            "cannot copy {} to {}: {failure}",
            binary.display(),
            staged_binary.display()
        )
    })?;
    make_executable(&staged_binary)?;

    let ui = match ui {
        None => None,
        Some(source) => {
            let _ = std::fs::remove_dir_all(&staged_ui);
            copy_tree(source, &staged_ui)?;
            Some(staged_ui)
        }
    };
    Ok((staged_binary, ui))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|failure| format!("cannot make {} executable: {failure}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Copy a directory and everything under it.
///
/// The bundle is a few hundred files and no symlinks, so this is the whole of
/// what is needed and a dependency to do it would not earn its place.
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to)
        .map_err(|failure| format!("cannot make {}: {failure}", to.display()))?;
    let entries = std::fs::read_dir(from)
        .map_err(|failure| format!("cannot read {}: {failure}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|failure| format!("cannot read {}: {failure}", from.display()))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|failure| format!("cannot stat {}: {failure}", source.display()))?;
        if kind.is_dir() {
            copy_tree(&source, &target)?;
        } else {
            std::fs::copy(&source, &target)
                .map_err(|failure| format!("cannot copy {}: {failure}", source.display()))?;
        }
    }
    Ok(())
}

/// Where systemd reads user units from, and where the log goes.
fn locations(home: &Path, data: &Path) -> (PathBuf, PathBuf) {
    (
        home.join(".config").join("systemd").join("user").join(UNIT),
        data.join("logs").join("service.log"),
    )
}

/// The home directory this is all relative to, or the reason there is none.
fn home() -> Result<PathBuf, String> {
    std::env::var("HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set, so there is no user systemd directory".to_string())
}

/// Whether this machine can have one of these at all.
fn supported() -> bool {
    cfg!(target_os = "linux") && home().is_ok()
}

/// Build the plan this binary would install right now.
///
/// `arguments` are the server flags to bake into the unit — whatever followed
/// `service install` on the command line.
pub fn plan(arguments: Vec<String>, ui: Option<PathBuf>) -> Result<Plan, String> {
    let home = home()?;
    let data = crate::config::data_dir();
    let binary = std::env::current_exe()
        .map_err(|failure| format!("cannot find this binary on disk: {failure}"))?;
    let (unit_path, log_path) = locations(&home, &data);
    Ok(Plan {
        binary,
        ui,
        arguments,
        path: installing_path(std::env::var("PATH").ok().as_deref()),
        unit_path,
        log_path,
    })
}

/// Write the unit, enable it, start it, and keep it running after logout.
///
/// **Lingering failing is a warning and not a rollback**, which is where this
/// departs from upstream. `loginctl enable-linger` can want a polkit
/// authorisation the operator has no way to give over `ssh host cmd`, and
/// tearing down a service that is installed and running because it will not
/// survive a logout throws away the working nine tenths of what was asked for.
/// The second return value is what could not be finished, said plainly.
pub fn install(plan: &Plan) -> Result<(Outcome, Vec<String>), String> {
    if !supported() {
        return Err(
            "the background service needs Linux with systemd; nothing was changed".to_string(),
        );
    }
    let existing = std::fs::read_to_string(&plan.unit_path).ok();
    let wanted = render(plan);
    let staged_exists = plan.binary.exists();
    if existing.as_deref() == Some(wanted.as_str()) && staged_exists {
        return Ok((Outcome::Unchanged(plan.clone()), Vec::new()));
    }

    if let Some(directory) = plan.log_path.parent() {
        std::fs::create_dir_all(directory)
            .map_err(|failure| format!("cannot make {}: {failure}", directory.display()))?;
    }
    let directory = plan
        .unit_path
        .parent()
        .ok_or_else(|| "the unit path has no parent".to_string())?;
    std::fs::create_dir_all(directory)
        .map_err(|failure| format!("cannot make {}: {failure}", directory.display()))?;
    std::fs::write(&plan.unit_path, &wanted)
        .map_err(|failure| format!("cannot write {}: {failure}", plan.unit_path.display()))?;

    // If activation fails partway, put back what was there. A unit file left
    // behind by a failed install is a service `status` calls installed and
    // systemd never enabled, and a dangling wants/ symlink logs "Failed to load
    // unit" at every boot.
    let activation = (|| {
        run("reloading the user units", "systemctl", &["--user", "daemon-reload"])?;
        run("enabling the service", "systemctl", &["--user", "enable", UNIT])?;
        // restart rather than `enable --now`: --now leaves an already-running
        // process alone, so repairing a stale unit would keep serving the old
        // binary until the box rebooted.
        run("starting the service", "systemctl", &["--user", "restart", UNIT])
    })();

    if let Err(failure) = activation {
        match &existing {
            Some(previous) => {
                let _ = std::fs::write(&plan.unit_path, previous);
                let _ = run("reloading the user units", "systemctl", &["--user", "daemon-reload"]);
                let _ = run("restoring the service", "systemctl", &["--user", "restart", UNIT]);
            }
            None => {
                let _ = run(
                    "removing the service",
                    "systemctl",
                    &["--user", "disable", "--now", UNIT],
                );
                let _ = std::fs::remove_file(&plan.unit_path);
                let _ = run("reloading the user units", "systemctl", &["--user", "daemon-reload"]);
            }
        }
        return Err(failure);
    }

    // The point of the whole exercise on a box reached over SSH: without
    // lingering the user manager stops when the last session closes, and the
    // service stops with it.
    let mut warnings = Vec::new();
    if let Err(failure) = run("enabling lingering", "loginctl", &["enable-linger"]) {
        warnings.push(format!(
            "{failure} — the service is running, but it will stop when you log out. \
             Run `sudo loginctl enable-linger $USER` to fix that."
        ));
    }

    Ok((
        match existing {
            Some(_) => Outcome::Updated(plan.clone()),
            None => Outcome::Installed(plan.clone()),
        },
        warnings,
    ))
}

/// Stop it, remove it from startup, and delete the unit.
///
/// The staged binary and bundle are **left alone**. They are this server, and an
/// operator who ran `service uninstall` asked to stop it starting itself, not to
/// uninstall laplus.
pub fn uninstall() -> Result<Outcome, String> {
    if !supported() {
        return Err("the background service needs Linux with systemd".to_string());
    }
    let (unit_path, _) = locations(&home()?, &crate::config::data_dir());
    if !unit_path.exists() {
        return Ok(Outcome::NotInstalled);
    }
    run(
        "stopping the service",
        "systemctl",
        &["--user", "disable", "--now", UNIT],
    )?;
    std::fs::remove_file(&unit_path)
        .map_err(|failure| format!("cannot remove {}: {failure}", unit_path.display()))?;
    run("reloading the user units", "systemctl", &["--user", "daemon-reload"])?;
    Ok(Outcome::Removed)
}

/// What is installed, and whether it is what this binary would install.
pub fn status(plan: Option<&Plan>) -> Result<Status, String> {
    let data = crate::config::data_dir();
    let (unit_path, log_path) = match home() {
        Ok(home) => locations(&home, &data),
        Err(_) => (PathBuf::from(UNIT), data.join("logs").join("service.log")),
    };
    if !supported() {
        return Ok(Status {
            supported: false,
            installed: false,
            current: false,
            unit_path,
            log_path,
        });
    }
    let Ok(unit) = std::fs::read_to_string(&unit_path) else {
        return Ok(Status {
            supported: true,
            installed: false,
            current: false,
            unit_path,
            log_path,
        });
    };
    // Current means two things, and the second is not obvious: the staged binary
    // can be deleted to reclaim space, leaving a unit that is textually perfect
    // and names nothing.
    let current = match plan {
        Some(plan) => started_the_same_way(&unit, &render(plan)) && plan.binary.exists(),
        None => false,
    };
    Ok(Status {
        supported: true,
        installed: true,
        current,
        unit_path,
        log_path,
    })
}

/// Do two units start the same server the same way?
///
/// **`PATH` is deliberately not part of this**, and it is the only line left
/// out. It records the shell that ran the install, so an operator who installs
/// from `bash` and asks `service status` from a login shell with one more entry
/// on `PATH` would otherwise be told their service is out of date — every time,
/// with a repair that changes nothing they can see. What makes a service stale
/// is the *server it starts*: a different binary, a different bundle, different
/// flags. That is `ExecStart`, and comparing it is comparing all of it.
fn started_the_same_way(unit: &str, wanted: &str) -> bool {
    execution_in(unit) == execution_in(wanted) && execution_in(unit).is_some()
}

fn execution_in(unit: &str) -> Option<&str> {
    unit.lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
}

/// Run one activation step, and say which step it was if it fails.
///
/// `systemctl --user` failures are otherwise invisible: the exit code is all
/// there is, and the operator sees a command that printed nothing.
fn run(step: &str, program: &str, arguments: &[&str]) -> Result<(), String> {
    let finished = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|failure| format!("cannot run {program} while {step}: {failure}"))?;
    if finished.status.success() {
        return Ok(());
    }
    let complaint = String::from_utf8_lossy(&finished.stderr);
    let complaint = complaint.trim();
    Err(format!(
        "failed while {step}{}",
        if complaint.is_empty() {
            String::new()
        } else {
            format!(": {complaint}")
        }
    ))
}

/// The environment a unit would be written from, for tests that want to check
/// the file without a home directory to write it into.
#[cfg(test)]
fn plan_for(binary: &str, ui: Option<&str>, arguments: &[&str]) -> Plan {
    Plan {
        binary: PathBuf::from(binary),
        ui: ui.map(PathBuf::from),
        arguments: arguments.iter().map(|argument| argument.to_string()).collect(),
        path: "/home/ubuntu/.local/bin:/usr/bin".to_string(),
        unit_path: PathBuf::from("/home/ubuntu/.config/systemd/user/laplus.service"),
        log_path: PathBuf::from("/home/ubuntu/.laplus/logs/service.log"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unit_runs_the_binary_with_the_bundle_and_the_flags() {
        let unit = render(&plan_for(
            "/home/ubuntu/.laplus/service/laplus-server",
            Some("/home/ubuntu/.laplus/service/ui"),
            &["--network", "--port", "4773"],
        ));
        assert!(unit.contains(
            "ExecStart=/home/ubuntu/.laplus/service/laplus-server \
             --ui /home/ubuntu/.laplus/service/ui --network --port 4773"
        ));
    }

    // The field this whole module exists for. A unit without it starts a server
    // that cannot find `claude`, which presents as laplus being broken rather
    // than as a PATH problem.
    #[test]
    fn the_unit_carries_the_installing_shells_path() {
        let unit = render(&plan_for("/opt/laplus-server", None, &[]));
        assert!(unit.contains("Environment=PATH=/home/ubuntu/.local/bin:/usr/bin"));
    }

    #[test]
    fn both_streams_land_in_one_log() {
        let unit = render(&plan_for("/opt/laplus-server", None, &[]));
        assert!(unit.contains("StandardOutput=append:/home/ubuntu/.laplus/logs/service.log"));
        assert!(unit.contains("StandardError=append:/home/ubuntu/.laplus/logs/service.log"));
    }

    #[test]
    fn a_restart_loop_gives_up_rather_than_growing_the_log_forever() {
        let unit = render(&plan_for("/opt/laplus-server", None, &[]));
        assert!(unit.contains("StartLimitIntervalSec=300"));
        assert!(unit.contains("StartLimitBurst=5"));
        assert!(unit.contains("Restart=always"));
    }

    // systemd would read a lone `%` as a specifier and write a different path
    // than the one the operator has their files in.
    #[test]
    fn a_percent_in_a_path_is_doubled_rather_than_expanded() {
        let plan = Plan {
            log_path: PathBuf::from("/home/ubuntu/100%/service.log"),
            ..plan_for("/opt/100%/laplus-server", None, &[])
        };
        let unit = render(&plan);
        assert!(unit.contains("StandardOutput=append:/home/ubuntu/100%%/service.log"));
        assert!(unit.contains("ExecStart=/opt/100%%/laplus-server"));
    }

    // `append:` takes the rest of the line literally, so quoting the path would
    // put quotation marks in the filename.
    #[test]
    fn the_log_path_is_escaped_but_never_quoted() {
        let plan = Plan {
            log_path: PathBuf::from("/home/some one/logs/service.log"),
            ..plan_for("/opt/laplus-server", None, &[])
        };
        let unit = render(&plan);
        assert!(unit.contains("StandardOutput=append:/home/some one/logs/service.log"));
    }

    // ExecStart is word-split, so the same path there must be quoted.
    #[test]
    fn a_space_in_the_binary_path_is_quoted() {
        let unit = render(&plan_for("/opt/la plus/laplus-server", None, &[]));
        assert!(unit.contains("ExecStart=\"/opt/la plus/laplus-server\""));
    }

    #[test]
    fn no_ui_flag_when_there_is_no_bundle() {
        let unit = render(&plan_for("/opt/laplus-server", None, &[]));
        assert!(!unit.contains("--ui"));
    }

    // Ordering on a target the user manager does not have is accepted and
    // silently ignored, which is worse than not asking.
    #[test]
    fn nothing_orders_on_a_target_the_user_manager_has_not_got() {
        let unit = render(&plan_for("/opt/laplus-server", None, &[]));
        assert!(!unit.contains("network-online.target"));
    }

    #[test]
    fn an_npx_cache_is_ephemeral_and_a_release_build_is_not() {
        assert!(is_ephemeral(Path::new(
            "/home/ubuntu/.npm/_npx/bfd8de85db250405/node_modules/@laplus/server-linux-arm64/laplus-server"
        )));
        assert!(is_ephemeral(Path::new(
            "/home/ubuntu/.cache/pnpm/dlx/abc/node_modules/laplus-server"
        )));
        assert!(!is_ephemeral(Path::new(
            "/home/ubuntu/laplus/server/target/release/laplus-server"
        )));
        assert!(!is_ephemeral(Path::new("/usr/local/bin/laplus-server")));
    }

    #[test]
    fn the_installing_path_is_kept_and_the_system_directories_are_ensured() {
        assert_eq!(
            installing_path(Some("/home/ubuntu/.local/bin:/usr/bin")),
            "/home/ubuntu/.local/bin:/usr/bin:/usr/local/bin:/bin"
        );
    }

    #[test]
    fn an_empty_path_still_gives_a_service_with_a_shell() {
        assert_eq!(installing_path(None), "/usr/local/bin:/usr/bin:/bin");
        assert_eq!(installing_path(Some("")), "/usr/local/bin:/usr/bin:/bin");
    }

    #[test]
    fn the_verbs_have_the_spellings_an_operator_reaches_for() {
        assert_eq!(Verb::parse("install"), Ok(Verb::Install));
        assert_eq!(Verb::parse("update"), Ok(Verb::Install));
        assert_eq!(Verb::parse("repair"), Ok(Verb::Install));
        assert_eq!(Verb::parse("status"), Ok(Verb::Status));
        assert_eq!(Verb::parse("uninstall"), Ok(Verb::Uninstall));
        assert_eq!(Verb::parse("remove"), Ok(Verb::Uninstall));
        assert!(Verb::parse("start").is_err());
    }

    #[test]
    fn an_unsupported_machine_says_so_rather_than_saying_not_installed() {
        let status = Status {
            supported: false,
            installed: false,
            current: false,
            unit_path: PathBuf::from("laplus.service"),
            log_path: PathBuf::from("service.log"),
        };
        assert!(status.to_string().contains("unavailable here"));
        assert!(status.to_string().contains("Linux with systemd"));
    }

    #[test]
    fn an_out_of_date_service_says_what_to_run() {
        let status = Status {
            supported: true,
            installed: true,
            current: false,
            unit_path: PathBuf::from("/home/ubuntu/.config/systemd/user/laplus.service"),
            log_path: PathBuf::from("/home/ubuntu/.laplus/logs/service.log"),
        };
        assert!(status.to_string().contains("out of date"));
        assert!(status.to_string().contains("service install"));
    }

    // The shell that asks is not evidence about the service. Without this,
    // installing from one shell and asking from another reports a repair that
    // would change nothing an operator can see — every time.
    #[test]
    fn a_different_path_does_not_make_a_service_out_of_date() {
        let installed = render(&plan_for("/opt/laplus-server", None, &["--network"]));
        let asking = render(&Plan {
            path: "/usr/bin:/snap/bin".to_string(),
            ..plan_for("/opt/laplus-server", None, &["--network"])
        });
        assert_ne!(installed, asking);
        assert!(started_the_same_way(&installed, &asking));
    }

    // What does make it stale: a different server, however it differs.
    #[test]
    fn a_different_binary_bundle_or_flag_does_make_it_out_of_date() {
        let installed = render(&plan_for("/opt/laplus-server", Some("/opt/ui"), &["--network"]));
        for changed in [
            plan_for("/opt/other/laplus-server", Some("/opt/ui"), &["--network"]),
            plan_for("/opt/laplus-server", Some("/opt/other-ui"), &["--network"]),
            plan_for("/opt/laplus-server", Some("/opt/ui"), &[]),
            plan_for("/opt/laplus-server", None, &["--network"]),
        ] {
            assert!(
                !started_the_same_way(&installed, &render(&changed)),
                "{:?} should have been out of date",
                changed.binary
            );
        }
    }

    // A file that is not one of ours at all — an operator's hand-written unit,
    // or a truncated write — is not "the same way" by accident.
    #[test]
    fn a_unit_with_no_exec_line_is_never_current() {
        assert!(!started_the_same_way("[Unit]\nDescription=laplus server\n", ""));
    }

    // `status` has to predict what `install` would do, without doing it.
    #[test]
    fn the_predicted_destination_is_the_one_staging_uses() {
        let scratch = tempfile::tempdir().unwrap();
        let cache = scratch.path().join("_npx").join("hash");
        std::fs::create_dir_all(cache.join("ui")).unwrap();
        let binary = cache.join("laplus-server");
        std::fs::write(&binary, "ELF").unwrap();
        let data = scratch.path().join("data");

        let predicted = destination(&binary, Some(&cache.join("ui")), &data);
        let staged = stage(&binary, Some(&cache.join("ui")), &data).unwrap();
        assert_eq!(predicted, staged);
    }

    #[test]
    fn a_stable_binary_is_predicted_where_it_already_is() {
        let binary = PathBuf::from("/usr/local/bin/laplus-server");
        let ui = PathBuf::from("/usr/local/share/laplus/ui");
        assert_eq!(
            destination(&binary, Some(&ui), Path::new("/home/ubuntu/.laplus")),
            (binary, Some(ui))
        );
    }

    // A stable path is what the unit points at directly; only a cache is copied.
    #[test]
    fn staging_leaves_a_stable_binary_where_it_is() {
        let binary = PathBuf::from("/usr/local/bin/laplus-server");
        let staged = stage(&binary, None, Path::new("/home/ubuntu/.laplus")).unwrap();
        assert_eq!(staged, (binary, None));
    }

    #[test]
    fn staging_copies_a_cached_binary_and_its_bundle_out() {
        let scratch = tempfile::tempdir().unwrap();
        let cache = scratch.path().join("_npx").join("hash");
        std::fs::create_dir_all(cache.join("ui").join("dist")).unwrap();
        let binary = cache.join("laplus-server");
        std::fs::write(&binary, "ELF").unwrap();
        std::fs::write(cache.join("ui").join("dist").join("index.html"), "<html>").unwrap();

        let data = scratch.path().join("data");
        let (staged_binary, staged_ui) = stage(&binary, Some(&cache.join("ui")), &data).unwrap();

        assert_eq!(staged_binary, data.join("service").join("laplus-server"));
        assert_eq!(std::fs::read_to_string(&staged_binary).unwrap(), "ELF");
        let staged_ui = staged_ui.unwrap();
        assert_eq!(
            std::fs::read_to_string(staged_ui.join("dist").join("index.html")).unwrap(),
            "<html>"
        );
    }
}
