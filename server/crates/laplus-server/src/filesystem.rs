//! Enumerating names on disk: the folder picker, the file tree, and the search
//! behind them.
//!
//! Three method tags land here, and their namespaces are upstream's rather than
//! a boundary — they are the same question asked at three scales, which is why
//! upstream groups them in one service too (`WorkspaceEntries`):
//!
//! - **browse** is one directory, directories only, filtered by a prefix. It is
//!   what the command palette drives while a user types a path into "add
//!   project", so it is called once per keystroke and must stay small.
//! - **listEntries** is a whole workspace, files and directories, in one
//!   answer. It is what the file tree renders from.
//! - **searchEntries** is that same workspace filtered by a fragment of a path.
//!   It is what the composer's `@` mention drives, debounced at 120 ms, so it
//!   reads [`Index`] rather than the disk. What keeps that reading true while
//!   the agent works is [`crate::watcher`] — see [`Index`].
//!
//! Reading and writing the *contents* of one of these files is
//! [`crate::files`], which is a different concern with a different rule — it
//! confines itself to a workspace root, and nothing here does.
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
//! No method here restricts where it may look, and that is deliberate rather
//! than overlooked: the folder picker's whole purpose is to walk a filesystem
//! the server has no project for yet, so a confinement rule would have to admit
//! every path anyway. Reachability is the boundary — the socket is bound to
//! loopback (see [`crate::server`]) — and everything here only ever *reads*.
//! [`crate::files`] is the module that does confine itself, because it is the
//! one that writes.
//!
//! Shapes are hand-written from `FilesystemBrowseResult` and
//! `ProjectListEntriesResult` in `t3code/packages/contracts/src/filesystem.ts`
//! and `project.ts`, and the error shapes from the `ProjectReadFileError`
//! captured in `fixtures/socket-wire/03-typed-error.ndjson` — the one typed
//! error of this family that was recorded from the reference server.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::projects::{expand_home, Rejection, WorkspaceRoot};
use crate::rpc::{declared, non_blank};
use crate::watcher::Watcher;

/// One directory, for the folder picker.
pub const BROWSE: &str = "filesystem.browse";

/// The `_tag` each of the three methods refuses under. The client decodes
/// against the error union its own method declares, so these are not
/// interchangeable.
const BROWSE_ERROR: &str = "FilesystemBrowseError";
const LIST_ERROR: &str = "ProjectListEntriesError";
const SEARCH_ERROR: &str = "ProjectSearchEntriesError";

/// A whole workspace, for the file tree.
pub const LIST_ENTRIES: &str = "projects.listEntries";

/// That workspace filtered by a fragment of a path, for the `@` mention.
pub const SEARCH_ENTRIES: &str = "projects.searchEntries";

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

