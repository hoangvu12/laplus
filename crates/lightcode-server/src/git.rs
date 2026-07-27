//! Working tree status: what has changed in a project's folder, kept true
//! while the agent works.
//!
//! Two method tags land here and they are the same answer asked for two ways.
//! `vcs.refreshStatus` is a developer pressing refresh; `subscribeVcsStatus` is
//! the panel that shows the answer and wants to be told when it moves. What the
//! second one adds is the whole of this ticket: the status a developer is
//! looking at is the one thing in the app that goes stale *because* the agent is
//! working, so a status that had to be asked for would be wrong exactly when it
//! mattered.
//!
//! Git is driven by shelling out to the installed `git` binary — the spec's
//! decision, not this module's. There is no library linkage in v1, which is why
//! everything below is a small process and a parser rather than an object graph.
//!
//! ## Where the work happens, and why not on the read loop
//!
//! `git status` on a repository of twenty-five thousand files is tens to
//! hundreds of milliseconds. Neither entry point may pay that where the client
//! would notice:
//!
//! - `vcs.refreshStatus` answers with [`crate::rpc::Deferred`] work, like every
//!   other method that has to wait on the world.
//! - `subscribeVcsStatus` is a stream, and a stream's snapshot is produced on
//!   the pump's own task where blocking would stall a runtime worker. So the
//!   subscription **never shells out**: it describes itself from the last status
//!   read, and a refresh happens on a thread of its own and publishes when it is
//!   done. A subscription opened before any status has been read describes
//!   itself with *nothing* — which [`EventSource`] allows and means literally
//!   "no news yet", rather than an empty status, which would be a claim that the
//!   tree is clean.
//!
//! ## Coalescing, and why it is a pause rather than a counter
//!
//! A `cargo build` produces thousands of change events in a few seconds. One
//! `git status` per event would pin a core and answer questions nobody asked, so
//! a change does not cause a refresh — it marks the repository stale, and one
//! refresh thread runs until the staleness stops arriving. The pause before each
//! read ([`COALESCE`]) is what turns a burst into one read; the re-check after
//! each read is what stops a change *during* a read from being lost. **See
//! ADR-0006**, which is where the alternatives and the sharp edges are.
//!
//! Note what this does not do: it does not debounce indefinitely. A workspace
//! being written to continuously refreshes every [`COALESCE`] plus the length of
//! one read, forever, rather than going quiet until the writing stops. That is
//! the right way round for this subscription — a status that only appeared once
//! the build finished would be useless during the build, which is when the
//! developer is watching it.
//!
//! ## What is answered without going to the network
//!
//! The contract splits a status into a **local** half and a **remote** one, and
//! the capture shows why: the reference server opens with a snapshot whose
//! `remote` is `null` and follows with a separate `remoteUpdated` once it has
//! been to the network (`fixtures/socket-wire/01-browser-session.ndjson`,
//! request 6).
//!
//! **lightcode sends the two together, and that is a declared divergence** —
//! `the_working_tree_status_snapshot_conforms_to_the_capture` in
//! `tests/socket_conformance.rs` enforces it. No call here reaches a network:
//! `aheadCount` and `behindCount` come from git's own record of the tracking
//! branch, which is a local ref read in the same breath as everything else, and
//! `pr` is always null because source-control hosting is explicitly out of v1's
//! scope. There is therefore no later moment at which the remote half could
//! arrive, and nothing for a `remoteUpdated` to carry. See
//! [`Status::to_snapshot`].
//!
//! `sourceControlProvider` is left out for the same reason `pr` is null. It is
//! optional in the contract, and it is a label — GitHub, GitLab — derived from
//! the remote's URL, which is the surface v1 does not have.
//!
//! Shapes are hand-written from `VcsStatusResult` and `VcsStatusStreamEvent` in
//! `t3code/packages/contracts/src/git.ts`, and the error from `GitCommandError`
//! in the same file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::filesystem::Index;
use crate::process::Search;
use crate::projects::WorkspaceRoot;
use crate::subscriptions::{EventSource, BACKLOG};
use crate::watcher::MAX_WATCHED;

/// The working tree status, and every change to it.
pub const SUBSCRIBE_STATUS: &str = "subscribeVcsStatus";

/// The same status, asked for once.
pub const REFRESH_STATUS: &str = "vcs.refreshStatus";

/// The `_tag` both methods refuse under.
///
/// `GitManagerServiceError` is a union of five and this is the one that
/// describes a git command that would not answer. **No `message` is sent with
/// it**: the class defines `message` as a getter over its declared fields, so
/// the client composes the sentence and a server that sent one would be sending
/// a field the reference server does not — the same rule the terminal errors
/// follow.
const ERROR: &str = "GitCommandError";

/// How long a burst of file changes is gathered before one read answers all of
/// them.
///
/// Long enough that a compiler writing a directory of object files is one read
/// rather than hundreds, short enough that a developer who saves a file sees the
/// change land while their hand is still on the keyboard. The composer's `@`
/// mention debounces at 120 ms for the same kind of reason, and this is the same
/// order of magnitude on purpose: both are "wait for the human or the machine to
/// stop, then answer once".
const COALESCE: Duration = Duration::from_millis(150);

/// The most changed files one status will name.
///
/// `VcsStatusResult` has no `truncated` flag, unlike the file tree's listing, so
/// there is nowhere on the wire to say the list was cut. What is bounded is
/// therefore only the *list*: `insertions`, `deletions` and
/// `hasWorkingTreeChanges` are computed over every changed file, so the summary
/// a developer reads stays exactly right and only the per-file rows stop. A
/// working tree with five thousand changed files is a `git checkout` of an
/// unrelated branch, not something anyone reviews row by row.
const MAX_FILES: usize = 5_000;

/// The most bytes one refresh will read to count lines in untracked files.
///
/// Untracked files are the ones `git diff` has nothing to say about, and they
/// are also the interesting case here — a file the agent has just created is
/// untracked, and reporting it as `+0` would be the wrong answer to the one
/// question this ticket exists for. So they are counted by reading them, and
/// this is the ceiling on what that can cost: a refresh reads at most this much
/// however many untracked files there are, and whatever is left over is reported
/// as zero rather than making the refresh unbounded.
const COUNTING_BUDGET: u64 = 8 << 20;

/// The most of any single file that is read to count its lines.
const LONGEST_COUNTED: u64 = 1 << 20;

/// How much of a file decides whether it is text.
///
/// A NUL byte in the first stretch of a file is what git itself treats as the
/// mark of a binary file, and a binary file has no lines to count.
const SNIFFED: usize = 8_000;

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// A validated call — both methods take the same payload, `VcsStatusInput`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusCall {
    cwd: String,
    /// Which method is asking. Only used to fill the error's `operation`, which
    /// is the one thing in a `GitCommandError` that says where it came from.
    operation: &'static str,
}

#[derive(Debug, Deserialize)]
struct StatusPayload {
    cwd: String,
}

impl StatusCall {
    pub fn read(payload: &Value, operation: &'static str) -> Result<StatusCall, Value> {
        let read: StatusPayload = serde_json::from_value(payload.clone())
            .map_err(|error| Unavailable::malformed(error).to_error(operation, ""))?;
        Ok(StatusCall {
            cwd: workspace(&read.cwd).map_err(|why| why.to_error(operation, ""))?,
            operation,
        })
    }

