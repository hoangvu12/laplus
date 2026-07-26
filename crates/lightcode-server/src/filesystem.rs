//! Reading the disk: the folder picker, and the file tree.
//!
//! Two method tags land here, and their namespaces are upstream's rather than a
//! boundary — `filesystem.browse` and `projects.listEntries` are the same
//! question asked at two scales:
//!
//! - **browse** is one directory, directories only, filtered by a prefix. It is
//!   what the command palette drives while a user types a path into "add
//!   project", so it is called once per keystroke and must stay small.
//! - **listEntries** is a whole workspace, files and directories, in one
//!   answer. It is what the file tree renders from.
//!
//! ## The file tree is not fetched a directory at a time
//!
//! Ticket 06 asks for directories that "load their contents when expanded", and
//! the honest report is that the UI does not offer the server that shape.
//! `ProjectListEntriesInput` is `{ cwd }` and nothing else — no directory, no
//! cursor, no depth — and `FileBrowserPanel.tsx` calls it exactly once per
//! project, with the workspace root, then hands the whole flat array to
//! `@pierre/trees` as `paths`. Laziness is real but it lives in the client: the
//! tree opens one level (`initialExpansion: 1`) and materialises rows as they
//! are revealed.
//!
//! So the server's obligation is not incremental fetching, which no client
//! would call. It is that the one listing is **bounded** ([`MAX_ENTRIES`], with
//! `truncated` telling the UI to say "partial") and that producing it does not
//! stall anything else — see [`crate::rpc::Deferred`], which exists because
//! this is the first method in the build that has to wait on the world.
//!
//! The lazily-expanding half of the ticket is [`Browse`], which genuinely does
//! read one directory per request and never walks a tree.
//!
//! ## Scope, and why there is no path confinement here
//!
//! Neither method restricts where it may look, and that is deliberate rather
//! than overlooked: the folder picker's whole purpose is to walk a filesystem
//! the server has no project for yet, so a confinement rule would have to admit
//! every path anyway. Reachability is the boundary — the socket is bound to
//! loopback (see [`crate::server`]) — and both methods only ever *read*.
//! Ticket 07's `projects.readFile` and `projects.writeFile` are a different
//! case and do confine themselves to a workspace root.
//!
//! Shapes are hand-written from `FilesystemBrowseResult` and
//! `ProjectListEntriesResult` in `t3code/packages/contracts/src/filesystem.ts`
//! and `project.ts`, and the error shapes from the `ProjectReadFileError`
//! captured in `fixtures/socket-wire/03-typed-error.ndjson` — the one typed
//! error of this family that was recorded from the reference server.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::projects::{expand_home, Rejection, WorkspaceRoot};

/// One directory, for the folder picker.
pub const BROWSE: &str = "filesystem.browse";

/// A whole workspace, for the file tree.
pub const LIST_ENTRIES: &str = "projects.listEntries";

/// The most entries one listing will carry.
///
/// Upstream's number, kept deliberately: the UI renders `truncated` as a
/// "· partial" badge next to the file count, and a client and server that
/// disagree about when a repository is too big would put that badge on
/// different projects on different days.
///
/// It is also the only bound the walk needs. A filesystem that cycles cannot
/// make it run forever — it can only make it fill up — so cycle handling is
/// about the *quality* of the answer rather than about termination.
pub const MAX_ENTRIES: usize = 25_000;

/// The directory the walk never descends into.
///
/// Not an ignore-file implementation and not the start of one — lightcode has
/// no `.gitignore` semantics, and
/// `.scratch/rust-server-tauri/issues/06-filesystem-browse-file-tree.md` says
/// what that costs. This single name is here because it is the one directory
/// that is present in every repository, is machine state rather than source,
/// and is large enough on its own to spend the whole of [`MAX_ENTRIES`] on
/// loose objects. Upstream's indexer does not surface it either, so a tree that
/// showed it would be a visible difference from the UI's own expectations.
const NEVER_WALKED: &str = ".git";

// ---------------------------------------------------------------------------
// filesystem.browse
// ---------------------------------------------------------------------------

/// A validated `filesystem.browse` call, ready to be run off the read loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse {
    partial_path: String,
    /// The project the user is browsing from, when there is one. Only an
    /// explicitly relative path needs it.
    cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowsePayload {
    partial_path: String,
    #[serde(default)]
    cwd: Option<String>,
}

impl Browse {
    /// Read the payload, or refuse with the error the method declares.
    pub fn read(payload: &Value) -> Result<Browse, Value> {
        let read: BrowsePayload = serde_json::from_value(payload.clone())
            .map_err(|error| malformed_browse(&format!("filesystem.browse is malformed: {error}")))?;

        let partial_path = read.partial_path.trim().to_string();
        if partial_path.is_empty() {
            return Err(malformed_browse("A browse needs a path; none was given."));
        }

        Ok(Browse {
            partial_path,
            // The contract types `cwd` as a trimmed non-empty string, so a
            // blank one is the same as none — and treating it as a value would
            // turn "no project open" into a browse of the process's own
            // directory.
            cwd: read.cwd.map(|cwd| cwd.trim().to_string()).filter(|cwd| !cwd.is_empty()),
        })
    }

