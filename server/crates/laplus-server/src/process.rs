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
use std::process::Command;

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
