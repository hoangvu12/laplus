//! Starting child processes the way this server needs them started, and
//! finding the programs to start.
//!
//! Small, and it exists because the same four lines were about to be written a
//! third time. The server shells out to `git` for the file tree
//! ([`crate::filesystem`]), starts the developer's editor
//! ([`crate::editor`]), resolves and runs the agent binary
//! ([`crate::provider`]), and will drive a shell in the tickets after this one.
//! Every one of them has the same Windows problem, and two of them have the
//! same question — *is this program on this machine, and where* — which
//! [`Search`] answers.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
#[cfg(windows)]
use std::process::Stdio;

/// Start this child without giving it a console window.
///
/// On Windows a process started from a GUI application gets a console of its
/// own unless it is told not to, so without this a black window flashes on
/// screen every time the file tree is scanned or an editor is launched — once
/// the server is inside the Tauri shell (ticket 23), which is where it will
/// spend its life. It is a visible bug for something the user never asked to
/// see.
///
/// A no-op everywhere else: the flag is a Windows creation flag and no other
/// platform has the problem.
pub fn without_a_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `CREATE_NO_WINDOW`, from the Windows process-creation flags.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Bind this child's life to this server's — in the kernel, rather than in this
/// code.
///
/// **Every other reaping path in this crate is cooperative.** [`crate::agent`]'s
/// `Agent::stop`, [`crate::codex`]'s `AppServer::stop` and [`crate::opencode`]'s
/// `OwnedServer::stop` all run because `Server::shutdown` called them, and
/// `Server::shutdown` runs because Tauri raised `RunEvent::ExitRequested` or
/// because [`crate::server`]'s `asked_to_stop` heard a signal. None of that
/// happens when laplus is ended from Task Manager, terminated with
/// `taskkill /F`, or dies on a panic. `kill_on_drop` is no help either: it
/// needs a tokio runtime that, in exactly those cases, no longer exists.
///
/// What the gap costs was measured rather than supposed. On 2026-09-01 this
/// machine was holding two `codex app-server` trees and six dev servers started
/// three days earlier — every one with a dead parent, none of them visible to a
/// restarted laplus, together holding four loopback ports. Upstream has the same
/// hole open as `pingdotgg/t3code#5241`, where the reporter counted twenty-eight
/// orphaned `opencode serve` processes and 8.8 GB of resident memory, and
/// reached the conclusion this server would also have reached: their inactivity
/// reaper cannot see an orphan, because it only closes sessions a live backend
/// still owns.
///
/// A job object closes it. The handle belongs to this process; when this process
/// ends by any means, including ones it cannot run code after, the kernel closes
/// the handle and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` terminates every process
/// in the job. **Membership is inherited**, which is the half that matters most:
/// assigning the `cmd.exe` that fronts `codex.cmd` covers the `node` and
/// `codex.exe` beneath it, and assigning `claude.exe` covers the dev servers its
/// Bash tool starts — which is the leak that was actually on the machine, and
/// the one no amount of care in `Agent::stop` could have reached, because those
/// processes are not this server's children and never were.
///
/// **A backstop, not a replacement.** The graceful paths still do the work and
/// still do it better: they close stdin, let the CLI finish its output, and
/// collect what it said on the way down. A job object has none of that
/// discretion — it is what decides the outcome when none of them get to run.
///
/// **Failure is reported and not fatal**, for the same reason `asked_to_stop`
/// treats a handler that will not install that way: a server that refused to
/// start because it could not create a job object would supervise its children
/// worse than one that carries on exactly as every version before this did.
pub fn bound_to_this_server(child: &Child) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        join_the_job(child.as_raw_handle() as isize);
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// [`bound_to_this_server`], for the children started onto the async runtime.
///
/// Two functions rather than one over a trait because the two `Child` types
/// share no handle accessor, and a trait implemented for both would need its own
/// `cfg(not(windows))` twin to keep the signature — more code, in a module whose
/// reason for existing is that the same four lines were about to be written a
/// third time.
///
/// A child whose handle has already gone is one that has already exited, so
/// there is nothing to bind and nothing to report.
pub fn bound_to_this_server_async(child: &tokio::process::Child) {
    #[cfg(windows)]
    if let Some(handle) = child.raw_handle() {
        join_the_job(handle as isize);
    }
    #[cfg(not(windows))]
    let _ = child;
}