    /// Do the work. Blocking, and called from a blocking task.
    pub fn run(self) -> Result<Value, Value> {
        self.target()
            .and_then(Target::read)
            .map(|browsed| browsed.to_value())
            .map_err(|rejection| self.to_error(&rejection))
    }

    /// Where to look and what to keep, from the fragment the user has typed.
    ///
    /// Pure: it is arithmetic on the string, and nothing here touches the disk.
    /// That matters because this is where every one of the picker's behaviours
    /// is decided — a trailing separator means "list this directory", anything
    /// else means "complete this name" — and it is the part worth testing
    /// without a filesystem.
    fn target(&self) -> Result<Target, BrowseRejection> {
        // Only reachable off Windows, which v1 does not ship; the contract
        // declares the failure and a developer running the suite on a Mac is
        // the person who meets it.
        if !cfg!(windows) && is_windows_absolute(&self.partial_path) {
            return Err(BrowseRejection::WindowsPathUnsupported);
        }

        let resolved = if is_explicitly_relative(&self.partial_path) {
            let cwd = self.cwd.as_deref().ok_or(BrowseRejection::CurrentProjectRequired)?;
            absolute(&expand_home(cwd).join(&self.partial_path))
        } else {
            absolute(&expand_home(&self.partial_path))
        }
        .map_err(|(path, detail)| BrowseRejection::ReadDirectoryFailed {
            parent_path: path,
            detail,
        })?;

        // A trailing separator is the user saying "inside here", and a bare `~`
        // is the same thing said about home. Anything else names a directory
        // only partly typed, so its last component is a filter rather than a
        // place.
        let listing_a_directory =
            ends_with_separator(&self.partial_path) || self.partial_path == "~";

        let (parent, prefix) = match (listing_a_directory, resolved.file_name()) {
            (false, Some(name)) => (
                resolved
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| resolved.clone()),
                name.to_string_lossy().into_owned(),
            ),
            // Either the user asked for a directory, or the path is a root and
            // has no final component to complete — `C:\`, `/`. Both list.
            _ => (resolved, String::new()),
        };

        Ok(Target {
            // Matching is case-insensitive because the picker is: a user typing
            // `doc` expects `Documents`, and on Windows that is the same name.
            prefix: prefix.to_lowercase(),
            // A partly-typed name hides dotted directories unless the user has
            // started typing the dot — which is how `.config` stays out of the
            // way without becoming unreachable.
            show_hidden: prefix.is_empty() || prefix.starts_with('.'),
            parent,
        })
    }

    /// The typed error, with the request echoed back into it the way the
    /// reference server does — see the `ProjectReadFileError` in
    /// `fixtures/socket-wire/03-typed-error.ndjson`, which carries its `cwd` and
    /// `relativePath` alongside the diagnosis.
    fn to_error(&self, rejection: &BrowseRejection) -> Value {
        let mut error = json!({
            "_tag": "FilesystemBrowseError",
            "partialPath": self.partial_path,
            "failure": rejection.failure(),
            "message": rejection.message(&self.partial_path),
        });

        if let Some(cwd) = &self.cwd {
            error["cwd"] = json!(cwd);
        }
        match rejection {
            BrowseRejection::WindowsPathUnsupported => error["platform"] = json!(std::env::consts::OS),
            BrowseRejection::ReadDirectoryFailed { parent_path, .. } => {
                error["parentPath"] = json!(parent_path)
            }
            BrowseRejection::CurrentProjectRequired => {}
        }
        error
    }
}

/// Where a browse will look, once the fragment has been read.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    parent: PathBuf,
    /// Lower-cased. Empty means "everything in this directory".
    prefix: String,
    show_hidden: bool,
}

impl Target {
    fn read(self) -> Result<Browsed, BrowseRejection> {
        let parent_path = self.parent.to_string_lossy().into_owned();

        let directory = match std::fs::read_dir(&self.parent) {
            Ok(directory) => directory,
            // A folder the process may not open is not a failed browse. The
            // picker shows it empty and the user goes elsewhere; an error would
            // put a red message under the input for the ordinary act of typing
            // past `C:\System Volume Information`.
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(Browsed {
                    parent_path,
                    entries: Vec::new(),
                })
            }
            Err(error) => {
                return Err(BrowseRejection::ReadDirectoryFailed {
                    parent_path,
                    detail: error.to_string(),
                })
            }
        };