    fn root(&self) -> Result<WorkspaceRoot, Value> {
        WorkspaceRoot::check(&self.cwd).map_err(|rejection| {
            Unavailable::Unusable {
                detail: rejection.message(),
            }
            .to_error(self.operation, &self.cwd)
        })
    }
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// Every workspace whose status is being kept, and the refreshing behind them.
///
/// Shared rather than per-connection, for the reason the file index is: two
/// windows on one project are looking at one working tree, and reading it twice
/// would be paying twice for the same answer. Cheap to clone; every clone is the
/// same registry.
#[derive(Clone)]
pub struct Repositories {
    inner: Arc<Inner>,
    /// Held so that opening a subscription can ask for the workspace to be
    /// watched. **The listener registered on this index must not close over a
    /// `Repositories`**, or the index would keep this alive and this would keep
    /// the index alive; it closes over [`Inner`] instead.
    index: Index,
}

struct Inner {
    /// Least-recently-subscribed first, so the ceiling drops the project nobody
    /// is looking at — the same rule, and the same number, as the watcher's own.
    /// More entries than can be watched would be entries that cannot refresh.
    kept: Mutex<Vec<Arc<Repository>>>,
}

/// One workspace's status, and the feed of changes to it.
///
/// A folder that is not a repository still gets one of these. "Is this a
/// repository" is part of the status rather than a precondition for having one,
/// which is what lets a project with no repository render as such and start
/// working the moment `git init` is run in it.
struct Repository {
    /// The folder under both its names, because both are needed and they must
    /// not be able to disagree: [`WorkspaceRoot::canonical`] is the key the
    /// watcher reports under, and [`WorkspaceRoot::display`] is what git is run
    /// in.
    root: WorkspaceRoot,
    events: broadcast::Sender<Value>,
    state: Mutex<Refreshing>,
}

impl Repository {
    fn path(&self) -> &Path {
        self.root.path()
    }
}

/// What a repository's refreshing knows about itself.
#[derive(Debug, Default)]
struct Refreshing {
    /// The last status read, already in the shape a subscriber opens with.
    /// `None` until the first read finishes.
    last: Option<Value>,
    /// A refresh thread is running for this repository. At most one, ever:
    /// two would race to write `last` and a subscriber would see the older
    /// answer second.
    running: bool,
    /// Something changed since the running refresh started reading, so it must
    /// go round again.
    stale: bool,
    /// Whether the last read failed. Only so that a repository git cannot read
    /// logs once rather than every [`COALESCE`] for as long as the developer
    /// keeps typing.
    reported: bool,
}

impl Repositories {
    /// Build the registry and start listening for changes.
    ///
    /// Takes the index rather than starting a watcher of its own: there is one
    /// watcher in the process, and the file tree and the working tree want the
    /// same events about the same folders.
    pub fn new(index: &Index) -> Repositories {
        let inner = Arc::new(Inner {
            kept: Mutex::new(Vec::new()),
        });
        let told = Arc::clone(&inner);
        index.on_change(move |key: &str, relative: &str| changed(&told, key, relative));
        Repositories {
            inner,
            index: index.clone(),
        }
    }

    /// Open `subscribeVcsStatus`: the status as it is now, then every change.
    ///
    /// Answered from the read loop, which is affordable because nothing here
    /// runs git. What it does do is check the folder, check that there is a
    /// `git` to run at all, remember the workspace, ask for it to be watched,
    /// and start the first read — all of which are bounded by something other
    /// than the size of the repository.
    pub fn subscribe(&self, call: &StatusCall) -> Result<EventSource, Value> {
        let root = call.root()?;
        installed().ok_or_else(|| Unavailable::NotInstalled.to_error(call.operation, &call.cwd))?;

        let repository = self.remember(&root);
        // Before the first read is started, so that a status published between
        // here and the pump's first description is delivered rather than missed.
        let updates = repository.events.subscribe();
        self.index.observe(&root);
        mark_stale(&repository);

        Ok(EventSource::new(
            move || {
                // Nothing at all until the first read lands. An empty status
                // would be a positive claim that the tree is clean.
                lock(&repository.state).last.clone().into_iter().collect()
            },
            updates,
        ))
    }

    /// Answer `vcs.refreshStatus`: read the status now and hand it back.
    ///
    /// Runs off the read loop — see [`crate::rpc::Deferred`]. A workspace that
    /// is being watched also has its held status replaced and its subscribers
    /// told, so pressing refresh and watching the panel cannot disagree.
    pub fn refresh(&self, call: &StatusCall) -> Result<Value, Value> {
        let root = call.root()?;
        let status = read(root.path())
            .map_err(|why| why.to_error(call.operation, &call.cwd))?;

        if let Some(repository) = self.inner.find(root.canonical()) {
            publish(&repository, &status);
        }
        Ok(status.to_result())
    }

    /// Say that something *this server* did has changed a working tree.
    ///
    /// The same door a file change comes through, opened from the inside. A
    /// switch moves `HEAD` and an init makes a `.git`, and the watcher would
    /// notice both eventually — but "eventually" is what makes "switch, then
    /// read the panel" a race, and a call that knows it changed the tree has no
    /// reason to wait to be told. A workspace nobody is keeping is nobody's
    /// panel, so there is nothing to say and this does nothing.
    ///
    /// See ADR-0006 for why this marks rather than reads.
    pub fn disturb(&self, root: &WorkspaceRoot) {
        if let Some(repository) = self.inner.find(root.canonical()) {
            mark_stale(&repository);
        }
    }

    /// How many workspaces are having their status kept. The gauge that says a
    /// subscription actually registered one.
    pub fn kept(&self) -> usize {
        lock_kept(&self.inner).len()
    }

    /// Get or create the entry for a workspace, moving it to the back of the
    /// eviction order because somebody just asked for it.
    fn remember(&self, root: &WorkspaceRoot) -> Arc<Repository> {
        let mut kept = lock_kept(&self.inner);
        if let Some(position) = kept
            .iter()
            .position(|entry| entry.root.canonical() == root.canonical())
        {
            let touched = kept.remove(position);
            kept.push(Arc::clone(&touched));
            return touched;
        }

        if kept.len() >= MAX_WATCHED {
            let evicted = kept.remove(0);
            eprintln!(
                "lightcode: {MAX_WATCHED} working trees are already being kept, so the status \
                 of {} will no longer refresh on its own",
                evicted.root.display()
            );
        }

        let repository = Arc::new(Repository {
            root: root.clone(),
            events: broadcast::channel(BACKLOG).0,
            state: Mutex::new(Refreshing::default()),
        });
        kept.push(Arc::clone(&repository));
        repository
    }
}

impl Inner {
    /// The entry for a workspace, if it is being kept.
    ///
    /// On [`Inner`] rather than on [`Repositories`] because both callers need it
    /// and only one of them has a `Repositories` to ask: the watcher's callback
    /// holds this and nothing else — see [`Repositories::new`].
    fn find(&self, key: &str) -> Option<Arc<Repository>> {
        lock(&self.kept)
            .iter()
            .find(|entry| entry.root.canonical() == key)
            .map(Arc::clone)
    }
}

impl std::fmt::Debug for Repositories {
    /// The count rather than the paths, for the reason [`crate::watcher`]'s own
    /// `Debug` gives: this is printed inside [`crate::rpc::Services`], and a
    /// list of the developer's project directories is not something to scatter
    /// through a log.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Repositories")
            .field("kept", &self.kept())
            .finish()
    }
}