/// The directory the fallback walk never descends into.
///
/// Only the walk needs this: the repository path below asks git, and git has
/// never listed its own directory. It is here because the walk is what runs on
/// a folder that is *not* a repository, where `.git` may still exist — a
/// checkout with a broken index, a worktree git will not answer for — and it is
/// the one directory that is machine state rather than source and is large
/// enough on its own to spend the whole of [`MAX_ENTRIES`] on loose objects.
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
            .map_err(|error| declared(BROWSE_ERROR, format_args!("filesystem.browse is malformed: {error}")))?;

        let partial_path = read.partial_path.trim().to_string();
        if partial_path.is_empty() {
            return Err(declared(BROWSE_ERROR, "A browse needs a path; none was given."));
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
            "_tag": BROWSE_ERROR,
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
            declared(LIST_ERROR, format_args!("projects.listEntries is malformed: {error}"))
        })?;

        Ok(ListEntries {
            cwd: non_blank(&read.cwd, LIST_ERROR, "workspace root")?,
        })
    }

    /// Do the work. Blocking, and called from a blocking task — a cold
    /// repository of twenty thousand files is seconds of disk, not microseconds
    /// of memory.
    ///
    /// Always rescans. This is the UI saying "show me this project", on opening
    /// it and on the refresh button, and answering either of those from a held
    /// scan would show the user a tree they had just asked to have redrawn.
    pub fn run(self, index: &Index) -> Result<Value, Value> {
        let root = WorkspaceRoot::check(&self.cwd).map_err(|rejection| self.to_error(&rejection))?;
        match index.rescan(&root, MAX_ENTRIES) {
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
            "_tag": LIST_ERROR,
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
/// The three that name a search index are upstream's own — laplus has no
/// index, and a failure the server can never produce is not worth a branch that
/// can never run.
fn listing_failure(rejection: &Rejection) -> &'static str {
    match rejection {
        Rejection::Blank | Rejection::Missing(_) => "workspace_root_not_found",
        Rejection::NotADirectory(_) => "workspace_root_not_directory",
        Rejection::NotReadable(_) | Rejection::Unusable { .. } => "workspace_root_stat_failed",
    }
}


// ---------------------------------------------------------------------------
// projects.searchEntries
// ---------------------------------------------------------------------------

/// The most matches a client may ask for, from `ProjectSearchEntriesInput`.
/// The composer asks for eighty.
const MAX_MATCHES: usize = 200;

/// A validated `projects.searchEntries` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchEntries {
    cwd: String,
    /// Lower-cased and stripped, ready to match against a lower-cased path.
    query: String,
    limit: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchEntriesPayload {
    cwd: String,
    query: String,
    limit: usize,
}

impl SearchEntries {
    pub fn read(payload: &Value) -> Result<SearchEntries, Value> {
        let read: SearchEntriesPayload = serde_json::from_value(payload.clone()).map_err(|error| {
            declared(SEARCH_ERROR, format_args!("projects.searchEntries is malformed: {error}"))
        })?;

        Ok(SearchEntries {
            cwd: non_blank(&read.cwd, SEARCH_ERROR, "workspace root")?,
            query: normalise_query(&read.query),
            // The contract caps the limit and the client already honours it, so
            // this is a clamp rather than a refusal — a client asking for too
            // much gets as much as the contract allows rather than an error it
            // cannot act on. Zero would mean "no matches, and also truncated",
            // which is not an answer, so it takes at least one.
            limit: read.limit.clamp(1, MAX_MATCHES),
        })
    }

    /// Answer from the held scan, scanning only if there is none.
    ///
    /// This is a keystroke — the composer debounces at 120 ms and the user is
    /// mid-word — so it must not cost a repository scan. What it can cost is
    /// being one write behind, and [`Index::forget`] is what keeps that bounded.
    pub fn run(self, index: &Index) -> Result<Value, Value> {
        let root = WorkspaceRoot::check(&self.cwd).map_err(|rejection| self.to_error(&rejection))?;
        let listing = index
            .current(&root, MAX_ENTRIES)
            .map_err(|rejection| self.to_error(&rejection))?;

        // An empty query is not "match everything": the composer only sends one
        // once the user has typed after the `@`, and answering the whole
        // workspace would put a thousand unrelated files under the cursor.
        let matched: Vec<&Entry> = if self.query.is_empty() {
            Vec::new()
        } else {
            listing
                .entries
                .iter()
                .filter(|entry| entry.path.to_lowercase().contains(&self.query))
                .collect()
        };

        Ok(json!({
            "entries": matched
                .iter()
                .take(self.limit)
                .map(|entry| entry.to_value())
                .collect::<Vec<Value>>(),
            "truncated": matched.len() > self.limit,
        }))
    }

    fn to_error(&self, rejection: &Rejection) -> Value {
        let mut error = declared(SEARCH_ERROR, rejection.message());
        error["cwd"] = json!(self.cwd);
        error["queryLength"] = json!(self.query.len());
        error["limit"] = json!(self.limit);
        error["failure"] = json!(listing_failure(rejection));
        error
    }
}

/// What the client typed, reduced to what it can be matched on.
///
/// The leading `@`, `.` and `/` come off because the composer sends the mention
/// trigger along with the fragment — upstream strips exactly these
/// (`WorkspaceEntries.search`), and a query of `@src` would otherwise match
/// nothing at all.
fn normalise_query(query: &str) -> String {
    query
        .trim()
        .trim_start_matches(['@', '.', '/'])
        .to_lowercase()
}


// ---------------------------------------------------------------------------
// The scan behind listEntries and searchEntries
// ---------------------------------------------------------------------------

/// The last scan of each workspace, so a burst of searches costs one.
///
/// The composer debounces its `@` mention at 120 ms and asks for eighty
/// matches; scanning a repository takes tens to hundreds of milliseconds. One
/// scan per keystroke would be the difference between a picker that keeps up
/// with typing and one that does not, so the scan is held and the search reads
/// it.
///
/// **Freshness is decided by which method is asking, not by a clock.**
/// `listEntries` is the UI saying "show me this project" — on opening it, and
/// on the refresh button — so it always rescans and leaves the result here.
/// `searchEntries` is a keystroke, so it takes whatever is here and only scans
/// when there is nothing. Two things invalidate: the app's own
/// `projects.writeFile` ([`Index::forget`]), and — ticket 08 — a change made by
/// anything else, which [`crate::watcher`] reports and [`Index::changed`] acts
/// on. Both go through the same door, which is why there is only one.
///
/// No expiry: an entry costs one workspace's paths and is replaced on the next
/// `listEntries`. A time-to-live would be a guess at how stale is too stale,
/// and the watcher is the honest answer to the question a number would be
/// guessing at.
///
/// ## Why a change invalidates rather than rescanning
///
/// The obvious alternative is to rescan in the background when the watcher
/// fires, so a search never waits. It is the wrong trade here, for two reasons
/// that only show up under load. A rescan shells out to `git ls-files` twice
/// and costs about a tenth of a second on a large repository, so a `cargo
/// build` or an `npm install` — thousands of events over minutes — would be
/// asking a background thread to do that over and over for a workspace nobody
/// is currently searching, which is exactly the "pins a core" failure. And it
/// would have to be debounced to be tolerable, which trades a bounded cost for
/// unbounded staleness during a sustained burst.
///
/// Forgetting costs one map removal per event and moves the scan to the first
/// caller who actually needs it. **That is where the coalescing comes from**:
/// a thousand changes and one search cost one scan, however fast the thousand
/// arrived, because forgetting something that is already forgotten is free.
#[derive(Clone)]
pub struct Index {
    /// Keyed by [`WorkspaceRoot::canonical`], so two spellings of one folder
    /// share a scan rather than each paying for their own.
    scans: Scans,
    /// Watching whatever [`Index::rescan`] has held a scan for. Holds a handle
    /// on `scans` and nothing on the `Index` itself, so the two do not keep
    /// each other alive.
    watcher: Watcher,
    /// Everything else that wants to hear about a change — [`crate::git`] is
    /// the only one. See [`Index::on_change`].
    listeners: Listeners,
}

/// The last scan of each workspace, shared with the watcher's callback.
type Scans = Arc<Mutex<HashMap<String, Arc<Listing>>>>;

/// Something other than the scans that wants to hear about a change. The
/// arguments are [`crate::watcher`]'s: the workspace's key, and the changed path
/// relative to its root.
type Listener = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// What else the one watcher tells, beyond the scans it was built for.
type Listeners = Arc<Mutex<Vec<Listener>>>;

impl Default for Index {
    fn default() -> Index {
        Index::new()
    }
}

impl std::fmt::Debug for Index {
    /// The counts rather than the contents, and by hand because a listener is a
    /// closure and closures have no `Debug`. What is printed is the same
    /// judgement [`Watcher`]'s own `Debug` makes: this reaches every dispatch
    /// trace through [`crate::rpc::Services`], and the developer's project
    /// directories are not something to scatter through a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Index")
            .field("scans", &lock(&self.scans).len())
            .field("watcher", &self.watcher)
            .field("listeners", &lock_listeners(&self.listeners).len())
            .finish()
    }
}