        let mut entries = Vec::new();
        // `flatten` drops entries the directory could not describe. One
        // unreadable name is not a reason to refuse the other fifty, and the
        // picker has nowhere to say "and one more that I could not stat".
        for entry in directory.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !self.wants(&name) || !leads_to_a_directory(&entry) {
                continue;
            }
            entries.push(Folder {
                full_path: entry.path().to_string_lossy().into_owned(),
                name,
            });
        }

        entries.sort_by(|left, right| by_name(&left.name, &right.name));
        Ok(Browsed {
            parent_path,
            entries,
        })
    }

    fn wants(&self, name: &str) -> bool {
        (self.show_hidden || !name.starts_with('.'))
            && name.to_lowercase().starts_with(&self.prefix)
    }
}

/// One directory the picker will offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub name: String,
    pub full_path: String,
}

/// The `FilesystemBrowseResult` the client decodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browsed {
    pub parent_path: String,
    pub entries: Vec<Folder>,
}

impl Browsed {
    pub fn to_value(&self) -> Value {
        json!({
            "parentPath": self.parent_path,
            "entries": self
                .entries
                .iter()
                .map(|folder| json!({"name": folder.name, "fullPath": folder.full_path}))
                .collect::<Vec<Value>>(),
        })
    }
}

/// Why a browse produced nothing.
///
/// The three variants are the contract's three `FilesystemBrowseFailure`
/// literals and nothing more — the client switches on that string, so inventing
/// a fourth would arrive as an undecodable error rather than as extra detail.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BrowseRejection {
    /// A Windows path on a server that is not on Windows.
    WindowsPathUnsupported,
    /// `./src` with no project open. There is nothing for the path to be
    /// relative *to*.
    CurrentProjectRequired,
    ReadDirectoryFailed { parent_path: String, detail: String },
}

impl BrowseRejection {
    fn failure(&self) -> &'static str {
        match self {
            BrowseRejection::WindowsPathUnsupported => "windows_path_unsupported",
            BrowseRejection::CurrentProjectRequired => "current_project_required",
            BrowseRejection::ReadDirectoryFailed { .. } => "read_directory_failed",
        }
    }

    fn message(&self, partial_path: &str) -> String {
        match self {
            BrowseRejection::WindowsPathUnsupported => format!(
                "Windows-style path '{partial_path}' cannot be browsed on {}.",
                std::env::consts::OS
            ),
            BrowseRejection::CurrentProjectRequired => format!(
                "A project must be open to browse the relative path '{partial_path}'."
            ),
            BrowseRejection::ReadDirectoryFailed {
                parent_path,
                detail,
            } => format!("Cannot read the directory '{parent_path}' ({detail})."),
        }
    }
}

/// A payload that did not decode. `failure` is deliberately absent: the
/// contract's three literals all describe a path that was read and refused, and
/// none of them describes a request that never named a path at all. The field
/// is optional on the wire, so the client decodes the error and shows the
/// message.
fn malformed_browse(message: &str) -> Value {
    json!({"_tag": "FilesystemBrowseError", "message": message})
}

// ---------------------------------------------------------------------------
// projects.listEntries
// ---------------------------------------------------------------------------

/// A validated `projects.listEntries` call, ready to be run off the read loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntries {
    cwd: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListEntriesPayload {
    cwd: String,
}

impl ListEntries {
    pub fn read(payload: &Value) -> Result<ListEntries, Value> {
        let read: ListEntriesPayload = serde_json::from_value(payload.clone()).map_err(|error| {
            malformed_listing(&format!("projects.listEntries is malformed: {error}"))
        })?;

        let cwd = read.cwd.trim().to_string();
        if cwd.is_empty() {
            return Err(malformed_listing(
                "A listing needs a workspace root; none was given.",
            ));
        }

        Ok(ListEntries { cwd })
    }

    /// Do the work. Blocking, and called from a blocking task — a cold
    /// repository of twenty thousand files is seconds of disk, not microseconds
    /// of memory.
    pub fn run(self) -> Result<Value, Value> {
        match list(&self.cwd, MAX_ENTRIES) {
            Ok(listing) => Ok(listing.to_value()),
            Err(rejection) => Err(self.to_error(&rejection)),
        }
    }

    /// The `ProjectListEntriesError` the client decodes.
    ///
    /// The sentence is [`Rejection::message`]'s, not a second one written here.
    /// "This folder is not there" is the same fact whether the user was adding
    /// a project or opening one, and two modules that phrased it differently
    /// would show the user two different errors for one problem.
    ///
    /// What *is* decided here is the `failure` literal, because that is about
    /// this method's contract rather than about the folder.
    fn to_error(&self, rejection: &Rejection) -> Value {
        let mut error = json!({
            "_tag": "ProjectListEntriesError",
            "cwd": self.cwd,
            "failure": listing_failure(rejection),
            "message": rejection.message(),
        });

        match rejection {
            Rejection::Missing(path)
            | Rejection::NotADirectory(path)
            | Rejection::NotReadable(path) => error["normalizedCwd"] = json!(path),
            Rejection::Unusable { path, detail } => {
                error["normalizedCwd"] = json!(path);
                error["detail"] = json!(detail);
            }
            // Refused by `read` before a path was ever resolved, so there is no
            // normalised form of it to report.
            Rejection::Blank => {}
        }
        error
    }
}