/// What a reported change does to a kept working tree.
///
/// A free function because it is called from the watcher's own thread, which
/// holds [`Inner`] and not the [`Repositories`] around it — see
/// [`Repositories::new`].
fn changed(inner: &Arc<Inner>, key: &str, relative: &str) {
    if !affects_status(relative) {
        return;
    }
    // `find` releases the registry lock before returning, which is what keeps
    // the lock ordering one-way: a refresh thread takes a repository's own
    // state, and nothing may hold the registry across that.
    if let Some(repository) = inner.find(key) {
        mark_stale(&repository);
    }
}

/// Could a change at this path have changed what a status says?
///
/// The rule is the inverse of the file tree's: a listing dismisses everything it
/// does not already name, and a status cannot, because the *point* of a status
/// is the file that is not in the last listing yet. So everything in the working
/// tree counts, and the filtering is only about git's own directory.
///
/// Three exclusions, and each one is a loop this module would otherwise be in:
///
/// - **`.git/objects` and `.git/logs`** churn on every fetch and every commit,
///   in volume, and neither can change a working tree status on its own.
/// - **Lock files** are written and deleted by every git command there is,
///   including the ones below.
///
/// What is deliberately kept is the rest of `.git`: `HEAD` moves when a branch
/// is switched, `index` moves when something is staged, and `MERGE_HEAD` appears
/// when a merge stops half-way. All three change the answer.
fn affects_status(relative: &str) -> bool {
    if relative.ends_with(".lock") {
        return false;
    }
    match relative.strip_prefix(".git/") {
        None => true,
        Some(inside) => !(inside.starts_with("objects/") || inside.starts_with("logs/")),
    }
}

/// Mark a working tree stale, and make sure something is going to read it.
///
/// The whole of ADR-0006 in one function: a change does not read the working
/// tree, it says the working tree needs reading.
fn mark_stale(repository: &Arc<Repository>) {
    {
        let mut state = lock(&repository.state);
        state.stale = true;
        if state.running {
            // The running refresh will see `stale` when it finishes and go
            // round again. This is the whole of the coalescing: a thousand
            // changes during one read cost one more read, not a thousand.
            return;
        }
        state.running = true;
    }

    let repository = Arc::clone(repository);
    // A plain thread rather than a `tokio` task: this is called from the
    // watcher's own thread, where there is no runtime to spawn onto, and what
    // it does is block on a child process.
    std::thread::spawn(move || refresh_until_settled(&repository));
}

/// Read the status until nothing has changed since the last read started.
///
/// **A read that fails publishes nothing**, which for a subscription that has
/// not had a successful one yet means a panel that stays empty. That is the
/// deliberate end of a trade: the stream's event union has no error variant, so
/// the alternatives were to invent a status the server does not believe or to
/// say nothing. `vcs.refreshStatus` is the door that *can* carry the
/// diagnostic — the panel's own refresh button — and it returns the real
/// refusal. Logged once rather than every [`COALESCE`], or a repository git
/// cannot read would fill the log for as long as the developer kept typing.
fn refresh_until_settled(repository: &Arc<Repository>) {
    loop {
        // Before the read rather than after it, so a burst that is still
        // arriving is gathered into the read that is about to happen.
        std::thread::sleep(COALESCE);
        lock(&repository.state).stale = false;

        match read(repository.path()) {
            Ok(status) => publish(repository, &status),
            Err(why) => {
                let mut state = lock(&repository.state);
                if !state.reported {
                    state.reported = true;
                    eprintln!(
                        "lightcode: cannot read the working tree status of {}: {}",
                        repository.root.display(),
                        why.detail()
                    );
                }
            }
        }

        let mut state = lock(&repository.state);
        if !state.stale {
            state.running = false;
            return;
        }
    }
}

/// Hold a status and tell everyone watching, in that order.
///
/// The order matters. A subscriber that opens between the two sees the held
/// status and then the event that produced it, which is one snapshot twice and
/// lands on the same state; the other order would let a subscriber open with
/// nothing and miss the event that would have filled it.
fn publish(repository: &Arc<Repository>, status: &Status) {
    let event = status.to_snapshot();
    let mut state = lock(&repository.state);
    state.reported = false;
    state.last = Some(event.clone());
    // `send` on a broadcast channel never blocks — it drops the oldest value
    // when the buffer is full, and a subscriber that lags is resent a snapshot
    // instead — so this cannot stall under the lock.
    let _ = repository.events.send(event);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_kept(inner: &Arc<Inner>) -> std::sync::MutexGuard<'_, Vec<Arc<Repository>>> {
    lock(&inner.kept)
}

// ---------------------------------------------------------------------------
// Running git
// ---------------------------------------------------------------------------

/// Why a status could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unavailable {
    /// There is no `git` on this machine.
    NotInstalled,
    /// The call itself cannot be acted on: it named no folder, or a folder that
    /// is not there, or a payload that was not one.
    Unusable { detail: String },
    /// git ran and refused. A repository mid-rebase with a broken index, a
    /// version too old for an option — whatever it was, git said it and the
    /// developer is the one who can act on it.
    Refused {
        detail: String,
        exit_code: Option<i32>,
    },
}

/// The `cwd` every git-shaped call takes, trimmed.
///
/// One function rather than one per call site because the sentence is the same
/// refusal every time, and a `cwd` of `""` is not merely useless — it means the
/// *server process's* own directory to every path API there is, so letting one
/// through would run git somewhere nobody asked about.
pub(crate) fn workspace(raw: &str) -> Result<String, Unavailable> {
    let cwd = raw.trim();
    if cwd.is_empty() {
        return Err(Unavailable::Unusable {
            detail: "This call needs a workspace root; none was given.".to_string(),
        });
    }
    Ok(cwd.to_string())
}

impl Unavailable {
    pub(crate) fn malformed(error: serde_json::Error) -> Unavailable {
        Unavailable::Unusable {
            detail: format!("This call is malformed: {error}"),
        }
    }

    /// The sentence a developer is shown. `GitCommandError` composes its own
    /// message from `operation`, `cwd` and this, so this is the part that says
    /// what actually happened.
    pub fn detail(&self) -> String {
        match self {
            Unavailable::NotInstalled => {
                "git is not installed, or is not on this server's PATH.".to_string()
            }
            Unavailable::Unusable { detail } | Unavailable::Refused { detail, .. } => detail.clone(),
        }
    }

    /// The typed error the client decodes, in the shape `GitCommandError`
    /// declares. No `message`: see [`ERROR`].
    pub fn to_error(&self, operation: &str, cwd: &str) -> Value {
        let mut error = json!({
            "_tag": ERROR,
            "operation": operation,
            "command": "git",
            "cwd": cwd,
            "detail": self.detail(),
        });
        if let Unavailable::Refused {
            exit_code: Some(code),
            ..
        } = self
        {
            error["exitCode"] = json!(code);
        }
        error
    }
}

/// Is there a `git` to run?
///
/// A `PATH` walk rather than a spawn, because this is asked on the read loop by
/// the subscription — which has no other way to report a missing binary, since a
/// stream that has already opened has no error frame in this union to send. The
/// unary refresh does not need it: its own spawn failing says the same thing,
/// with the same [`Unavailable::NotInstalled`] on the other side.
fn installed() -> Option<PathBuf> {
    Search::from_environment().locate("git")
}

