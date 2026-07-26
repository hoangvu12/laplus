//! What a project is, and what makes a folder eligible to be one.
//!
//! Pure in the same sense [`crate::config`] is: types and the rules over them,
//! with no database and no wire. [`crate::store`] persists these, and
//! [`crate::orchestration`] is what puts them on the socket. The one thing here
//! that touches the world is [`WorkspaceRoot::check`], which has to — "is this
//! folder usable" is a question only the filesystem can answer.
//!
//! The shape is hand-written from `OrchestrationProjectShell` in
//! `t3code/packages/contracts/src/orchestration.ts` and validated against the
//! `project-upserted` event captured in
//! `fixtures/socket-wire/05-orchestration-and-backpressure.ndjson`. Three
//! fields are constants rather than data, and each is a later ticket's:
//!
//! | Field | Filled by |
//! |---|---|
//! | `repositoryIdentity` | tickets 19–21 — git, which is where a repository's canonical identity comes from |
//! | `defaultModelSelection` | ticket 09, once model slugs are known |
//! | `scripts` | not in v1's scope at all; the contract requires the key |

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// A registered project.
///
/// [`Project::canonical_root`] is the odd one out: it is the only field that
/// never reaches the client. It exists because "the same folder" and "the same
/// string" are different questions — see [`WorkspaceRoot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    pub title: String,
    /// The absolute path, as the UI displays it and as the agent will use it
    /// for a working directory.
    pub workspace_root: String,
    /// The same folder reduced to a form two spellings of it share. Storage
    /// and duplicate detection only.
    pub canonical_root: String,
    pub created_at: String,
    pub updated_at: String,
}

impl Project {
    /// The `OrchestrationProjectShell` the client decodes.
    ///
    /// Built by hand rather than derived, because half of it is constants the
    /// contract requires and a `Serialize` impl would hide that.
    pub fn to_value(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "workspaceRoot": self.workspace_root,
            "repositoryIdentity": Value::Null,
            "defaultModelSelection": Value::Null,
            "scripts": [],
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        })
    }
}

/// A folder that has been checked and may be registered.
///
/// Constructing one is the check — there is no way to hold a `WorkspaceRoot`
/// for a path that was not, at that moment, an existing readable directory.
/// That is the whole reason it is a type rather than a validation function
/// returning a string.
///
/// It carries the folder under two names because a project registry needs
/// both. [`WorkspaceRoot::display`] is the path the user typed, made absolute
/// and nothing more — what the UI shows and what a shell will `cd` to.
/// [`WorkspaceRoot::canonical`] is the same folder with symlinks resolved and,
/// on Windows, case folded; it is what makes "adding the same folder twice"
/// answerable when the two spellings differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRoot {
    display: String,
    canonical: String,
}

impl WorkspaceRoot {
    /// Check a path from a client and, if it will serve, accept it.
    ///
    /// The order of the checks is the order of the answers a user can act on:
    /// missing before not-a-directory before not-readable. Readability is
    /// proved by actually opening the directory rather than by reading a
    /// permission bit, because on Windows the bit does not mean what a
    /// POSIX-shaped guess would assume.
    pub fn check(raw: &str) -> Result<WorkspaceRoot, Rejection> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(Rejection::Blank);
        }

        let requested = expand_home(trimmed);
        let absolute = std::path::absolute(&requested)
            .map_err(|error| Rejection::unusable(&requested, &error))?;
        let display = absolute.to_string_lossy().into_owned();

        match std::fs::metadata(&absolute) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(Rejection::NotADirectory(display))
            }
            Ok(_) => {}
            Err(error) => return Err(Rejection::from_io(display, &error)),
        }

        // Statting a directory says it is there; listing it says the server can
        // use it. They differ exactly in the case this check exists for.
        if let Err(error) = std::fs::read_dir(&absolute) {
            return Err(Rejection::from_io(display, &error));
        }

        Ok(WorkspaceRoot {
            canonical: canonicalize(&absolute).unwrap_or_else(|| fold_case(&display)),
            display,
        })
    }

    pub fn display(&self) -> &str {
        &self.display
    }

    pub fn canonical(&self) -> &str {
        &self.canonical
    }

    /// The name to show when the client did not supply one.
    ///
    /// The upstream UI infers a title from the path before it dispatches, so
    /// this is the fallback for a client that does not — and for the folder
    /// with no final component, a drive root, where the drive itself is the
    /// most useful thing left to call it.
    pub fn inferred_title(&self) -> String {
        Path::new(&self.display)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| self.display.clone())
    }
}