/// One of the contract's `ProjectEntriesFailure` literals.
///
/// The three that name a search index are upstream's own — lightcode has no
/// index, and a failure the server can never produce is not worth a branch that
/// can never run.
fn listing_failure(rejection: &Rejection) -> &'static str {
    match rejection {
        Rejection::Blank | Rejection::Missing(_) => "workspace_root_not_found",
        Rejection::NotADirectory(_) => "workspace_root_not_directory",
        Rejection::NotReadable(_) | Rejection::Unusable { .. } => "workspace_root_stat_failed",
    }
}

/// A payload that did not name a workspace root. `failure` and `normalizedCwd`
/// are optional on the wire — the contract keeps them optional so a newer
/// client can decode an older server's message-only failure — so a request that
/// never got as far as a path can leave both out and still arrive as a failed
/// call rather than a broken connection.
fn malformed_listing(message: &str) -> Value {
    json!({"_tag": "ProjectListEntriesError", "message": message})
}

/// What a workspace holds, as the file tree reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listing {
    entries: Vec<Entry>,
    /// The workspace held more than the limit. The UI renders this as
    /// "· partial" beside the file count.
    truncated: bool,
}

impl Listing {
    fn to_value(&self) -> Value {
        json!({
            "entries": self.entries.iter().map(Entry::to_value).collect::<Vec<Value>>(),
            "truncated": self.truncated,
        })
    }
}

/// One file or directory, named relative to the workspace root.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// Forward slashes, no leading separator, no trailing one — the form
    /// upstream's indexer normalises to, and the form the tree splits on.
    path: String,
    kind: Kind,
}

impl Entry {
    fn to_value(&self) -> Value {
        json!({"path": self.path, "kind": self.kind.as_str()})
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::File => "file",
            Kind::Directory => "directory",
        }
    }
}

/// Walk `cwd` and describe everything under it, up to `limit` entries.
///
/// **Breadth-first, and that is the load-bearing choice.** A listing that stops
/// at the limit stops somewhere, and where it stops decides what the user sees:
/// depth-first would spend the whole budget inside the first directory and
/// leave the workspace root itself half-described, while breadth-first fills
/// the shallow levels completely and drops the deepest ones. It also means a
/// directory is always emitted before anything inside it, so a truncated
/// listing never contains a path whose parent is missing — which the tree would
/// otherwise have to invent.
///
/// `limit` is a parameter rather than [`MAX_ENTRIES`] read directly so that the
/// truncation behaviour can be tested against a handful of files instead of
/// twenty-five thousand. It is a real parameter and not a test seam — the
/// socket path passes [`MAX_ENTRIES`], and there is no other caller.
///
/// The precondition is [`WorkspaceRoot::check`]'s and not a second one written
/// here: "is this folder there, is it a folder, and will it open" is the same
/// question the registry asks, asked in the same order, answered in the same
/// words.
fn list(cwd: &str, limit: usize) -> Result<Listing, Rejection> {
    let root = PathBuf::from(WorkspaceRoot::check(cwd)?.display());
    let display = root.to_string_lossy().into_owned();

    let mut entries = Vec::new();
    let mut queue = VecDeque::from([(root.clone(), String::new())]);
    let mut unreadable = 0;
    let mut truncated = false;

    // Only the identities of directories reached *through a symlink* are kept.
    // A plain directory cannot contain itself, so canonicalising every one of
    // them would be a syscall per directory to answer a question only symlinks
    // can raise. Seeded with the root so a link pointing back at the workspace
    // is caught on its first hop.
    let mut visited: HashSet<PathBuf> = std::fs::canonicalize(&root).into_iter().collect();

    'walk: while let Some((directory, prefix)) = queue.pop_front() {
        let read = match std::fs::read_dir(&directory) {
            Ok(read) => read,
            // The root was opened once already by the check above, so this is a
            // folder that went away underneath the walk. There is nothing left
            // to list and no other answer to give.
            Err(error) if prefix.is_empty() => {
                return Err(Rejection::from_io(display, &error))
            }
            // Any other directory is reported in place: it keeps its entry in
            // the tree and loses only its children. `ProjectEntry` is a path
            // and a kind with nowhere to say "and this one refused", so the
            // count goes to the log instead of the wire — refusing the whole
            // workspace because one folder is locked would be much the worse
            // answer.
            Err(_) => {
                unreadable += 1;
                continue;
            }
        };

        for entry in read {
            // An entry the directory could not even name. It cannot be
            // reported in place, because there is no place — there is no name
            // to put in the tree — so the only honest thing left is to say how
            // many there were.
            let Ok(entry) = entry else {
                unreadable += 1;
                continue;
            };

            let name = entry.file_name().to_string_lossy().into_owned();
            if name == NEVER_WALKED {
                continue;
            }
            if entries.len() >= limit {
                truncated = true;
                break 'walk;
            }

            let path = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let kind = if leads_to_a_directory(&entry) {
                Kind::Directory
            } else {
                Kind::File
            };

            if kind == Kind::Directory && may_descend(&entry, &mut visited) {
                queue.push_back((entry.path(), path.clone()));
            }
            entries.push(Entry { path, kind });
        }
    }

    if unreadable > 0 {
        eprintln!(
            "lightcode: {unreadable} entr(ies) under {display} could not be read \
             and are listed with no contents, or not at all"
        );
    }

    entries.sort_by(|left, right| by_name(&left.path, &right.path));
    Ok(Listing { entries, truncated })
}