impl Index {
    pub fn new() -> Index {
        let scans = Scans::default();
        let listeners = Listeners::default();
        let held = Arc::clone(&scans);
        let told = Arc::clone(&listeners);
        Index {
            watcher: Watcher::new(move |key: &str, relative: &str| {
                changed(&held, key, relative);
                for listener in lock_listeners(&told).iter() {
                    listener(key, relative);
                }
            }),
            scans,
            listeners,
        }
    }

    /// Tell `listener` about every change the one watcher reports, alongside
    /// the scans it was built for.
    ///
    /// There is a single [`Watcher`] in the process on purpose — see that
    /// module's first section — so a second subsystem that needs to hear about
    /// changes registers here rather than starting a watcher of its own, which
    /// would double the handles held on every workspace to hear the same
    /// events twice.
    ///
    /// **A listener must not reach back into this index**, because it is
    /// called with the listener registry held. Registration happens once at
    /// startup and the list is read on every event, so cloning it per event
    /// would be an allocation on the busiest path there is.
    pub fn on_change(&self, listener: impl Fn(&str, &str) + Send + Sync + 'static) {
        lock_listeners(&self.listeners).push(Arc::new(listener));
    }

    /// Watch a workspace without scanning it.
    ///
    /// [`Index::rescan`] is the usual way a workspace comes to be watched, and
    /// it watches because it has a scan to keep fresh. A subscriber to
    /// something *other* than the file tree — [`crate::git`] — has its own
    /// reason to want the events and no scan to go with it, so it says so
    /// here. Idempotent, and it counts as a use for the eviction order the
    /// same way a listing does.
    pub fn observe(&self, root: &WorkspaceRoot) {
        self.watcher
            .watch(root.canonical(), Path::new(root.display()));
    }

    /// Scan now, keep the result, and watch the workspace it came from.
    ///
    /// The watch is started here rather than when the project is registered
    /// because this is the moment there is something to keep fresh: a scan
    /// nobody has taken cannot go stale. It is idempotent, so the refresh
    /// button does not accumulate registrations.
    ///
    /// **The hold is released before the watch is asked for.** The watcher's
    /// callback runs on its own thread and takes `scans`; this path takes
    /// `scans` and then the watcher's registry. Holding one across the other
    /// here would complete the cycle.
    fn rescan(&self, root: &WorkspaceRoot, limit: usize) -> Result<Arc<Listing>, Rejection> {
        let listing = Arc::new(scan(root, limit)?);
        self.hold(root.canonical(), Arc::clone(&listing));
        self.watcher
            .watch(root.canonical(), Path::new(root.display()));
        Ok(listing)
    }

    /// Whatever was last scanned, scanning only if nothing was.
    fn current(&self, root: &WorkspaceRoot, limit: usize) -> Result<Arc<Listing>, Rejection> {
        if let Some(held) = self.held(root.canonical()) {
            return Ok(held);
        }
        self.rescan(root, limit)
    }

    /// Drop what is held for a workspace, so the next reader scans.
    ///
    /// Takes the path as the client spelled it and canonicalises it here: a
    /// write names the workspace the same way the listing did, but there is no
    /// guarantee the two calls spelled it identically.
    pub fn forget(&self, cwd: &str) {
        if let Ok(root) = WorkspaceRoot::check(cwd) {
            lock(&self.scans).remove(root.canonical());
        }
    }

    /// Forget a workspace and stop watching it — what a project being closed
    /// costs the server.
    ///
    /// Keyed by the canonical root rather than by a path from a client, because
    /// the caller is `project.delete` and the folder may well be gone by the
    /// time it runs; [`Index::forget`]'s re-check would then quietly do
    /// nothing and leave the handle held.
    pub fn release(&self, canonical_root: &str) {
        self.watcher.release(canonical_root);
        lock(&self.scans).remove(canonical_root);
    }

    /// Workspaces currently being watched. The gauge that says a closed project
    /// gave its handle back.
    pub fn watched(&self) -> usize {
        self.watcher.len()
    }