/// Why a folder cannot be a project.
///
/// Every variant renders to a message naming both the problem and the path.
/// That is not a nicety: `OrchestrationDispatchCommandError` carries a message
/// and nothing else machine-readable, so the string *is* the whole diagnostic
/// the user gets — see [`crate::orchestration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// No path at all. A client-side bug rather than a user's mistake, but it
    /// arrives over the same wire as one.
    Blank,
    Missing(String),
    NotADirectory(String),
    NotReadable(String),
    /// Something else the filesystem refused — a bad drive, a name the OS will
    /// not accept, a device that is not there. Rare, and the detail is the only
    /// useful thing left to report.
    Unusable { path: String, detail: String },
}

impl Rejection {
    /// What an `io::Error` about a folder means, said once.
    ///
    /// `pub(crate)` for [`crate::filesystem`], which meets the same folder
    /// going wrong in the same ways one step later — the walk can find a
    /// workspace root gone that [`WorkspaceRoot::check`] had just opened.
    pub(crate) fn from_io(path: String, error: &std::io::Error) -> Rejection {
        match error.kind() {
            std::io::ErrorKind::NotFound => Rejection::Missing(path),
            std::io::ErrorKind::PermissionDenied => Rejection::NotReadable(path),
            _ => Rejection::Unusable {
                path,
                detail: error.to_string(),
            },
        }
    }