/// The one job object this process owns, created on first use.
///
/// A `OnceLock` rather than a field on the server, because the thing being
/// modelled is a property of the *process* — the kernel closes the handle when
/// the process ends, and nothing finer-grained than that is what makes the
/// guarantee true. It also means a child started before any `Server` exists, or
/// after one has been dropped, is covered on the same terms as every other.
///
/// `None` once creation has failed: the failure is reported at the point it
/// happens and never again, so a machine that will not give this server a job
/// object does not also print a line per child for the rest of the session.
#[cfg(windows)]
static SUPERVISION: std::sync::OnceLock<Option<win32job::Job>> = std::sync::OnceLock::new();

/// A job object that terminates its members when its last handle closes.
///
/// Separate from [`SUPERVISION`] only so the tests can hold one of their own:
/// the guarantee is "when the handle closes", and a static that lives as long as
/// the process is by construction something a test in that process can never
/// watch close.
#[cfg(windows)]
fn supervision_job() -> Result<win32job::Job, win32job::JobError> {
    let mut limits = win32job::ExtendedLimitInfo::new();
    limits.limit_kill_on_job_close();
    win32job::Job::create_with_limit_info(&limits)
}

/// Put one process handle into [`SUPERVISION`], if there is one to put it in.
#[cfg(windows)]
fn join_the_job(handle: isize) {
    let job = SUPERVISION.get_or_init(|| {
        match supervision_job() {
            Ok(job) => Some(job),
            Err(error) => {
                eprintln!(
                    "laplus: cannot supervise child processes through a job object: {error}. \
                     Agents will still be stopped when laplus exits normally, but one that \
                     survives an abrupt exit will have to be ended by hand."
                );
                None
            }
        }
    });

    // A process that is already in a job this server does not own — a CI
    // container, a debugger — cannot always be re-assigned, and one that exited
    // between `spawn` and here cannot be assigned at all. Both are ordinary, and
    // both leave the cooperative paths doing what they already did.
    if let Some(job) = job.as_ref() {
        if let Err(error) = job.assign_process(handle) {
            eprintln!("laplus: a child process could not be supervised: {error}");
        }
    }
}

/// Terminate a child and everything it launched, then wait for the child.
///
/// Windows command shims are processes, not aliases: starting `codex.cmd`
/// gives this server a `cmd.exe` whose Codex process is its child. Killing only
/// the handle returned by [`Command::spawn`] leaves the process that owns the
/// protocol alive. `taskkill /T` follows that tree before terminating it. Other
/// platforms start the resolved native Codex executable directly, so the child
/// itself is the tree root that needs reaping.
pub fn terminate_tree_and_wait(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }

    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill.exe");
        command
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        without_a_console(&mut command);
        let _ = command.status();
    }

    let _ = child.kill();
    let _ = child.wait();
}

/// [`terminate_tree_and_wait`], for a child on the async runtime.
///
/// **A job object does not make this redundant, and the two answer different
/// questions.** [`bound_to_this_server`] decides what happens to a child when
/// *laplus* ends; this decides what happens when a *conversation* does, which is
/// the common case and the earlier one. The job stays open for as long as the
/// server runs, so a `claude` reaped at the end of a turn takes its `bun dev`
/// with it only if something walks the tree here — otherwise the dev server
/// lives until laplus exits, which on a machine left open for a week is not a
/// bound at all.
pub async fn terminate_tree_and_wait_async(child: &mut tokio::process::Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }

    #[cfg(windows)]
    if let Some(pid) = child.id() {
        let mut command = tokio::process::Command::new("taskkill.exe");
        command
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        without_a_console(command.as_std_mut());
        let _ = command.status().await;
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
}