/// May the walk go inside this directory?
///
/// Yes for a real directory, and yes for a symlink whose target has not been
/// walked already. The set is what stops a link pointing at its own ancestor
/// from producing the same subtree at every depth until the limit runs out —
/// the walk would still *terminate* without it, but it would terminate having
/// spent the whole listing on one directory repeated.
fn may_descend(entry: &std::fs::DirEntry, visited: &mut HashSet<PathBuf>) -> bool {
    match entry.file_type() {
        Ok(kind) if !kind.is_symlink() => true,
        // A link whose target cannot be resolved is not followed. There is
        // nothing to enumerate and nothing to record.
        _ => std::fs::canonicalize(entry.path())
            .map(|target| visited.insert(target))
            .unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// Path arithmetic shared by both
// ---------------------------------------------------------------------------

/// Absolute and tidy, or the path and why it could not be made so.
///
/// Re-collecting the components is what removes the trailing separator the
/// picker's own "list this directory" spelling leaves behind. It matters
/// because the result is echoed back as `parentPath`, which the palette
/// compares against what the user typed (`resolvedAddProjectPath` in
/// `CommandPalette.tsx`) — and `C:\repo` and `C:\repo\` would read as two
/// different places.
fn absolute(path: &Path) -> Result<PathBuf, (String, String)> {
    std::path::absolute(path)
        .map(|absolute| absolute.components().collect())
        .map_err(|error| (path.to_string_lossy().into_owned(), error.to_string()))
}

/// Does this entry lead to a directory?
///
/// The type on a directory entry describes the *link*, not its target, so a
/// symlinked folder reads as neither file nor directory until it is followed.
/// Following costs one syscall and only for the entries that are links, which
/// is what the two-step here buys.
fn leads_to_a_directory(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_dir() => true,
        Ok(kind) if kind.is_symlink() => std::fs::metadata(entry.path())
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false),
        // A plain file, or an entry that would not describe itself. Neither is
        // somewhere to go.
        _ => false,
    }
}

/// `C:\…`, `\\server\share` — the two absolute forms Windows has.
fn is_windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes.get(2), None | Some(b'/') | Some(b'\\')))
}

/// `.`, `..`, `./x`, `..\x` — a path that says out loud it is relative.
///
/// A bare `src` is *not* one of these, and that is upstream's rule rather than
/// an oversight: the picker resolves an unqualified name against the process's
/// own directory, and only an explicit `./` asks for the open project.
fn is_explicitly_relative(path: &str) -> bool {
    matches!(path, "." | "..")
        || ["./", "../", ".\\", "..\\"]
            .iter()
            .any(|prefix| path.starts_with(prefix))
}

fn ends_with_separator(path: &str) -> bool {
    path.ends_with('/') || path.ends_with('\\')
}