/// Run one `git` in `root` and hand back everything it said.
///
/// The one place that knows how this server starts a `git`, which is why
/// [`crate::filesystem`]'s scan comes through here too:
///
/// - **No console window**, or the Tauri shell would flash a black rectangle
///   every time a file changed. See [`crate::process::without_a_console`].
/// - **`--no-optional-locks`**, which is load-bearing rather than tidy. Without
///   it `git status` opportunistically rewrites `.git/index` to refresh its stat
///   cache — a change to a path this module watches, which would mark the
///   repository stale, which would read it again, forever.
/// - **`LC_ALL=C`**, because the one thing here that reads git's own prose is
///   telling "this is not a repository" from "this repository is broken", and a
///   translated message would make every non-English machine report the second.
pub fn output(root: &Path, arguments: &[&str]) -> Result<Output, Unavailable> {
    let mut command = std::process::Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("LC_ALL", "C")
        .env("LANGUAGE", "");
    crate::process::without_a_console(&mut command)
        .output()
        .map_err(|error| match error.kind() {
            // The one a developer can act on, and the only one that means what
            // its sentence says. Everything else — a process table that is
            // full, a binary the machine will not execute — is git failing to
            // start for a reason of its own, and reporting *that* as "git is not
            // installed" would send the developer to install something they
            // already have.
            std::io::ErrorKind::NotFound => Unavailable::NotInstalled,
            _ => Unavailable::Refused {
                detail: format!("git could not be started: {error}"),
                exit_code: None,
            },
        })
}