/// Where to look for a program, and what counts as startable when it is found.
///
/// An argument rather than a read of `std::env` at the point of use, and not
/// only for the tests' benefit. `PATH` is process-global mutable state: a suite
/// that had to set it in order to drive a lookup could not run its tests in
/// parallel, and a resolver that read it directly could not be asked "what
/// would you find in *this* directory?" at all — which is exactly what
/// [`crate::provider`]'s tests need to ask without a `claude` installed.
///
/// Built once and reused. A `PATH` walk by hand rather than `where`/`which`,
/// which would be a process spawn per candidate — twenty-odd of them at startup
/// for the editor list alone, each flashing a console window on Windows.
#[derive(Debug, Clone)]
pub struct Search {
    directories: Vec<PathBuf>,
    /// The suffixes a bare name may acquire before it names a file. `PATHEXT`
    /// on Windows, empty everywhere else — an executable bit is not a suffix.
    extensions: Vec<String>,
}

impl Search {
    /// What the machine this server is running on offers.
    pub fn from_environment() -> Search {
        let directories = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect())
            .unwrap_or_default();
        Search {
            directories,
            extensions: executable_extensions(),
        }
    }

    /// A lookup that may only see `directories`, with this platform's own idea
    /// of what is executable inside them.
    pub fn over(directories: &[&Path]) -> Search {
        Search {
            directories: directories.iter().map(PathBuf::from).collect(),
            extensions: executable_extensions(),
        }
    }

    /// The directories this lookup would walk, in order. Part of the diagnostic
    /// when a program is not found: "looked and did not find it" is only
    /// actionable next to *where*.
    pub fn directories(&self) -> &[PathBuf] {
        &self.directories
    }

    /// The first startable file named `command` in any of the directories.
    ///
    /// On Windows a bare name is not enough — `code` is `code.cmd` and `claude`
    /// is `claude.exe` — so every suffix in `PATHEXT` is tried in each
    /// directory before moving to the next one, which is the order the shell
    /// itself uses.
    pub fn locate(&self, command: &str) -> Option<PathBuf> {
        self.directories
            .iter()
            .find_map(|directory| self.startable(&directory.join(command)))
    }

    /// This path, or this path with one of the platform's executable suffixes
    /// on the end — whichever is a program that can be started.
    ///
    /// The suffix is *appended* rather than substituted, because
    /// `Path::with_extension` would turn `code.insiders` into `code.EXE` for any
    /// name that already contains a dot.
    pub fn startable(&self, path: &Path) -> Option<PathBuf> {
        if self.is_executable(path) {
            return Some(path.to_path_buf());
        }
        self.extensions.iter().find_map(|extension| {
            let mut name = path.as_os_str().to_os_string();
            name.push(extension);
            let candidate = PathBuf::from(name);
            self.is_executable(&candidate).then_some(candidate)
        })
    }

    /// Can this path be started?
    ///
    /// On Windows there is no executable bit and the extension is what decides,
    /// against the same `PATHEXT` the shell uses. On Unix it is a mode bit. Both
    /// require a *file*: a directory named `claude` is not a program, and
    /// telling the two apart is the difference between two of the diagnostics
    /// [`crate::provider`] produces.
    pub fn is_executable(&self, path: &Path) -> bool {
        if !path.is_file() {
            return false;
        }

        #[cfg(windows)]
        {
            let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                return false;
            };
            self.extensions
                .iter()
                .any(|candidate| candidate[1..].eq_ignore_ascii_case(extension))
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(path)
                .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }
    }
}

/// `PATHEXT` on Windows, nothing anywhere else. Each entry keeps its leading
/// dot, so it can be appended to a bare name as it stands.
///
/// **Lower-cased**, though `PATHEXT` is conventionally upper. Nothing about
/// *finding* the file depends on it — the filesystem is case-insensitive and
/// [`Search::is_executable`] compares without regard to case — but the path that
/// is found is the path a diagnostic prints, and `claude.CMD` next to a
/// `claude.cmd` on disk reads as a different file. Installers write lower case,
/// so this matches what is there in the case that happens.
fn executable_extensions() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .map(str::trim)
        .filter(|extension| extension.starts_with('.') && extension.len() > 1)
        .map(str::to_lowercase)
        .collect()
}