    fn held(&self, key: &str) -> Option<Arc<Listing>> {
        lock(&self.scans).get(key).map(Arc::clone)
    }

    fn hold(&self, key: &str, listing: Arc<Listing>) {
        lock(&self.scans).insert(key.to_string(), listing);
    }
}

/// What the watcher does with a reported change.
///
/// A free function rather than a method because it is called from the watcher's
/// own thread, which holds the scans and not the [`Index`] — see
/// [`Index::new`].
///
/// **Two changes are ignored, and between them they are most of what a busy
/// machine produces.** A workspace with nothing held has nothing to invalidate,
/// so a build that has already forgotten it once costs nothing for the rest of
/// its run. And a path the last listing would not have named is not part of the
/// workspace — see [`Listing::is_interesting`], which is where `node_modules`,
/// `target` and `.git` stop being the server's problem without this module
/// having to know any of those names.
fn changed(scans: &Scans, key: &str, relative: &str) {
    let mut scans = lock(scans);
    let Some(listing) = scans.get(key) else {
        return;
    };
    if listing.is_interesting(relative) {
        scans.remove(key);
    }
}

/// A poisoned lock means a previous holder panicked while holding it. A
/// `HashMap` is not left half-written by that, and refusing to read the scans
/// again would turn one panic into a session where no project can be searched —
/// the same reasoning, and the same choice, as [`crate::watcher`]'s own lock.
fn lock(scans: &Scans) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Listing>>> {
    scans.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The same tolerance for the listener registry, and for the same reason: a
/// `Vec` is not left half-written by a panic, and refusing to read it again
/// would silently stop git status refreshing for the rest of the session.
fn lock_listeners(listeners: &Listeners) -> std::sync::MutexGuard<'_, Vec<Listener>> {
    listeners
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a workspace holds, as the file tree reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Listing {
    entries: Vec<Entry>,
    /// The workspace held more than the limit. The UI renders this as
    /// "· partial" beside the file count.
    truncated: bool,
    /// Every directory in `entries`, lower-cased, for
    /// [`Listing::is_interesting`]. Derived rather than given — see
    /// [`Listing::new`], which is the only way to build one.
    directories: HashSet<String>,
}

impl Listing {
    /// Build a listing, deriving what the watcher will need to ask of it.
    ///
    /// The set is built from the entries *after* truncation, so it describes
    /// the listing the client was actually given rather than the one the scan
    /// found. Lower-cased once here rather than at each of the thousands of
    /// events a build produces.
    fn new(entries: Vec<Entry>, truncated: bool) -> Listing {
        Listing {
            directories: entries
                .iter()
                .filter(|entry| entry.kind == Kind::Directory)
                .map(|entry| entry.path.to_lowercase())
                .collect(),
            entries,
            truncated,
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "entries": self.entries.iter().map(Entry::to_value).collect::<Vec<Value>>(),
            "truncated": self.truncated,
        })
    }

    /// Could a change at this path have changed what this listing says?
    ///
    /// **The rule is one line: the path's parent must be the workspace root or
    /// a directory this listing names.** It is worth spelling out why that is
    /// the right line, because it is doing the whole of ticket 08's "watching
    /// does not recurse into ignored directories".
    ///
    /// A recursive watch cannot be told to skip a subtree — Windows'
    /// `ReadDirectoryChangesW` is all-or-nothing — so the exclusion has to
    /// happen on the events. But the server has no ignore rules of its own to
    /// filter by: what is in a workspace is whatever `git ls-files` said, and
    /// asking git per event would cost far more than the scan being avoided.
    ///
    /// The last listing *is* the ignore rule, already computed. `node_modules`
    /// is not in it, so nothing under `node_modules/left-pad/` has a parent it
    /// names, and an `npm install` passes without a single invalidation. The
    /// same holds for `target/debug/…` and `.git/objects/…`, and it holds for
    /// whatever a project ignores that this server has never heard of.
    ///
    /// The one thing it deliberately does not filter is a change *directly*
    /// inside a known directory — including the creation of `node_modules`
    /// itself, whose parent is the root. That costs one invalidation, and it
    /// has to: until the workspace is scanned again there is no way to know
    /// whether a new directory is ignored or is the user's new feature. After
    /// that scan it is absent from the listing and its subtree is silent.
    ///
    /// Two consequences worth naming. A file created two levels below the last
    /// listing — `src/newthing/a.rs` where `src/newthing` is also new — is not
    /// reported, but the creation of `src/newthing` was, so the listing is
    /// already forgotten by the time the file arrives and there is nothing left
    /// to miss. And a listing that was `truncated` names fewer directories than
    /// the workspace has, so changes below the cut are ignored — which is the
    /// same bargain the truncation itself struck.
    fn is_interesting(&self, relative: &str) -> bool {
        match relative.rsplit_once('/') {
            // Directly in the workspace root, or the root itself. Nothing a
            // listing knows can dismiss that.
            None => true,
            Some((parent, _)) => self.directories.contains(&parent.to_lowercase()),
        }
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

/// Describe a workspace: what git says is in it, or a plain walk if git will
/// not say.
///
/// **Asking git is how laplus gets ignore semantics without owning any.**
/// A file tree that shows `node_modules` is not merely untidy — the entry limit
/// is finite, and in a JavaScript project the ignored files exhaust it before
/// the walk reaches the user's own source, so the tree renders with
/// `packages/web/src` present and empty. Ticket 25 is where that was decided;
/// the short version is that `.gitignore` has enough subtlety (negations,
/// anchoring, nested files) that implementing it approximately would hide files
/// silently, and the two honest options were a matcher crate — which roughly
/// doubles the dependency graph of a project whose whole reason is size — or
/// the tool that already knows the answer.
///
/// The spec already commits to shelling out to `git` for the git tickets, so
/// this adds no dependency and no bytes.
///
/// Two calls rather than one, and the second is not optional: `--cached` lists
/// what the *index* holds, which includes files the user has deleted without
/// staging the deletion. A tree that offered those would offer files that are
/// not there, and ticket 07's read would fail on every one of them.
fn scan(root: &WorkspaceRoot, limit: usize) -> Result<Listing, Rejection> {
    let directory = Path::new(root.display());
    match tracked(directory) {
        Some(files) => Ok(listing_of(files, limit)),
        None => walk(directory, limit),
    }
}

/// The paths git says are in the working tree, or `None` when it will not
/// answer — git is not installed, or this folder is not a repository.
fn tracked(root: &Path) -> Option<Vec<String>> {
    let present = git(
        root,
        &["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
    )?;
    // A failure here is not a reason to abandon a listing git has already
    // given: the worst case is a handful of deleted files shown as present,
    // which is better than falling back to a walk that shows every ignored one.
    let gone: HashSet<String> = git(root, &["ls-files", "--deleted", "-z"])
        .map(|deleted| split_nul(&deleted).collect())
        .unwrap_or_default();

    Some(
        split_nul(&present)
            .filter(|path| !gone.contains(path))
            .collect(),
    )
}

/// Run one `git` and take its output, or `None` if it did not succeed.
///
/// The spawning itself is [`crate::git::output`], which is where the flags
/// every `git` this server runs needs — no console window, no optional locks,
/// untranslated messages — are decided once. Here the *reason* a call did not
/// answer is genuinely not wanted: git absent and this folder not being a
/// repository lead to the same fallback walk.
fn git(root: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = crate::git::output(root, arguments).ok()?;
    output.status.success().then_some(output.stdout)
}

/// `-z` output: NUL-separated, and unquoted whatever `core.quotePath` says —
/// which is the reason for asking for it in the first place, because the
/// default quoting would mangle every non-ASCII name.
fn split_nul(output: &[u8]) -> impl Iterator<Item = String> + '_ {
    output
        .split(|byte| *byte == 0)
        .filter(|piece| !piece.is_empty())
        .map(|piece| String::from_utf8_lossy(piece).into_owned())
}

/// Turn a flat list of files into the tree the client decodes.
///
/// Git names files and never the directories holding them, so the ancestors are
/// synthesised — the same manoeuvre upstream's indexer makes
/// (`withDirectoryAncestors`), for the same reason: the tree splits paths and
/// needs a node to hang each level on.
///
/// One consequence is worth naming: a directory with no files under it does not
/// appear, because git has nothing to say about it. An empty folder is
/// invisible in the tree until something is put in it.
fn listing_of(files: Vec<String>, limit: usize) -> Listing {
    let mut known: HashMap<String, Kind> = HashMap::new();
    for file in files {
        let mut ancestor = file.as_str();
        while let Some((parent, _)) = ancestor.rsplit_once('/') {
            if known.insert(parent.to_string(), Kind::Directory).is_some() {
                // This parent was reached before, so every parent above it was
                // too.
                break;
            }
            ancestor = parent;
        }
        known.insert(file, Kind::File);
    }

    let mut entries: Vec<Entry> = known
        .into_iter()
        .map(|(path, kind)| Entry { path, kind })
        .collect();
    entries.sort_by(|left, right| by_name(&left.path, &right.path));

    // Sorting first is what makes truncation safe: a path always sorts after
    // the path that is its prefix, so cutting the tail can never leave an entry
    // whose parent is missing.
    let truncated = entries.len() > limit;
    entries.truncate(limit);
    Listing::new(entries, truncated)
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
fn walk(root: &Path, limit: usize) -> Result<Listing, Rejection> {
    let root = root.to_path_buf();
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
            "laplus: {unreadable} entr(ies) under {display} could not be read \
             and are listed with no contents, or not at all"
        );
    }

    entries.sort_by(|left, right| by_name(&left.path, &right.path));
    Ok(Listing::new(entries, truncated))
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

    /// How long a test will wait for the operating system to report a change.
    ///
    /// The same bound, for the same reason, as `watcher::tests::PATIENCE` and
    /// `socket_watch.rs`'s: generously long, and not a claim about latency. It
    /// is what turns "never arrives" into a failure with a message rather than
    /// a hung suite.
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

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

        let listing = walk(directory.path(), MAX_ENTRIES).expect("listed");

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

        let listing = walk(directory.path(), MAX_ENTRIES).expect("listed");
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

        let listing = walk(directory.path(), 3).expect("listed");

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

        let listing = walk(directory.path(), MAX_ENTRIES).expect("listed");

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

        let listing = walk(directory.path(), MAX_ENTRIES)
            .expect("one unreadable entry does not fail the workspace");

        assert_eq!(paths(&listing), ["dangling", "kept", "kept/file.txt"]);
        // Nothing resolves behind it, so the only honest kind left is the one
        // that promises the tree no children.
        assert_eq!(listing.entries[0].kind, Kind::File);
    }

    /// Run one git command in `directory`, failing the test if it does not
    /// succeed. Unlike the symlink helper this does not skip: the spec commits
    /// to shelling out to `git` for the git tickets, so a machine without it
    /// cannot run this suite meaningfully in the first place.
    fn run_git(directory: &Path, arguments: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(directory)
            // A repository made inside a test must not inherit the developer's
            // identity, hooks or templates — and `commit` refuses without a
            // name and address, so they are supplied rather than assumed.
            .args(["-c", "user.name=laplus-test"])
            .args(["-c", "user.email=test@laplus.invalid"])
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "core.hooksPath=/dev/null"])
            .args(arguments)
            .output()
            .expect("git runs");
        assert!(
            status.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    fn repository(paths: &[&str]) -> tempfile::TempDir {
        let directory = tree(paths);
        run_git(directory.path(), &["init"]);
        directory
    }

    fn scanned(directory: &Path) -> Listing {
        let root = WorkspaceRoot::check(&directory.to_string_lossy()).expect("a workspace");
        scan(&root, MAX_ENTRIES).expect("scanned")
    }

    /// The whole point of asking git: what the repository ignores does not
    /// reach the tree.
    ///
    /// Nothing is committed here, so this runs entirely through
    /// `--others --exclude-standard` — which is the case that matters, because
    /// a working tree is mostly untracked files the moment anyone edits it.
    #[test]
    fn a_repository_does_not_list_what_it_ignores() {
        let directory = repository(&[
            ".gitignore",
            "src/main.rs",
            "node_modules/left-pad/index.js",
            "target/debug/build.log",
        ]);
        std::fs::write(
            directory.path().join(".gitignore"),
            "node_modules/\ntarget/\n",
        )
        .expect("writes the ignore file");

        let paths = {
            let listing = scanned(directory.path());
            listing
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<String>>()
        };

        assert_eq!(paths, [".gitignore", "src", "src/main.rs"], "{paths:?}");
    }

    /// Git names files; the directories holding them are inferred. Without that
    /// the tree would have leaves and no branches to hang them on.
    #[test]
    fn directories_are_inferred_from_the_files_git_names() {
        let listing = listing_of(
            vec![
                "packages/web/src/app.tsx".to_string(),
                "packages/web/README.md".to_string(),
                "top.txt".to_string(),
            ],
            MAX_ENTRIES,
        );

        assert_eq!(
            paths(&listing),
            [
                "packages",
                "packages/web",
                "packages/web/README.md",
                "packages/web/src",
                "packages/web/src/app.tsx",
                "top.txt",
            ]
        );
        assert_eq!(listing.entries[0].kind, Kind::Directory);
        assert!(!listing.truncated);
    }

    /// Sorting before truncating is what keeps a cut listing coherent: a path
    /// always sorts after the path that is its prefix, so no entry can lose its
    /// parent.
    #[test]
    fn a_truncated_git_listing_still_has_every_parent() {
        let listing = listing_of(
            vec![
                "a/b/c/d.txt".to_string(),
                "z.txt".to_string(),
                "m/n.txt".to_string(),
            ],
            4,
        );

        assert!(listing.truncated);
        assert_eq!(listing.entries.len(), 4);
        for entry in &listing.entries {
            if let Some((parent, _)) = entry.path.rsplit_once('/') {
                assert!(
                    paths(&listing).contains(&parent),
                    "{} lost its parent",
                    entry.path
                );
            }
        }
    }

    /// `--cached` lists what the index holds, which includes a file the user
    /// deleted without staging the deletion. Offering those would put files in
    /// the tree that are not on disk, and every attempt to open one would fail.
    #[test]
    fn a_file_deleted_without_staging_is_not_listed() {
        let directory = repository(&["kept.txt", "removed.txt"]);
        run_git(directory.path(), &["add", "."]);
        run_git(directory.path(), &["commit", "-m", "first"]);
        std::fs::remove_file(directory.path().join("removed.txt")).expect("removes the file");

        let listing = scanned(directory.path());

        assert_eq!(paths(&listing), ["kept.txt"], "{:?}", paths(&listing));
    }

    /// A folder that is not a repository still has a file tree, and it comes
    /// from the walk — which is also the path every other test in this module
    /// drives directly.
    #[test]
    fn a_folder_that_is_not_a_repository_falls_back_to_the_walk() {
        let directory = tree(&["src/main.rs", "notes.md"]);
        assert!(
            !directory.path().join(".git").exists(),
            "the fixture is not a repository"
        );

        assert_eq!(
            paths(&scanned(directory.path())),
            paths(&walk(directory.path(), MAX_ENTRIES).expect("walked"))
        );
    }

    fn search(index: &Index, cwd: &Path, query: &str, limit: usize) -> Value {
        SearchEntries::read(&json!({
            "cwd": cwd.to_string_lossy(),
            "query": query,
            "limit": limit,
        }))
        .expect("a well-formed payload")
        .run(index)
        .expect("a readable workspace")
    }

    fn matched(result: &Value) -> Vec<&str> {
        result["entries"]
            .as_array()
            .expect("an array")
            .iter()
            .map(|entry| entry["path"].as_str().expect("a path"))
            .collect()
    }

    /// The composer's `@` mention: a fragment of a path in, the paths holding
    /// it out. Matching is on the whole path rather than the final component,
    /// because `web/app` is how a user distinguishes two files both called
    /// `app.tsx`.
    #[test]
    fn a_search_matches_any_part_of_a_path() {
        let directory = tree(&["packages/web/app.tsx", "packages/api/app.ts", "README.md"]);
        let index = Index::new();

        assert_eq!(
            matched(&search(&index, directory.path(), "app.ts", 80)),
            ["packages/api/app.ts", "packages/web/app.tsx"]
        );
        assert_eq!(
            matched(&search(&index, directory.path(), "web/app", 80)),
            ["packages/web/app.tsx"]
        );
        // Case-insensitively, because the user is typing.
        assert_eq!(
            matched(&search(&index, directory.path(), "README", 80)),
            ["README.md"]
        );
        assert_eq!(
            matched(&search(&index, directory.path(), "readme", 80)),
            ["README.md"]
        );
    }

    /// The trigger character arrives with the fragment, and a query of `@src`
    /// would otherwise match nothing at all.
    #[test]
    fn the_mention_trigger_is_stripped_from_the_query() {
        let directory = tree(&["src/main.rs"]);
        let index = Index::new();

        for query in ["@src", "./src", "/src", "@./src"] {
            assert_eq!(
                matched(&search(&index, directory.path(), query, 80)),
                ["src", "src/main.rs"],
                "{query}"
            );
        }
    }

    /// An empty query is not "everything". The composer sends one while the
    /// user is still deciding, and a thousand unrelated files under the cursor
    /// is not a useful answer to that.
    #[test]
    fn an_empty_query_matches_nothing() {
        let directory = tree(&["src/main.rs"]);
        let result = search(&Index::new(), directory.path(), "   @  ", 80);

        assert_eq!(matched(&result), Vec::<&str>::new());
        assert_eq!(result["truncated"], json!(false));
    }

    /// The client says how many it can show. More than that is `truncated`, so
    /// the picker can say there is more rather than implying it has everything.
    #[test]
    fn a_search_stops_at_the_limit_the_client_asked_for() {
        let directory = tree(&["a1.rs", "a2.rs", "a3.rs", "b.rs"]);
        let index = Index::new();

        let capped = search(&index, directory.path(), "a", 2);
        assert_eq!(matched(&capped).len(), 2);
        assert_eq!(capped["truncated"], json!(true));

        let complete = search(&index, directory.path(), "a", 80);
        assert_eq!(matched(&complete).len(), 3);
        assert_eq!(complete["truncated"], json!(false));
    }

    /// Search reads the held scan; a listing replaces it, and a write forgets
    /// it. Those are the two doors a *method* controls, and between them they
    /// are the freshness rule as the client can see it.
    ///
    /// The third door — a change nobody asked for — is ticket 08's, and it is
    /// driven separately below, because asserting it here would mean asserting
    /// that an operating-system event had *not* arrived yet.
    #[test]
    fn a_search_reads_the_held_scan_and_a_listing_replaces_it() {
        let directory = tree(&["before.txt"]);
        let index = Index::new();
        let cwd = directory.path().to_string_lossy().into_owned();

        // Nothing held yet, so the first search scans for itself.
        assert_eq!(
            matched(&search(&index, directory.path(), "txt", 80)),
            ["before.txt"]
        );

        std::fs::write(directory.path().join("after.txt"), "new").expect("writes the file");
        ListEntries::read(&json!({"cwd": &cwd}))
            .expect("a well-formed payload")
            .run(&index)
            .expect("listed");
        assert_eq!(
            matched(&search(&index, directory.path(), "txt", 80)),
            ["after.txt", "before.txt"],
            "listing the project did not refresh what search reads"
        );

        // And forgetting is the other door in, which is what a write uses.
        std::fs::write(directory.path().join("third.txt"), "new").expect("writes the file");
        index.forget(&cwd);
        assert_eq!(
            matched(&search(&index, directory.path(), "txt", 80)),
            ["after.txt", "before.txt", "third.txt"]
        );
    }

    /// A keystroke must not pay for a rescan. Nothing changes between the two
    /// reads, so nothing can invalidate between them either — which is what
    /// makes this the one form of the claim that is not a race with the
    /// watcher.
    ///
    /// Identity rather than equality: two scans of an unchanged workspace are
    /// equal, so comparing values would pass whether or not the disk was
    /// touched again.
    #[test]
    fn a_second_search_reads_the_scan_the_first_one_took() {
        let directory = tree(&["src/main.rs", "README.md"]);
        let index = Index::new();
        let root = WorkspaceRoot::check(&directory.path().to_string_lossy()).expect("a workspace");

        let first = index.current(&root, MAX_ENTRIES).expect("scanned");
        let second = index.current(&root, MAX_ENTRIES).expect("read");

        assert!(
            Arc::ptr_eq(&first, &second),
            "the second keystroke went back to the disk"
        );
    }

    /// The rule the watcher's events are filtered by, without an operating
    /// system in the way. It is doing the whole of "watching does not recurse
    /// into ignored directories", so it is worth pinning case by case.
    #[test]
    fn a_change_matters_only_where_the_listing_names_its_parent() {
        let listing = Listing::new(
            vec![
                Entry {
                    path: "src".to_string(),
                    kind: Kind::Directory,
                },
                Entry {
                    path: "src/lib".to_string(),
                    kind: Kind::Directory,
                },
                Entry {
                    path: "src/main.rs".to_string(),
                    kind: Kind::File,
                },
                Entry {
                    path: ".gitignore".to_string(),
                    kind: Kind::File,
                },
            ],
            false,
        );

        // Anything directly in the root, including the root itself and a
        // directory that is about to turn out to be ignored.
        for relative in ["", "README.md", ".gitignore", "node_modules", "target"] {
            assert!(listing.is_interesting(relative), "{relative:?}");
        }
        // Anything directly inside a directory the listing names.
        for relative in ["src/other.rs", "src/lib", "src/lib/util.rs", "SRC/other.rs"] {
            assert!(listing.is_interesting(relative), "{relative:?}");
        }

        // The whole of an ignored subtree, which is where the noise lives: a
        // build writing into `target/`, an install writing into
        // `node_modules/`, and git writing into its own directory.
        for relative in [
            "target/debug/build.log",
            "node_modules/left-pad/index.js",
            ".git/objects/ab/cdef",
            "src/lib/deeper/still/leaf.rs",
        ] {
            assert!(!listing.is_interesting(relative), "{relative:?}");
        }

        // A file that is *not* a directory is not somewhere changes can happen,
        // so nothing claiming to be inside one is worth acting on.
        assert!(!listing.is_interesting("src/main.rs/impossible"));
    }

    /// What the watcher's callback does with a change, driven directly.
    ///
    /// The end-to-end path is `tests/socket_watch.rs`; this is the decision it
    /// rests on, made without waiting for anything.
    #[test]
    fn a_reported_change_drops_the_scan_only_when_it_could_have_changed_it() {
        let directory = repository(&[".gitignore", "src/main.rs", "target/debug/build.log"]);
        std::fs::write(directory.path().join(".gitignore"), "target/\n")
            .expect("writes the ignore file");
        let index = Index::new();
        let root = WorkspaceRoot::check(&directory.path().to_string_lossy()).expect("a workspace");

        let held = index.current(&root, MAX_ENTRIES).expect("scanned");
        let key = root.canonical();

        // Deep inside a directory the listing never named. This is an
        // `npm install` or a `cargo build`, and it must cost nothing.
        changed(&index.scans, key, "target/debug/build.log");
        assert!(
            index.held(key).is_some_and(|now| Arc::ptr_eq(&held, &now)),
            "a change under an ignored directory threw the scan away"
        );

        // A workspace nothing is held for has nothing to invalidate, which is
        // what keeps the *rest* of a long build free.
        changed(&index.scans, "a-workspace-nobody-listed", "src/main.rs");

        // And a change where the listing can see it.
        changed(&index.scans, key, "src/other.rs");
        assert!(
            index.held(key).is_none(),
            "a change beside a listed file left the scan in place"
        );
    }

    /// The end of the whole path, with a real watcher and a real change: a file
    /// that appears without the server writing it makes the next search go and
    /// look.
    ///
    /// Bounded rather than slept-through, so "never arrives" fails with a
    /// message instead of passing by accident.
    #[test]
    fn a_file_created_outside_the_server_reaches_the_next_search() {
        let directory = tree(&["before.txt"]);
        let index = Index::new();
        let root = WorkspaceRoot::check(&directory.path().to_string_lossy()).expect("a workspace");

        index.current(&root, MAX_ENTRIES).expect("scanned");
        assert_eq!(index.watched(), 1, "listing a workspace did not watch it");

        std::fs::write(directory.path().join("ghost.txt"), "not ours").expect("writes the file");

        let deadline = std::time::Instant::now() + PATIENCE;
        while index.held(root.canonical()).is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "a file created outside the server never invalidated the scan"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        assert_eq!(
            matched(&search(&index, directory.path(), "txt", 80)),
            ["before.txt", "ghost.txt"]
        );
    }

    /// Closing a project gives back what was held for it — the scan and the
    /// watch both, keyed by the same canonical root.
    #[test]
    fn releasing_a_workspace_drops_its_scan_and_its_watch() {
        let directory = tree(&["src/main.rs"]);
        let index = Index::new();
        let root = WorkspaceRoot::check(&directory.path().to_string_lossy()).expect("a workspace");

        index.current(&root, MAX_ENTRIES).expect("scanned");
        assert_eq!(index.watched(), 1);
        assert!(index.held(root.canonical()).is_some());

        index.release(root.canonical());

        assert_eq!(index.watched(), 0);
        assert!(index.held(root.canonical()).is_none());
    }

    /// A workspace root that is missing, is a file, or is blank each fails with
    /// the literal the client switches on and a message naming the path.
    #[test]
    fn a_workspace_root_that_cannot_be_listed_is_refused_by_name() {
        let directory = tree(&["a-file.txt"]);

        let missing = directory.path().join("not-there");
        let error = ListEntries::read(&json!({"cwd": missing.to_string_lossy()}))
            .expect("a well-formed payload")
            .run(&Index::new())
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
            .run(&Index::new())
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
        let listing = Listing::new(
            vec![
                Entry {
                    path: "src".to_string(),
                    kind: Kind::Directory,
                },
                Entry {
                    path: "src/main.rs".to_string(),
                    kind: Kind::File,
                },
            ],
            true,
        );

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

        let listing = walk(directory.path(), MAX_ENTRIES).expect("listed");
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