/// Case-insensitive first, exact second.
///
/// The client sorts with `localeCompare`, which puts `Documents` next to
/// `desktop` rather than in a separate uppercase block. The exact comparison
/// behind it is only there so two names that differ in case alone have one
/// answer rather than an arbitrary one.
fn by_name(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree written out from a list of `path -> contents`. Directories are
    /// implied by the paths, and a path ending in `/` is an empty one.
    fn tree(paths: &[&str]) -> tempfile::TempDir {
        let directory = tempfile::tempdir().expect("a temporary directory");
        for path in paths {
            let full = directory.path().join(path.trim_end_matches('/'));
            if path.ends_with('/') {
                std::fs::create_dir_all(&full).expect("creates the directory");
            } else {
                std::fs::create_dir_all(full.parent().expect("a parent"))
                    .expect("creates the parents");
                std::fs::write(&full, "contents").expect("writes the file");
            }
        }
        directory
    }

    /// A path with the platform's separator on the end — the picker's "list
    /// this directory" form, spelled the way a user's keyboard would.
    fn inside(path: &Path) -> String {
        format!("{}{}", path.to_string_lossy(), std::path::MAIN_SEPARATOR)
    }

    /// One browse, through the same two calls `rpc::dispatch` makes and
    /// answering with the same payload the client would decode. Composing the
    /// steps again here would leave [`Browse::run`] itself untested.
    fn browse(partial_path: &str, cwd: Option<&str>) -> Result<Value, Value> {
        let mut payload = json!({"partialPath": partial_path});
        if let Some(cwd) = cwd {
            payload["cwd"] = json!(cwd);
        }
        Browse::read(&payload)?.run()
    }

    fn names(browsed: &Value) -> Vec<&str> {
        browsed["entries"]
            .as_array()
            .unwrap_or_else(|| panic!("an array of entries: {browsed}"))
            .iter()
            .map(|folder| folder["name"].as_str().expect("a name"))
            .collect()
    }

    fn paths(listing: &Listing) -> Vec<&str> {
        listing
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }

    /// Directories only, filtered by what has been typed so far. The files are
    /// in the fixture because the picker picks folders and a file that matched
    /// the prefix would otherwise be offered as one.
    #[test]
    fn browsing_a_partly_typed_name_offers_the_directories_that_start_with_it() {
        let directory = tree(&["alphabet.txt", "alpha/index.ts", "alpine/index.ts", "beta/"]);
        let typed = directory.path().join("alp");

        let browsed = browse(&typed.to_string_lossy(), None).expect("a readable directory");

        assert_eq!(
            browsed["parentPath"],
            json!(directory.path().to_string_lossy())
        );
        assert_eq!(names(&browsed), ["alpha", "alpine"]);
        assert_eq!(
            browsed["entries"][0]["fullPath"],
            json!(directory.path().join("alpha").to_string_lossy())
        );
    }

    /// A trailing separator means "inside here" rather than "complete this
    /// name" — the difference between having typed a directory and being part
    /// way through typing its neighbour.
    #[test]
    fn a_trailing_separator_lists_the_directory_rather_than_filtering_its_neighbours() {
        let directory = tree(&["alpha/nested/", "alpine/"]);
        let partial = directory.path().join("alpha");

        assert_eq!(
            names(&browse(&inside(&partial), None).expect("listed")),
            ["nested"]
        );
        assert_eq!(
            names(&browse(&partial.to_string_lossy(), None).expect("listed")),
            ["alpha"]
        );
    }

    /// Dotted directories stay out of the way until the user types the dot.
    #[test]
    fn dotted_directories_appear_when_a_directory_is_listed_or_the_dot_is_typed() {
        let directory = tree(&[".config/settings.json", "config/settings.json"]);
        let listed = inside(directory.path());

        assert_eq!(
            names(&browse(&listed, None).expect("listed")),
            [".config", "config"]
        );
        assert_eq!(
            names(&browse(&format!("{listed}.c"), None).expect("listed")),
            [".config"]
        );
        assert_eq!(
            names(&browse(&format!("{listed}c"), None).expect("listed")),
            ["config"]
        );
    }

    /// An explicitly relative path is the one form that needs the open project,
    /// and the refusal has to say so — there is nothing wrong with the path
    /// itself.
    #[test]
    fn an_explicitly_relative_path_needs_a_project_and_says_so_without_one() {
        let directory = tree(&["packages/pkg.json"]);
        let cwd = directory.path().to_string_lossy().into_owned();

        let browsed = browse("./pack", Some(&cwd)).expect("resolved against the project");
        assert_eq!(browsed["parentPath"], json!(cwd));
        assert_eq!(names(&browsed), ["packages"]);

        let error = browse("./pack", None).expect_err("nothing to be relative to");
        assert_eq!(error["_tag"], "FilesystemBrowseError");
        assert_eq!(error["failure"], "current_project_required");
        assert_eq!(error["partialPath"], "./pack");
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("./pack"));

        // A blank cwd is the same as none: the contract types it as a trimmed
        // non-empty string, and treating it as a value would silently browse
        // the server's own directory.
        assert_eq!(
            browse("./pack", Some("   ")).expect_err("a blank project")["failure"],
            "current_project_required"
        );
    }

    /// The picker walks a filesystem the user is still typing, so most of what
    /// it is asked for does not exist yet. That has to arrive as the method's
    /// own error, naming the directory it could not open.
    #[test]
    fn a_directory_that_cannot_be_read_is_refused_by_name() {
        let directory = tree(&["real/"]);
        let missing = directory.path().join("not-there").join("deeper");

        let error = browse(&missing.to_string_lossy(), None).expect_err("nothing there");
        assert_eq!(error["_tag"], "FilesystemBrowseError");
        assert_eq!(error["failure"], "read_directory_failed");
        assert_eq!(
            error["parentPath"],
            json!(directory.path().join("not-there").to_string_lossy())
        );
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("not-there"));
    }

    /// The two payloads that are not a browse at all. Both have to fail the
    /// call rather than the connection, so both carry the error's `_tag`.
    #[test]
    fn a_payload_without_a_path_is_refused_before_anything_is_read() {
        for payload in [json!({}), json!({"partialPath": "   "})] {
            let error = Browse::read(&payload).expect_err("not a browse");
            assert_eq!(error["_tag"], "FilesystemBrowseError");
            assert!(error["message"].is_string(), "{error}");
            assert!(
                error.get("failure").is_none(),
                "none of the three literals describes a request with no path: {error}"
            );
        }
    }

    /// Not reachable on the platform v1 ships, and pinned anyway: the check is
    /// the reason a developer running the suite on a Mac sees a diagnosis
    /// instead of an empty listing of the process's own directory.
    #[test]
    fn a_windows_path_is_recognised_as_absolute() {
        for path in [r"C:\Users", "c:/Users", "C:", r"\\server\share"] {
            assert!(is_windows_absolute(path), "{path}");
        }
        for path in ["/usr/local", "./src", "src", "~/projects", "CC:/x"] {
            assert!(!is_windows_absolute(path), "{path}");
        }
    }

    /// The whole workspace in one answer: every file, every directory, named
    /// relative to the root with forward slashes whatever the platform's
    /// separator is.
    #[test]
    fn a_listing_names_every_file_and_directory_relative_to_the_root() {
        let directory = tree(&["src/main.rs", "src/lib/util.rs", "README.md", "empty/"]);

        let listing = list(&directory.path().to_string_lossy(), MAX_ENTRIES).expect("listed");

        assert_eq!(
            paths(&listing),
            [
                "empty",
                "README.md",
                "src",
                "src/lib",
                "src/lib/util.rs",
                "src/main.rs",
            ]
        );
        assert!(!listing.truncated);

        let kinds: Vec<Kind> = listing.entries.iter().map(|entry| entry.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Directory,
                Kind::File,
                Kind::Directory,
                Kind::Directory,
                Kind::File,
                Kind::File,
            ]
        );
    }

    /// Spaces and non-ASCII names are ordinary. They are worth a test because
    /// every step between the directory entry and the JSON string is a place
    /// one could be mangled.
    #[test]
    fn names_with_spaces_and_non_ascii_characters_survive_the_listing() {
        let directory = tree(&[
            "my documents/notes.txt",
            "café/naïve.txt",
            "日本語/ファイル.txt",
        ]);

        let listing = list(&directory.path().to_string_lossy(), MAX_ENTRIES).expect("listed");
        let paths = paths(&listing);

        for expected in [
            "my documents",
            "my documents/notes.txt",
            "café",
            "café/naïve.txt",
            "日本語",
            "日本語/ファイル.txt",
        ] {
            assert!(paths.contains(&expected), "{expected} is missing: {paths:?}");
        }
    }

    /// The limit is a promise about size, and breadth-first is what makes the
    /// truncated part usable: everything shallow is there, and nothing is left
    /// pointing at a parent that is not.
    #[test]
    fn a_listing_past_the_limit_is_truncated_with_its_shallow_levels_intact() {
        let directory = tree(&["a/deep/deeper/leaf.txt", "b.txt", "c.txt"]);

        let listing = list(&directory.path().to_string_lossy(), 3).expect("listed");

        assert!(listing.truncated);
        assert_eq!(listing.entries.len(), 3);
        assert_eq!(paths(&listing), ["a", "b.txt", "c.txt"]);

        for entry in &listing.entries {
            if let Some((parent, _)) = entry.path.rsplit_once('/') {
                assert!(
                    paths(&listing).contains(&parent),
                    "{} is listed without its parent",
                    entry.path
                );
            }
        }
    }

    /// `.git` is machine state, it is in every repository, and on its own it is
    /// large enough to spend the whole limit. It is the only name the walk
    /// knows about.
    #[test]
    fn the_git_directory_is_not_listed() {
        let directory = tree(&[".git/objects/ab/cdef", ".gitignore", "src/main.rs"]);

        let listing = list(&directory.path().to_string_lossy(), MAX_ENTRIES).expect("listed");

        assert_eq!(paths(&listing), [".gitignore", "src", "src/main.rs"]);
    }

    /// An entry whose target the process cannot reach keeps its place in the
    /// listing, and the workspace around it is described in full.
    ///
    /// A dangling link is the one form of unreadable entry a test can make on
    /// any machine. A directory the *operating system* refuses to open takes
    /// `icacls` on Windows and `chmod` elsewhere and would not run the same way
    /// on a developer's machine and in CI, so that half is declared uncovered
    /// in the ticket — the same call as
    /// `projects::tests::a_folder_the_server_may_not_open_is_reported_as_unreadable`.
    #[test]
    fn an_entry_that_cannot_be_followed_keeps_its_place_in_the_listing() {
        let directory = tree(&["kept/file.txt"]);
        let dangling = directory.path().join("dangling");
        if !symlink_dir(&directory.path().join("never-existed"), &dangling) {
            eprintln!("skipped: this machine will not create directory symlinks");
            return;
        }

        let listing = list(&directory.path().to_string_lossy(), MAX_ENTRIES)
            .expect("one unreadable entry does not fail the workspace");

        assert_eq!(paths(&listing), ["dangling", "kept", "kept/file.txt"]);
        // Nothing resolves behind it, so the only honest kind left is the one
        // that promises the tree no children.
        assert_eq!(listing.entries[0].kind, Kind::File);
    }

    /// A workspace root that is missing, is a file, or is blank each fails with
    /// the literal the client switches on and a message naming the path.
    #[test]
    fn a_workspace_root_that_cannot_be_listed_is_refused_by_name() {
        let directory = tree(&["a-file.txt"]);

        let missing = directory.path().join("not-there");
        let error = ListEntries::read(&json!({"cwd": missing.to_string_lossy()}))
            .expect("a well-formed payload")
            .run()
            .expect_err("nothing there");
        assert_eq!(error["_tag"], "ProjectListEntriesError");
        assert_eq!(error["failure"], "workspace_root_not_found");
        assert_eq!(error["cwd"], json!(missing.to_string_lossy()));
        assert_eq!(error["normalizedCwd"], json!(missing.to_string_lossy()));
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("does not exist"));

        let file = directory.path().join("a-file.txt");
        let error = ListEntries::read(&json!({"cwd": file.to_string_lossy()}))
            .expect("a well-formed payload")
            .run()
            .expect_err("a file is not a workspace");
        assert_eq!(error["failure"], "workspace_root_not_directory");
        assert!(error["message"]
            .as_str()
            .expect("a message")
            .contains("is not a directory"));

        for payload in [json!({}), json!({"cwd": "  "})] {
            let error = ListEntries::read(&payload).expect_err("not a listing");
            assert_eq!(error["_tag"], "ProjectListEntriesError");
            assert!(error["message"].is_string(), "{error}");
        }
    }

    /// The listing the client actually decodes, key for key.
    #[test]
    fn a_listing_serializes_to_the_contract_shape() {
        let listing = Listing {
            entries: vec![
                Entry {
                    path: "src".to_string(),
                    kind: Kind::Directory,
                },
                Entry {
                    path: "src/main.rs".to_string(),
                    kind: Kind::File,
                },
            ],
            truncated: true,
        };

        assert_eq!(
            listing.to_value(),
            json!({
                "entries": [
                    {"path": "src", "kind": "directory"},
                    {"path": "src/main.rs", "kind": "file"},
                ],
                "truncated": true,
            })
        );
    }

    /// The rule for symlinked directories, both halves of it: a link to
    /// somewhere the walk has not been is followed, and a link to somewhere it
    /// has — including the workspace root itself — is listed but not entered.
    ///
    /// So a directory's contents appear once in the listing however many names
    /// lead to them, and a link pointing at its own ancestor is a leaf rather
    /// than a hall of mirrors. Termination is not what this buys: the entry
    /// limit already guarantees that. What it buys is a listing that spends
    /// that limit on distinct files.
    ///
    /// Skipped where the operating system will not create a directory symlink —
    /// Windows without Developer Mode, in practice. Announced rather than
    /// silent: a test that quietly passes because it did nothing is worse than
    /// no test.
    #[test]
    fn a_symlinked_directory_is_walked_once_and_a_cycle_is_not_walked_at_all() {
        let elsewhere = tree(&["shared/note.md"]);
        let directory = tree(&["real/leaf.txt"]);

        let outward = directory.path().join("outward");
        let loop_back = directory.path().join("real").join("loop");
        if !symlink_dir(&elsewhere.path().join("shared"), &outward)
            || !symlink_dir(directory.path(), &loop_back)
        {
            eprintln!(
                "skipped: this machine will not create directory symlinks, so the \
                 cycle guard is unexercised"
            );
            return;
        }

        let listing = list(&directory.path().to_string_lossy(), MAX_ENTRIES).expect("listed");
        let paths = paths(&listing);

        assert_eq!(
            paths,
            [
                "outward",
                "outward/note.md",
                "real",
                "real/leaf.txt",
                "real/loop",
            ],
            "{paths:?}"
        );
        // Both links are directories the tree may show; only one of them has
        // anything behind it.
        for entry in &listing.entries {
            if entry.path == "outward" || entry.path == "real/loop" {
                assert_eq!(entry.kind, Kind::Directory, "{}", entry.path);
            }
        }
        assert!(!listing.truncated, "the cycle filled the listing: {paths:?}");
    }

    #[cfg(windows)]
    fn symlink_dir(target: &Path, link: &Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[cfg(not(windows))]
    fn symlink_dir(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    /// A symlinked folder is somewhere the user can go, so the picker offers
    /// it. The entry type on a directory entry describes the link rather than
    /// its target, which is the thing this is really checking.
    #[test]
    fn the_picker_offers_a_symlinked_folder() {
        let directory = tree(&["real/"]);
        let link = directory.path().join("linked");
        if !symlink_dir(&directory.path().join("real"), &link) {
            eprintln!("skipped: this machine will not create directory symlinks");
            return;
        }

        let listed = browse(&inside(directory.path()), None).expect("listed");
        assert_eq!(names(&listed), ["linked", "real"]);
    }
}