#[cfg(all(test, windows))]
mod supervision {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::time::{Duration, Instant};

    /// A hang detector, not a budget — the same rule as `READ_TIMEOUT` in the
    /// integration harness. Every assertion below is about *what* the kernel
    /// did, and this only bounds how long the test waits to see it.
    const SETTLES_WITHIN: Duration = Duration::from_secs(10);

    /// Something that stays alive without a console, a network, or a file on
    /// disk, and that starts a child of its own. `cmd.exe` is the parent and
    /// `ping` is the grandchild, which is the shape that matters: it is
    /// `codex.cmd`'s tree, and it is `claude` starting a dev server.
    fn a_tree() -> std::process::Child {
        Command::new("cmd.exe")
            .args(["/C", "ping -n 60 127.0.0.1 > nul"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd.exe starts")
    }

    /// Wait for the job to hold at least `wanted` processes, and answer with
    /// what it held when the wait ended.
    fn holds_at_least(job: &win32job::Job, wanted: usize) -> usize {
        let deadline = Instant::now() + SETTLES_WITHIN;
        loop {
            let held = job.query_process_id_list().expect("the job can be queried").len();
            if held >= wanted || Instant::now() >= deadline {
                return held;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The whole of what the job object is for, in one assertion each.
    ///
    /// **Inheritance is the first half.** Assigning the process this server
    /// started is only worth anything if what *it* starts joins too — that is
    /// the difference between reaping `cmd.exe` and reaping the `codex.exe`
    /// underneath it, and between reaping `claude.exe` and reaping the `bun dev`
    /// it left behind, which is the leak this was written for.
    ///
    /// **Closing the handle is the second.** Dropping the job here stands in for
    /// the process exiting, which is the only way this guarantee is ever
    /// invoked in production and not something a test inside that process can
    /// arrange. What the kernel does on the last handle closing is the same
    /// either way.
    #[test]
    fn a_supervised_tree_joins_whole_and_dies_when_the_job_handle_closes() {
        let job = supervision_job().expect("a job object");
        let mut child = a_tree();
        job.assign_process(child.as_raw_handle() as isize)
            .expect("the child joins the job");

        assert!(
            holds_at_least(&job, 2) >= 2,
            "the child's own child did not inherit the job, so reaping the tree \
             would still have left the process that does the work"
        );

        drop(job);

        let deadline = Instant::now() + SETTLES_WITHIN;
        loop {
            match child.try_wait().expect("the child can be observed") {
                Some(_) => break,
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    panic!("the supervised tree outlived the job handle that bounded it");
                }
                None => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }

    /// The path every spawn site actually calls, against the process-wide job.
    ///
    /// It cannot assert the reap — [`SUPERVISION`] closes when this test process
    /// does — so what it pins is that a real child is accepted into the job and
    /// is a member afterwards. The reap itself is the test above.
    #[test]
    fn the_spawn_sites_route_a_child_into_the_process_wide_job() {
        let mut child = a_tree();
        bound_to_this_server(&child);

        let members = SUPERVISION
            .get()
            .expect("supervision was initialised by the call above")
            .as_ref()
            .expect("this platform gives laplus a job object")
            .query_process_id_list()
            .expect("the job can be queried");

        assert!(
            members.contains(&(child.id() as usize)),
            "a child that went through `bound_to_this_server` is not in the job: {members:?}"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    /// A child that has already exited is not an error to bind. `Agent::stop`
    /// and `AppServer::stop` both race a CLI that exits on its own, so this is
    /// an ordinary case rather than a defensive one.
    #[test]
    fn binding_a_child_that_has_already_gone_is_not_a_failure() {
        let mut child = Command::new("cmd.exe")
            .args(["/C", "exit"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("cmd.exe starts");
        let _ = child.wait();

        bound_to_this_server(&child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file this platform will agree is a program, and one it will not.
    fn program(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(if cfg!(windows) {
            format!("{name}.cmd")
        } else {
            name.to_string()
        });
        std::fs::write(&path, "").expect("writes the file");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("sets the mode");
        }
        path
    }

    /// The base case, and its negative: a name that is somewhere on the list is
    /// found wherever on it that is, and one that is nowhere is not "found" at the
    /// first directory that could have held it.
    #[test]
    fn a_bare_name_finds_a_program_in_any_searched_directory() {
        let first = tempfile::tempdir().expect("a temporary directory");
        let second = tempfile::tempdir().expect("a temporary directory");
        let expected = program(second.path(), "widget");

        let search = Search::over(&[first.path(), second.path()]);

        assert_eq!(search.locate("widget"), Some(expected));
        assert_eq!(search.locate("nothing-of-the-sort"), None);
    }

    /// Directories are tried in the order given, because that is what makes
    /// "earlier on `PATH` wins" true.
    #[test]
    fn the_first_directory_holding_the_program_wins() {
        let first = tempfile::tempdir().expect("a temporary directory");
        let second = tempfile::tempdir().expect("a temporary directory");
        let winner = program(first.path(), "widget");
        program(second.path(), "widget");

        assert_eq!(
            Search::over(&[first.path(), second.path()]).locate("widget"),
            Some(winner)
        );
    }

    /// A directory is not a program. This is the case the provider's
    /// "configured, but not something that can be started" diagnostic rests on,
    /// so it is pinned here rather than only through that.
    #[test]
    fn a_directory_is_never_startable() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let inside = directory.path().join("claude");
        std::fs::create_dir(&inside).expect("creates the directory");

        assert!(!Search::over(&[]).is_executable(&inside));
        assert_eq!(Search::over(&[directory.path()]).locate("claude"), None);
    }

    /// What "not executable" means for a file that does exist. The two
    /// platforms disagree about the mechanism and the test says so rather than
    /// asserting one of them everywhere.
    #[test]
    fn a_file_that_is_not_a_program_is_not_startable() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        // On Windows the extension is what decides, so a name with none that
        // `PATHEXT` knows is not startable however it is spelled. On Unix the
        // same file is unstartable because nothing set its mode bits.
        let notes = directory
            .path()
            .join(if cfg!(windows) { "claude.txt" } else { "claude" });
        std::fs::write(&notes, "not a program").expect("writes the file");

        assert!(!Search::over(&[]).is_executable(&notes));
    }

    /// The suffix goes on the end rather than replacing what is there, so a
    /// command whose name already contains a dot is still found.
    #[test]
    fn an_extension_is_appended_rather_than_substituted() {
        if !cfg!(windows) {
            eprintln!("skipped: PATHEXT is a Windows idea");
            return;
        }

        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("code.insiders.cmd");
        std::fs::write(&path, "").expect("writes the file");

        assert_eq!(
            Search::over(&[directory.path()]).locate("code.insiders"),
            Some(path)
        );
    }

    /// A path that already names the file exactly needs no suffix.
    #[test]
    fn a_path_that_is_already_a_program_is_returned_unchanged() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = program(directory.path(), "widget");

        assert_eq!(
            Search::over(&[]).startable(&path),
            Some(path.clone()),
            "{}",
            path.display()
        );
    }

    /// Windows lets a command be named without its suffix, and the shell fills
    /// it in. So does this.
    #[test]
    fn a_path_missing_its_suffix_is_completed() {
        if !cfg!(windows) {
            eprintln!("skipped: PATHEXT is a Windows idea");
            return;
        }

        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = program(directory.path(), "widget");

        assert_eq!(
            Search::over(&[]).startable(&directory.path().join("widget")),
            Some(path)
        );
    }

    /// A machine with no `PATH` at all finds nothing rather than panicking, and
    /// says so through an empty directory list rather than by looking like a
    /// search that succeeded.
    #[test]
    fn a_search_over_nothing_finds_nothing() {
        let search = Search::over(&[]);
        assert!(search.directories().is_empty());
        assert_eq!(search.locate("claude"), None);
    }
}