    fn unusable(path: &Path, error: &std::io::Error) -> Rejection {
        Rejection::Unusable {
            path: path.to_string_lossy().into_owned(),
            detail: error.to_string(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            Rejection::Blank => "A project needs a workspace root; none was given.".to_string(),
            Rejection::Missing(path) => format!("Workspace root does not exist: {path}"),
            Rejection::NotADirectory(path) => format!("Workspace root is not a directory: {path}"),
            Rejection::NotReadable(path) => format!("Workspace root is not readable: {path}"),
            Rejection::Unusable { path, detail } => {
                format!("Workspace root cannot be used: {path} ({detail})")
            }
        }
    }
}

/// `~` and `~/…`, which a user typing a path expects to work and which the
/// upstream server expands too (`expandHomePath` in its `WorkspacePaths`).
///
/// Shared with [`crate::filesystem`], which meets the same paths one step
/// earlier — the folder picker is where a user types `~/` in the first place,
/// and a picker and a registry that disagreed about what it meant would offer a
/// folder that could not then be added.
pub(crate) fn expand_home(path: &str) -> PathBuf {
    expand_tilde(path, home_dir())
}

/// The rule, with the machine's home passed in rather than looked up.
///
/// Split out so both branches are reachable from a test: a machine with no home
/// directory is rare but not impossible, and "leave the path alone" is the
/// behaviour that would otherwise only ever run there.
///
/// Anything that is not `~` or `~/…` is left exactly as given — `~someone` is
/// another user's home on Unix and this server has no business guessing where
/// that is.
fn expand_tilde(path: &str, home: Option<PathBuf>) -> PathBuf {
    let rest = match path.strip_prefix('~') {
        Some("") => "",
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => &rest[1..],
        _ => return PathBuf::from(path),
    };

    match home {
        Some(home) if rest.is_empty() => home,
        Some(home) => home.join(rest),
        None => PathBuf::from(path),
    }
}

fn home_dir() -> Option<PathBuf> {
    for variable in ["USERPROFILE", "HOME"] {
        if let Ok(value) = std::env::var(variable) {
            if !value.trim().is_empty() {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

/// The folder's identity, as far as duplicate detection is concerned.
///
/// `canonicalize` resolves symlinks and, on Windows, returns the path in the
/// casing the filesystem actually holds — which is why the case folding below
/// is a second line rather than the first. It is kept because a Windows volume
/// *can* be mounted case-sensitively, and answering "is this the same folder"
/// wrongly there would let the same directory be registered twice.
fn canonicalize(path: &Path) -> Option<String> {
    let canonical = std::fs::canonicalize(path).ok()?;
    Some(fold_case(&canonical.to_string_lossy()))
}

fn fold_case(path: &str) -> String {
    if cfg!(windows) {
        path.to_lowercase()
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            id: "6ee34f01-3d27-4719-8254-2e9c255e5586".to_string(),
            title: "wire-capture".to_string(),
            workspace_root: r"C:\Users\ADMIN\AppData\Local\Temp\wire-capture".to_string(),
            canonical_root: r"c:\users\admin\appdata\local\temp\wire-capture".to_string(),
            created_at: "2026-07-26T00:23:04.909Z".to_string(),
            updated_at: "2026-07-26T00:23:04.909Z".to_string(),
        }
    }

    /// Field for field against the `project-upserted` payload in
    /// `fixtures/socket-wire/05-orchestration-and-backpressure.ndjson`. The
    /// three constants are the point: the contract requires the keys, and a
    /// client decoding `OrchestrationProjectShell` fails the whole snapshot if
    /// one is missing.
    #[test]
    fn a_project_serializes_to_the_captured_shell_shape() {
        assert_eq!(
            project().to_value(),
            json!({
                "id": "6ee34f01-3d27-4719-8254-2e9c255e5586",
                "title": "wire-capture",
                "workspaceRoot": r"C:\Users\ADMIN\AppData\Local\Temp\wire-capture",
                "repositoryIdentity": Value::Null,
                "defaultModelSelection": Value::Null,
                "scripts": [],
                "createdAt": "2026-07-26T00:23:04.909Z",
                "updatedAt": "2026-07-26T00:23:04.909Z",
            })
        );
    }

    /// The key that never leaves the server must not leak into the payload —
    /// on Windows it is case-folded, so a client that showed it would show the
    /// user a path that is not the one they typed.
    #[test]
    fn the_canonical_root_is_not_on_the_wire() {
        let value = project().to_value();
        assert!(value.get("canonicalRoot").is_none());
        assert!(value.get("canonical_root").is_none());
    }

    #[test]
    fn an_existing_directory_is_accepted() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let root = WorkspaceRoot::check(&directory.path().to_string_lossy())
            .expect("an existing directory is a usable workspace root");

        assert_eq!(root.display(), directory.path().to_string_lossy());
        assert!(!root.canonical().is_empty());
    }

    /// Each rejection has to name the problem *and* the path, because the
    /// message is the entire diagnostic the user is given.
    #[test]
    fn each_rejection_names_the_problem_and_the_path() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        let missing = directory.path().join("not-there");
        let rejection = WorkspaceRoot::check(&missing.to_string_lossy())
            .expect_err("a path that is not there cannot be a project");
        assert_eq!(
            rejection,
            Rejection::Missing(missing.to_string_lossy().into_owned())
        );
        assert!(rejection.message().contains("does not exist"));
        assert!(rejection.message().contains(&*missing.to_string_lossy()));

        let file = directory.path().join("a-file.txt");
        std::fs::write(&file, "not a directory").expect("writes the file");
        let rejection = WorkspaceRoot::check(&file.to_string_lossy())
            .expect_err("a file cannot be a project");
        assert_eq!(
            rejection,
            Rejection::NotADirectory(file.to_string_lossy().into_owned())
        );
        assert!(rejection.message().contains("is not a directory"));
        assert!(rejection.message().contains(&*file.to_string_lossy()));

        let blank = WorkspaceRoot::check("   ").expect_err("a blank path cannot be a project");
        assert_eq!(blank, Rejection::Blank);
        assert!(blank.message().contains("workspace root"));
    }

    /// The third of the ticket's bad paths, reached directly.
    ///
    /// Making a genuinely unreadable directory means `icacls` on Windows and
    /// `chmod` elsewhere, and neither is available to a test that has to pass
    /// on a developer's machine and in CI. What is actually worth pinning is
    /// the mapping — that a refused open becomes "not readable" and names the
    /// path, rather than being swept into the catch-all with a raw OS message.
    #[test]
    fn a_folder_the_server_may_not_open_is_reported_as_unreadable() {
        let denied = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let rejection = Rejection::from_io(r"C:\locked".to_string(), &denied);

        assert_eq!(rejection, Rejection::NotReadable(r"C:\locked".to_string()));
        assert!(rejection.message().contains("is not readable"));
        assert!(rejection.message().contains(r"C:\locked"));

        // Anything else keeps its own detail rather than being mislabelled.
        let broken = std::io::Error::from(std::io::ErrorKind::NotADirectory);
        assert!(matches!(
            Rejection::from_io(r"C:\odd".to_string(), &broken),
            Rejection::Unusable { .. }
        ));
    }

    /// A relative path is resolved rather than refused: the upstream client
    /// sends one for a project relative to the server's own directory, and the
    /// reference server resolves it the same way.
    #[test]
    fn a_relative_path_is_made_absolute() {
        let here = std::env::current_dir().expect("a current directory");
        let root = WorkspaceRoot::check(".").expect("the current directory exists");

        assert!(
            Path::new(root.display()).is_absolute(),
            "{} is not absolute",
            root.display()
        );
        assert_eq!(
            root.canonical(),
            canonicalize(&here).expect("the current directory canonicalizes")
        );
    }

    /// Two spellings of one folder have to agree on the canonical form, or
    /// "adding the same folder twice" would depend on how it was typed.
    #[test]
    fn two_spellings_of_one_folder_share_a_canonical_root() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let nested = directory.path().join("inner");
        std::fs::create_dir(&nested).expect("creates the nested directory");

        let direct = WorkspaceRoot::check(&nested.to_string_lossy()).expect("accepted");
        let roundabout = WorkspaceRoot::check(
            &directory.path().join("inner").join("..").join("inner").to_string_lossy(),
        )
        .expect("accepted");

        assert_eq!(direct.canonical(), roundabout.canonical());
    }

    #[test]
    fn a_title_is_inferred_from_the_final_path_component() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let named = directory.path().join("my-project");
        std::fs::create_dir(&named).expect("creates the directory");

        let root = WorkspaceRoot::check(&named.to_string_lossy()).expect("accepted");
        assert_eq!(root.inferred_title(), "my-project");
    }

    /// `~` is expanded, a path that merely starts with a tilde is not, and a
    /// machine with no home directory leaves the path alone rather than
    /// inventing one.
    #[test]
    fn a_leading_tilde_expands_to_the_home_directory() {
        let home = PathBuf::from(r"C:\Users\someone");
        let expand = |path: &str| expand_tilde(path, Some(home.clone()));

        assert_eq!(expand("~"), home);
        assert_eq!(expand("~/projects"), home.join("projects"));
        assert_eq!(expand(r"~\projects"), home.join("projects"));

        assert_eq!(expand("~someone/projects"), PathBuf::from("~someone/projects"));
        assert_eq!(expand("/tmp/plain"), PathBuf::from("/tmp/plain"));

        assert_eq!(expand_tilde("~/projects", None), PathBuf::from("~/projects"));
    }
}