/// Run one `git` and take its standard output as text, or say why not.
pub(crate) fn text(root: &Path, arguments: &[&str]) -> Result<String, Unavailable> {
    let output = output(root, arguments)?;
    if !output.status.success() {
        return Err(refusal(&output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// How many lines of git's own complaint are carried to the developer.
///
/// Whole lines rather than the first one, because the line a refused
/// `git switch` is worth reading is its *last*: the first names the problem in
/// the abstract, the middle ones name the files in the way, and the last says
/// to commit or stash them. See ADR-0007. Bounded because a switch blocked by a
/// thousand files would otherwise put a thousand paths in an error frame.
const KEPT_LINES: usize = 20;

/// And how much of them, for the case where the lines themselves are long —
/// twenty paths on Windows can be twenty times two hundred and sixty
/// characters.
const KEPT_CHARACTERS: usize = 2_000;

/// What git said when it would not do the thing.
///
/// Git's own words, because they are the only thing that says what is wrong:
/// a repository mid-rebase, a version too old for an option, a switch that
/// would overwrite work. Nothing here interprets them — [`is_not_a_repository`]
/// is the single exception, and it exists because that one case is not a
/// failure at all.
pub(crate) fn refusal(output: &Output) -> Unavailable {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut detail: String = stderr
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .take(KEPT_LINES)
        .collect::<Vec<&str>>()
        .join("\n");
    if detail.is_empty() {
        detail = "git refused without saying why.".to_string();
    }
    if detail.chars().count() > KEPT_CHARACTERS {
        detail = detail.chars().take(KEPT_CHARACTERS).collect::<String>() + "…";
    }
    Unavailable::Refused {
        detail,
        exit_code: output.status.code(),
    }
}

/// Is this refusal the one that means "there is no repository here"?
///
/// The one place anything reads git's prose, and the reason [`output`] pins
/// `LC_ALL=C`: a translated message would make every non-English machine report
/// a perfectly ordinary folder as a broken repository.
pub(crate) fn is_not_a_repository(refused: &Unavailable) -> bool {
    match refused {
        Unavailable::Refused { detail, .. } => detail.contains("not a git repository"),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Reading a status
// ---------------------------------------------------------------------------

/// One project's working tree, as the UI reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Status {
    is_repo: bool,
    has_primary_remote: bool,
    is_default_ref: bool,
    /// `None` on a detached HEAD, which is a repository with no branch rather
    /// than a repository in trouble.
    ref_name: Option<String>,
    /// Sorted by path, and cut at [`MAX_FILES`].
    files: Vec<Change>,
    /// Over *every* changed file, including any past [`MAX_FILES`].
    insertions: u64,
    deletions: u64,
    changed: bool,
    upstream: Option<Upstream>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Change {
    path: String,
    insertions: u64,
    deletions: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Upstream {
    ahead: u64,
    behind: u64,
}

impl Status {
    /// A folder git will answer for, but not as a repository.
    ///
    /// Every flag false and every list empty — which reads the same as a clean
    /// repository except for `isRepo`, and that is the field the UI branches on.
    fn not_a_repository() -> Status {
        Status::default()
    }

    fn to_local(&self) -> Value {
        json!({
            "isRepo": self.is_repo,
            "hasPrimaryRemote": self.has_primary_remote,
            "isDefaultRef": self.is_default_ref,
            "refName": self.ref_name,
            "hasWorkingTreeChanges": self.changed,
            "workingTree": {
                "files": self.files.iter().map(Change::to_value).collect::<Vec<Value>>(),
                "insertions": self.insertions,
                "deletions": self.deletions,
            },
        })
    }

    /// The remote half, or `null` for a folder that is not a repository.
    ///
    /// `aheadOfDefaultCount` is left out rather than sent as zero: the contract
    /// makes it optional, and a zero would be a claim that this branch is level
    /// with the default one rather than an admission that nothing counted.
    fn to_remote(&self) -> Value {
        if !self.is_repo {
            return Value::Null;
        }
        let upstream = self.upstream.unwrap_or_default();
        json!({
            "hasUpstream": self.upstream.is_some(),
            "aheadCount": upstream.ahead,
            "behindCount": upstream.behind,
            "pr": Value::Null,
        })
    }

    /// The stream event.
    ///
    /// Always `snapshot`, never `localUpdated` or `remoteUpdated`. The union's
    /// other two variants exist because upstream's remote half costs a network
    /// round trip and arrives separately; here both halves are read together
    /// from local refs, so every read produces a whole status and a whole status
    /// is a snapshot. The client folds one by replacing what it holds
    /// (`applyGitStatusStreamEvent`), so sending the same snapshot twice lands
    /// on the same state — which is what makes a re-description after a lagged
    /// subscriber safe.
    fn to_snapshot(&self) -> Value {
        json!({
            "_tag": "snapshot",
            "local": self.to_local(),
            "remote": self.to_remote(),
        })
    }

    /// The unary answer, which is the two halves flattened into one object —
    /// exactly what the client's own `mergeGitStatusParts` would have built.
    fn to_result(&self) -> Value {
        let mut result = self.to_local();
        let remote = match self.to_remote() {
            Value::Null => json!({
                "hasUpstream": false,
                "aheadCount": 0,
                "behindCount": 0,
                "pr": Value::Null,
            }),
            remote => remote,
        };
        for (key, value) in remote.as_object().expect("the remote half is an object") {
            result[key] = value.clone();
        }
        result
    }
}

impl Change {
    fn to_value(&self) -> Value {
        json!({
            "path": self.path,
            "insertions": self.insertions,
            "deletions": self.deletions,
        })
    }
}

/// Read one working tree.
///
/// Four short `git` calls rather than one, because git has no single command
/// that answers all of this: what changed, by how much, whether there is a
/// remote, and what this repository considers its default branch. Only the first
/// one costs anything on a large repository, and the coalescing above is what
/// bounds how often the set runs.
fn read(root: &Path) -> Result<Status, Unavailable> {
    let porcelain = output(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            // Without this git names a whole untracked *directory* rather than
            // the files in it, and the contract's `files` are files.
            "--untracked-files=all",
            "-z",
        ],
    )?;
    if !porcelain.status.success() {
        let refused = refusal(&porcelain);
        if is_not_a_repository(&refused) {
            return Ok(Status::not_a_repository());
        }
        return Err(refused);
    }

    let reported = parse_porcelain(&porcelain.stdout);
    let counted = counts(root, &reported);
    let primary = primary_remote(root);
    let default = default_ref(root, primary.as_deref());

    let mut files: Vec<Change> = reported
        .paths
        .iter()
        .map(|path| {
            let (insertions, deletions) = counted.get(path).copied().unwrap_or((0, 0));
            Change {
                path: path.clone(),
                insertions,
                deletions,
            }
        })
        .collect();
    // Sorted so that two reads of an unchanged tree are the same document, and
    // so that the cut below takes an arbitrary tail rather than an arbitrary
    // set. git's own order is the traversal's, which is stable but not
    // meaningful.
    files.sort_by(|left, right| left.path.cmp(&right.path));

    let insertions = files.iter().map(|file| file.insertions).sum();
    let deletions = files.iter().map(|file| file.deletions).sum();
    let changed = !files.is_empty();
    if files.len() > MAX_FILES {
        eprintln!(
            "lightcode: {} has {} changed files; the status names the first {MAX_FILES}",
            root.display(),
            files.len()
        );
        files.truncate(MAX_FILES);
    }

    Ok(Status {
        is_repo: true,
        has_primary_remote: primary.is_some(),
        is_default_ref: match (&reported.ref_name, &default) {
            (Some(current), Some(default)) => current == default,
            _ => false,
        },
        ref_name: reported.ref_name,
        files,
        insertions,
        deletions,
        changed,
        upstream: reported.upstream,
    })
}

/// What `git status --porcelain=v2 --branch -z` said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Porcelain {
    ref_name: Option<String>,
    /// A repository with no commit yet. Its whole index is an addition, and
    /// there is no `HEAD` to diff against.
    unborn: bool,
    upstream: Option<Upstream>,
    /// Every path git named, in the order it named them.
    paths: Vec<String>,
    /// The subset of `paths` that git does not track, which are the ones no
    /// `git diff` will count.
    untracked: Vec<String>,
}

/// Read porcelain v2's records.
///
/// The format is deliberately parseable and this is the whole of it: NUL between
/// records rather than newline, `# ` for the headers `--branch` adds, and a
/// leading character per entry saying which of five shapes it is. Only two of
/// those shapes need care —
///
/// - **`2` (renamed or copied) carries two paths**, and in `-z` mode the second
///   is a record of its own rather than a field inside the first. Reading it as
///   one record would offer the original path as a changed file in its own
///   right.
/// - **`u` (unmerged)** is what a repository stopped half-way through a merge is
///   full of. It has a different number of fields from `1`, so the path cannot
///   be taken from a fixed offset — but it is the last field in both, so it is
///   taken from the end.
fn parse_porcelain(output: &[u8]) -> Porcelain {
    let mut read = Porcelain::default();
    let mut records = records(output);

    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            match header.split_once(' ') {
                Some(("branch.head", "(detached)")) => read.ref_name = None,
                Some(("branch.head", name)) => read.ref_name = Some(name.to_string()),
                Some(("branch.oid", "(initial)")) => read.unborn = true,
                Some(("branch.upstream", _)) => {
                    read.upstream = Some(read.upstream.unwrap_or_default())
                }
                Some(("branch.ab", counts)) => read.upstream = Some(ahead_behind(counts)),
                _ => {}
            }
            continue;
        }

        match record.split_once(' ') {
            Some(("1", rest)) => read.name(path_after(rest, CHANGED_FIELDS), false),
            Some(("2", rest)) => {
                read.name(path_after(rest, RENAMED_FIELDS), false);
                // The original path, which is a record of its own in `-z` mode
                // and is not a changed file.
                records.next();
            }
            Some(("u", rest)) => read.name(path_after(rest, UNMERGED_FIELDS), false),
            Some(("?", path)) => read.name(path.to_string(), true),
            // `!` is an ignored file, which is not asked for, and anything else
            // is a record shape this build does not know. Neither is a reason to
            // abandon the rest of the status.
            _ => {}
        }
    }

    read
}

impl Porcelain {
    /// Record one changed path, **unless it is blank**.
    ///
    /// A record shape this build cannot read leaves nothing after its metadata,
    /// and a blank path is not merely a useless row: `path` is a
    /// `TrimmedNonEmptyString` in the contract, so one empty string would fail
    /// the client's decode of the *whole* status and blank the panel. Dropping
    /// the row costs one file; keeping it costs all of them.
    fn name(&mut self, path: String, untracked: bool) {
        if path.trim().is_empty() {
            return;
        }
        if untracked {
            self.untracked.push(path.clone());
        }
        self.paths.push(path);
    }
}

/// Split a `-z` stream into its records.
///
/// NUL-separated, and unquoted whatever `core.quotePath` says — which is the
/// reason for asking for `-z` in the first place, because the default quoting
/// would mangle every non-ASCII name. Shared by the two things read here, which
/// are different formats that agree about their framing.
fn records(output: &[u8]) -> impl Iterator<Item = String> + '_ {
    output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| String::from_utf8_lossy(record).into_owned())
}

/// `+3 -1`, as git writes the ahead/behind pair.
fn ahead_behind(counts: &str) -> Upstream {
    let mut read = Upstream::default();
    for field in counts.split_whitespace() {
        match field.split_at(1) {
            ("+", count) => read.ahead = count.parse().unwrap_or(0),
            ("-", count) => read.behind = count.parse().unwrap_or(0),
            _ => {}
        }
    }
    read
}

/// How many fields stand between a record's kind and its path.
///
/// Straight from the format, and named rather than inlined because the three
/// numbers being different is the whole reason the path cannot be found by
/// looking at it:
///
/// - `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
/// - `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>`
/// - `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>`
const CHANGED_FIELDS: usize = 7;
const RENAMED_FIELDS: usize = 8;
const UNMERGED_FIELDS: usize = 9;

/// The path at the end of a porcelain entry, after `fields` fixed fields.
///
/// **Counted from the left, never from the right.** Under `-z` git does not
/// quote paths — which is the reason for asking for `-z` at all — so a path may
/// contain spaces, and taking the last space-separated word would silently
/// truncate `my documents/a file.txt` to `file.txt`. Every field before the path
/// is a fixed token with no space in it, so skipping exactly that many is
/// arithmetic rather than a guess, and whatever remains is the path verbatim.
fn path_after(rest: &str, fields: usize) -> String {
    rest.splitn(fields + 1, ' ')
        .nth(fields)
        .unwrap_or_default()
        .to_string()
}

/// How many lines each changed file gained and lost.
///
/// Two sources, because git has two answers. Tracked changes come from
/// `git diff --numstat` against `HEAD`, which covers staged and unstaged
/// together — the developer sees one working tree, not two. Untracked files are
/// invisible to that by definition, so they are counted by reading them, under
/// the budget [`COUNTING_BUDGET`] describes.
///
/// A repository with no commit has no `HEAD` to diff against, and every file in
/// it is an addition, so the diff is skipped entirely and everything is counted
/// from disk.
///
/// **Reading one means resolving where it is, and the project root is not the
/// answer.** Porcelain names every path relative to the *repository* root, which
/// is not the workspace root whenever the developer has opened a package inside
/// a larger repository — a monorepo, which is the case this whole server exists
/// to work in. Joining against the workspace root there would open
/// `packages/web/packages/web/…`, find nothing, and report the agent's new file
/// as `+0`. So the repository root is asked for, once, and only when there is
/// something to count with it.
fn counts(root: &Path, reported: &Porcelain) -> HashMap<String, (u64, u64)> {
    let mut counted = HashMap::new();

    if !reported.unborn {
        if let Ok(numstat) = output(root, &["diff", "--numstat", "-z", "HEAD"]) {
            if numstat.status.success() {
                counted = parse_numstat(&numstat.stdout);
            }
        }
    }

    let uncounted: &[String] = if reported.unborn {
        &reported.paths
    } else {
        &reported.untracked
    };
    if uncounted.is_empty() {
        return counted;
    }

    let repository = work_tree_root(root);
    let mut budget = COUNTING_BUDGET;
    for path in uncounted {
        let added = added_lines(&repository.join(path), &mut budget);
        counted.insert(path.clone(), (added, 0));
    }

    counted
}

/// Where the repository the workspace is in actually starts.
///
/// Falls back to the workspace root, which is the right answer in the common
/// case and no worse than the alternative in every other: a project that *is*
/// the repository root, which is most of them, gets the same path either way.
fn work_tree_root(root: &Path) -> PathBuf {
    match text(root, &["rev-parse", "--show-toplevel"]) {
        Ok(toplevel) if !toplevel.trim().is_empty() => PathBuf::from(toplevel.trim()),
        _ => root.to_path_buf(),
    }
}

/// Read `git diff --numstat -z`.
///
/// Each record is `<insertions> TAB <deletions> TAB <path>`, and there are two
/// cases that are not that:
///
/// - **A binary file** is `- TAB -`, because a binary file has no lines. It
///   reads as nothing gained and nothing lost, which is the honest answer and
///   the one the UI renders as no counts at all.
/// - **A rename** leaves the path field empty and follows with two records of
///   its own, the old path and the new one. The new one is what the status names.
fn parse_numstat(output: &[u8]) -> HashMap<String, (u64, u64)> {
    let mut counted = HashMap::new();
    let mut records = records(output);

    while let Some(record) = records.next() {
        let mut fields = record.splitn(3, '\t');
        let (Some(insertions), Some(deletions), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let counts = (
            insertions.parse().unwrap_or(0),
            deletions.parse().unwrap_or(0),
        );

        if path.is_empty() {
            // A rename: the old path, then the new one.
            let _old = records.next();
            if let Some(new) = records.next() {
                counted.insert(new, counts);
            }
            continue;
        }
        counted.insert(path.to_string(), counts);
    }

    counted
}

/// How many lines a file that git has never seen would add.
///
/// Reads at most [`LONGEST_COUNTED`] of it, and at most what is left of the
/// refresh's budget. A file that is binary — judged the way git judges one, by a
/// NUL in the first stretch — has no lines and counts as none.
fn added_lines(path: &Path, budget: &mut u64) -> u64 {
    use std::io::Read;

    if *budget == 0 {
        return 0;
    }
    let allowed = LONGEST_COUNTED.min(*budget);
    let Ok(file) = std::fs::File::open(path) else {
        // Deleted between git naming it and this reading it, or a name this
        // process may not open. Either way there is nothing to count and
        // nothing a developer could act on.
        return 0;
    };

    let mut contents = Vec::new();
    if file.take(allowed).read_to_end(&mut contents).is_err() {
        return 0;
    }
    *budget = budget.saturating_sub(contents.len() as u64);

    if contents[..contents.len().min(SNIFFED)].contains(&0) {
        return 0;
    }
    if contents.is_empty() {
        return 0;
    }

    let newlines = contents.iter().filter(|byte| **byte == b'\n').count() as u64;
    // A last line with no newline after it is still a line, which is what git
    // counts and what "\ No newline at end of file" in a diff is about.
    if contents.last() == Some(&b'\n') {
        newlines
    } else {
        newlines + 1
    }
}

/// The remote a status is *about*, when there is one.
///
/// `origin` if it is there, and otherwise the first one git lists. "Primary" is
/// the contract's word and it has no configuration behind it in git — there is
/// no such thing as a default remote for a repository, only a default for a
/// branch — so this is the convention every git front end uses.
pub(crate) fn primary_remote(root: &Path) -> Option<String> {
    let listed = text(root, &["remote"]).ok()?;
    let mut names = listed.lines().map(str::trim).filter(|name| !name.is_empty());
    let first = names.next()?.to_string();
    if first == "origin" || listed.lines().any(|name| name.trim() == "origin") {
        return Some("origin".to_string());
    }
    Some(first)
}

/// What this repository considers its default branch.
///
/// Asked of the remote first, because `refs/remotes/<remote>/HEAD` is the only
/// place git records an answer rather than a convention. A repository with no
/// remote — which a project the developer just ran `git init` in is — has only
/// the convention left, so whichever of `main` and `master` actually exists is
/// taken, and a repository with neither has no default branch rather than a
/// guessed one.
pub(crate) fn default_ref(root: &Path, primary: Option<&str>) -> Option<String> {
    if let Some(primary) = primary {
        if let Ok(pointed) = text(
            root,
            &["symbolic-ref", "--short", &format!("refs/remotes/{primary}/HEAD")],
        ) {
            // `origin/main`, or `origin/release/next` — the remote's name and a
            // slash come off the front, and whatever is left is the branch.
            if let Some((_, branch)) = pointed.trim().split_once('/') {
                if !branch.is_empty() {
                    return Some(branch.to_string());
                }
            }
        }
    }

    let conventional = text(
        root,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "refs/heads/main",
            "refs/heads/master",
        ],
    )
    .ok()?;
    conventional
        .lines()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Reading what git says
    // -----------------------------------------------------------------------

    /// Build a `-z` stream the way git does: every record NUL-terminated.
    fn nul(records: &[&str]) -> Vec<u8> {
        let mut written = Vec::new();
        for record in records {
            written.extend_from_slice(record.as_bytes());
            written.push(0);
        }
        written
    }

    /// The four kinds of change the ticket names, straight out of porcelain v2.
    #[test]
    fn modified_added_deleted_and_untracked_files_are_all_read() {
        let read = parse_porcelain(&nul(&[
            "# branch.oid 0d1c2b3a4f5e6d7c8b9a0f1e2d3c4b5a6f7e8d9c",
            "# branch.head main",
            "1 .M N... 100644 100644 100644 aaaa bbbb src/main.rs",
            "1 A. N... 000000 100644 100644 0000 cccc src/added.rs",
            "1 .D N... 100644 100644 000000 dddd dddd README.md",
            "? notes.txt",
        ]));

        assert_eq!(
            read.paths,
            ["src/main.rs", "src/added.rs", "README.md", "notes.txt"]
        );
        assert_eq!(read.untracked, ["notes.txt"]);
        assert_eq!(read.ref_name.as_deref(), Some("main"));
        assert!(!read.unborn);
    }

    /// A rename carries its original path as a record of its own, which is not
    /// a changed file. Reading it as one would offer the developer a path that
    /// no longer exists.
    #[test]
    fn a_renamed_files_original_path_is_not_a_change_of_its_own() {
        let read = parse_porcelain(&nul(&[
            "# branch.head main",
            "2 R. N... 100644 100644 100644 aaaa aaaa R100 src/new-name.rs",
            "src/old-name.rs",
            "? after.txt",
        ]));

        assert_eq!(read.paths, ["src/new-name.rs", "after.txt"]);
    }

    /// A repository stopped half-way through a merge is full of `u` records,
    /// which have more fields than an ordinary change. The ticket asks for this
    /// to be reported rather than to crash.
    #[test]
    fn a_repository_mid_merge_reports_its_unmerged_paths() {
        let read = parse_porcelain(&nul(&[
            "# branch.oid 0d1c2b3a4f5e6d7c8b9a0f1e2d3c4b5a6f7e8d9c",
            "# branch.head main",
            "u UU N... 100644 100644 100644 100644 aaaa bbbb cccc conflict.txt",
        ]));

        assert_eq!(read.paths, ["conflict.txt"]);
    }

    /// A detached HEAD is a repository with no branch, which is a status rather
    /// than a failure.
    #[test]
    fn a_detached_head_has_no_branch_name() {
        let read = parse_porcelain(&nul(&[
            "# branch.oid 0d1c2b3a4f5e6d7c8b9a0f1e2d3c4b5a6f7e8d9c",
            "# branch.head (detached)",
        ]));

        assert_eq!(read.ref_name, None);
        assert!(!read.unborn);
    }

    /// A repository with no commit yet has nothing to diff against, and this is
    /// the flag that says so.
    #[test]
    fn a_repository_with_no_commit_is_unborn() {
        let read = parse_porcelain(&nul(&["# branch.oid (initial)", "# branch.head main"]));

        assert!(read.unborn);
        assert_eq!(read.ref_name.as_deref(), Some("main"));
    }

    /// The tracking counts, and the difference between "no upstream" and "an
    /// upstream we are level with" — which the UI shows differently.
    #[test]
    fn the_tracking_branch_is_read_when_there_is_one() {
        let with = parse_porcelain(&nul(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +3 -1",
        ]));
        assert_eq!(with.upstream, Some(Upstream { ahead: 3, behind: 1 }));

        let level = parse_porcelain(&nul(&[
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +0 -0",
        ]));
        assert_eq!(level.upstream, Some(Upstream::default()));

        let without = parse_porcelain(&nul(&["# branch.head main"]));
        assert_eq!(without.upstream, None);
    }

    /// Paths with spaces in them are not quoted under `-z`, so the path has to
    /// be everything after the metadata rather than the last word.
    #[test]
    fn a_path_with_spaces_survives() {
        let read = parse_porcelain(&nul(&[
            "# branch.head main",
            "1 .M N... 100644 100644 100644 aaaa bbbb my documents/a file.txt",
            "? another file.txt",
        ]));

        assert_eq!(read.paths, ["my documents/a file.txt", "another file.txt"]);
    }

    /// A record shape this build does not know is skipped rather than taken for
    /// a path — the same tolerance the agent protocol has for an event it has
    /// not seen.
    #[test]
    fn an_ignored_file_and_an_unknown_record_are_both_passed_over() {
        let read = parse_porcelain(&nul(&[
            "# branch.head main",
            "! target/debug/build.log",
            "z something from a later git",
            "? real.txt",
        ]));

        assert_eq!(read.paths, ["real.txt"]);
    }

    /// A record of a *known* kind with fewer fields than it should have leaves
    /// nothing after the metadata, and a blank path is worse than a missing row:
    /// `path` is a `TrimmedNonEmptyString`, so one of them would fail the
    /// client's decode of the whole status and blank the panel. One file is the
    /// cost of keeping the other files.
    #[test]
    fn a_truncated_record_costs_its_own_row_and_not_the_status() {
        let read = parse_porcelain(&nul(&[
            "# branch.head main",
            "1 .M N... 100644",
            "?",
            "1 .M N... 100644 100644 100644 aaaa bbbb src/real.rs",
        ]));

        assert_eq!(read.paths, ["src/real.rs"]);
        assert!(read.untracked.is_empty());
    }

    // -----------------------------------------------------------------------
    // Counting lines
    // -----------------------------------------------------------------------

    /// The ordinary case, and the reason there are two sources of counts at
    /// all: this is what git will tell you about a file it already tracks.
    #[test]
    fn numstat_counts_are_read_per_path() {
        let counted = parse_numstat(&nul(&["12\t3\tsrc/main.rs", "0\t40\tREADME.md"]));

        assert_eq!(counted.get("src/main.rs"), Some(&(12, 3)));
        assert_eq!(counted.get("README.md"), Some(&(0, 40)));
    }

    /// A binary file has no lines, and git says so with dashes rather than
    /// numbers. It must not read as a parse failure that drops the file.
    #[test]
    fn a_binary_file_counts_as_no_lines_rather_than_failing() {
        let counted = parse_numstat(&nul(&["-\t-\tlogo.png", "1\t1\tsrc/main.rs"]));

        assert_eq!(counted.get("logo.png"), Some(&(0, 0)));
        assert_eq!(counted.get("src/main.rs"), Some(&(1, 1)));
    }

    /// A rename's counts belong to the new path, and its two paths arrive as
    /// records of their own.
    #[test]
    fn a_renamed_files_counts_land_on_its_new_path() {
        let counted = parse_numstat(&nul(&[
            "2\t1\t",
            "src/old-name.rs",
            "src/new-name.rs",
            "5\t0\tsrc/other.rs",
        ]));

        assert_eq!(counted.get("src/new-name.rs"), Some(&(2, 1)));
        assert_eq!(counted.get("src/old-name.rs"), None);
        assert_eq!(counted.get("src/other.rs"), Some(&(5, 0)));
    }

    /// The other source, and the four things it has to get right about a file
    /// git will not count: a last line with no newline after it is still a
    /// line, an empty file has none, a binary file has none, and a file that
    /// went away between git naming it and this reading it is not an error.
    #[test]
    fn an_untracked_files_lines_are_counted_from_disk() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let mut budget = COUNTING_BUDGET;

        let ended = directory.path().join("ended.txt");
        std::fs::write(&ended, "one\ntwo\nthree\n").expect("writes the file");
        assert_eq!(added_lines(&ended, &mut budget), 3);

        // A last line with no newline after it is still a line.
        let unended = directory.path().join("unended.txt");
        std::fs::write(&unended, "one\ntwo").expect("writes the file");
        assert_eq!(added_lines(&unended, &mut budget), 2);

        let empty = directory.path().join("empty.txt");
        std::fs::write(&empty, "").expect("writes the file");
        assert_eq!(added_lines(&empty, &mut budget), 0);

        // A binary file has no lines, judged the way git judges one.
        let binary = directory.path().join("logo.png");
        std::fs::write(&binary, [0x89, b'P', b'N', b'G', 0x00, 0x0d, b'\n']).expect("writes");
        assert_eq!(added_lines(&binary, &mut budget), 0);

        // A file that went away between git naming it and this reading it.
        assert_eq!(added_lines(&directory.path().join("gone.txt"), &mut budget), 0);
    }

    /// The budget is what keeps a refresh bounded when a project is full of
    /// untracked files. Past it, files count as nothing rather than the read
    /// going on.
    #[test]
    fn counting_stops_when_the_budget_is_spent() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("lines.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("writes the file");

        let mut budget = 14;
        assert_eq!(added_lines(&path, &mut budget), 3);
        assert_eq!(budget, 0);
        assert_eq!(added_lines(&path, &mut budget), 0, "the budget was spent");
    }

    // -----------------------------------------------------------------------
    // What goes on the wire
    // -----------------------------------------------------------------------

    /// A folder that is not a repository is a status, not an error — and the
    /// remote half of one is `null` rather than a set of zeroes claiming there
    /// is a branch level with its upstream.
    #[test]
    fn a_folder_that_is_not_a_repository_describes_itself() {
        let status = Status::not_a_repository();

        assert_eq!(
            status.to_snapshot(),
            json!({
                "_tag": "snapshot",
                "local": {
                    "isRepo": false,
                    "hasPrimaryRemote": false,
                    "isDefaultRef": false,
                    "refName": Value::Null,
                    "hasWorkingTreeChanges": false,
                    "workingTree": {"files": [], "insertions": 0, "deletions": 0},
                },
                "remote": Value::Null,
            })
        );

        // The unary answer has no nullable remote half, so the client's own
        // empty remote is what it carries.
        let result = status.to_result();
        assert_eq!(result["isRepo"], json!(false));
        assert_eq!(result["hasUpstream"], json!(false));
        assert_eq!(result["aheadCount"], json!(0));
        assert_eq!(result["pr"], Value::Null);
    }

    /// Every field the contract requires, in the spelling it requires, for a
    /// repository that has something to say.
    #[test]
    fn a_status_serializes_to_the_contracts_shape() {
        let status = Status {
            is_repo: true,
            has_primary_remote: true,
            is_default_ref: false,
            ref_name: Some("feature/status".to_string()),
            files: vec![Change {
                path: "src/main.rs".to_string(),
                insertions: 12,
                deletions: 3,
            }],
            insertions: 12,
            deletions: 3,
            changed: true,
            upstream: Some(Upstream { ahead: 2, behind: 0 }),
        };

        assert_eq!(
            status.to_result(),
            json!({
                "isRepo": true,
                "hasPrimaryRemote": true,
                "isDefaultRef": false,
                "refName": "feature/status",
                "hasWorkingTreeChanges": true,
                "workingTree": {
                    "files": [{"path": "src/main.rs", "insertions": 12, "deletions": 3}],
                    "insertions": 12,
                    "deletions": 3,
                },
                "hasUpstream": true,
                "aheadCount": 2,
                "behindCount": 0,
                "pr": Value::Null,
            })
        );
    }

    /// `aheadOfDefaultCount` is optional and nothing here counts it, so it is
    /// absent rather than zero — a zero would be a claim.
    #[test]
    fn a_count_nothing_produced_is_absent_rather_than_zero() {
        let status = Status {
            is_repo: true,
            ..Status::default()
        };
        assert!(status.to_result().get("aheadOfDefaultCount").is_none());
    }

    // -----------------------------------------------------------------------
    // Refusals
    // -----------------------------------------------------------------------

    /// The diagnostic for a machine with no git. It is the one failure here
    /// that a developer cannot work around by fixing their repository, so the
    /// sentence has to name the actual problem.
    #[test]
    fn a_missing_git_binary_is_a_declared_error_that_names_it() {
        let error = Unavailable::NotInstalled.to_error(REFRESH_STATUS, r"C:\project");

        assert_eq!(error["_tag"], ERROR);
        assert_eq!(error["operation"], REFRESH_STATUS);
        assert_eq!(error["command"], "git");
        assert_eq!(error["cwd"], r"C:\project");
        let detail = error["detail"].as_str().expect("a detail");
        assert!(detail.contains("git"), "{detail}");
        assert!(detail.contains("PATH"), "{detail}");

        // `message` is a getter on the client's own class, so sending one would
        // be sending a field the reference server does not.
        assert!(error.get("message").is_none());
    }

    /// A repository git refuses to read keeps git's own sentence, because that
    /// is the only thing that says what is wrong with it.
    #[test]
    fn a_refusal_keeps_gits_own_first_line_and_exit_code() {
        let refused = Unavailable::Refused {
            detail: "fatal: not a valid object name: HEAD".to_string(),
            exit_code: Some(128),
        };
        let error = refused.to_error(SUBSCRIBE_STATUS, r"C:\project");

        assert_eq!(error["detail"], "fatal: not a valid object name: HEAD");
        assert_eq!(error["exitCode"], json!(128));
    }

    /// Both ways a call can arrive without naming a folder, because they are
    /// different bugs on the other end and neither may reach the disk: a `cwd`
    /// of `""` means the process's own directory, and an absent one would mean
    /// whatever `serde` made of it.
    #[test]
    fn a_call_without_a_workspace_root_is_refused_before_anything_runs() {
        let error = StatusCall::read(&json!({"cwd": "   "}), REFRESH_STATUS)
            .expect_err("a blank workspace root is not a workspace root");
        assert_eq!(error["_tag"], ERROR);

        let error = StatusCall::read(&json!({}), SUBSCRIBE_STATUS)
            .expect_err("a payload with no cwd at all");
        assert_eq!(error["_tag"], ERROR);
    }

    // -----------------------------------------------------------------------
    // Which changes matter
    // -----------------------------------------------------------------------

    /// The rule that keeps a build from refreshing a status thousands of times,
    /// and the one that keeps this module out of a loop with itself.
    #[test]
    fn only_changes_that_could_move_a_status_are_acted_on() {
        for interesting in [
            "",
            "src/main.rs",
            "node_modules/left-pad/index.js",
            ".git/HEAD",
            ".git/index",
            ".git/MERGE_HEAD",
            ".git/refs/heads/main",
        ] {
            assert!(affects_status(interesting), "{interesting}");
        }

        for ignored in [
            ".git/objects/ab/cdef",
            ".git/logs/HEAD",
            ".git/index.lock",
            ".git/refs/heads/main.lock",
        ] {
            assert!(!affects_status(ignored), "{ignored}");
        }
    }

    // -----------------------------------------------------------------------
    // The registry
    // -----------------------------------------------------------------------

    /// A workspace is kept once however many windows are looking at it, and a
    /// second look moves it to the back of the eviction order rather than
    /// adding an entry.
    #[test]
    fn a_workspace_is_kept_once() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let index = Index::new();
        let repositories = Repositories::new(&index);
        let root = WorkspaceRoot::check(&directory.path().to_string_lossy()).expect("a workspace");

        let first = repositories.remember(&root);
        let second = repositories.remember(&root);

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(repositories.kept(), 1);
    }

    /// Past the ceiling the least recently subscribed workspace is dropped, for
    /// the reason the watcher's own ceiling exists: without one, the number of
    /// working trees this server keeps is decided by whatever is on the other
    /// end of the socket.
    #[test]
    fn past_the_ceiling_the_least_recently_subscribed_workspace_is_dropped() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let index = Index::new();
        let repositories = Repositories::new(&index);
        let root = |name: &str| {
            let path = directory.path().join(name);
            std::fs::create_dir_all(&path).expect("creates the directory");
            WorkspaceRoot::check(&path.to_string_lossy()).expect("a workspace")
        };

        let working_in = root("the-one-being-used");
        repositories.remember(&working_in);
        for index in 0..MAX_WATCHED - 1 {
            repositories.remember(&root(&format!("abandoned-{index}")));
        }
        assert_eq!(repositories.kept(), MAX_WATCHED);

        // Looking at the real project again keeps it, and the next arrival
        // displaces an abandoned one instead.
        repositories.remember(&working_in);
        repositories.remember(&root("later"));

        assert_eq!(repositories.kept(), MAX_WATCHED);
        assert!(
            repositories.inner.find(working_in.canonical()).is_some(),
            "the project being worked in was dropped in favour of abandoned folders"
        );
    }
}
