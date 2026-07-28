//! Refs: the branches a project has, the one it is on, and the repository it
//! does not have yet.
//!
//! Four method tags land here and they are one job: keeping a developer out of
//! a shell. `vcs.listRefs` is the branch picker, `vcs.switchRef` is choosing
//! from it, `vcs.createRef` is starting work that does not have a branch yet,
//! and `vcs.init` is the project that is not a repository at all — which
//! [`crate::git`] already reports as a status rather than as a failure,
//! precisely so that this call has somewhere to land.
//!
//! Git is driven by shelling out, the same as the status is, and through the
//! same [`crate::git::output`] — which is where `--no-optional-locks`, the
//! suppressed console window and `LC_ALL=C` live. Nothing here re-implements
//! any of that.
//!
//! ## Every call here changes the answer another subsystem is publishing
//!
//! A switch moves `HEAD`, which changes the branch the status panel shows and
//! usually the files it lists. An init turns "this is not a repository" into a
//! status. So each of the three calls that change something tells
//! [`crate::git::Repositories`] the working tree is stale, and the refresh that
//! already exists publishes the new status to whoever is watching. The watcher
//! would eventually notice `.git/HEAD` moving on its own; saying so directly is
//! what makes "switch, then read the panel" a sequence rather than a race.
//!
//! ## Where the work happens
//!
//! All four are unary and all four run git, so all four are
//! [`crate::rpc::Deferred`] — off the connection's read loop, like every other
//! method that waits on the world. None of them streams.
//!
//! ## What is checked here rather than by git
//!
//! **Branch names are validated before git sees one.** See ADR-0007; the short
//! version is that `git branch` refuses a bad name with `check-ref-format`'s
//! own vocabulary, which describes a *ref* and not the branch the developer
//! typed, and that a name arriving from a text field is the one input here that
//! is neither a path nor a flag.
//!
//! What is deliberately *not* checked here is whether a switch is safe. git
//! already refuses a switch that would overwrite uncommitted work, names the
//! files, and says what to do about it — so that refusal is carried through
//! verbatim rather than pre-empted by a worse sentence of this module's own.
//!
//! Shapes are hand-written from `VcsListRefsInput`, `VcsListRefsResult`,
//! `VcsRef`, `VcsCreateRefInput`, `VcsCreateRefResult`, `VcsSwitchRefInput`,
//! `VcsSwitchRefResult` and `VcsInitInput` in
//! `t3code/packages/contracts/src/git.ts`, and the errors from `GitCommandError`
//! in the same file and the `VcsError` union in `vcs.ts`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::git::{self, Repositories, Unavailable};
use crate::projects::WorkspaceRoot;

/// The branch picker.
pub const LIST_REFS: &str = "vcs.listRefs";

/// Starting work on a branch that does not exist yet.
pub const CREATE_REF: &str = "vcs.createRef";

/// Moving the working tree to another branch.
pub const SWITCH_REF: &str = "vcs.switchRef";

/// Making a repository in a project that has none.
pub const INIT: &str = "vcs.init";

/// The most refs one listing will consider.
///
/// `totalCount` is counted over what was considered, so this is a ceiling on
/// the answer and not only on the work. A repository with ten thousand refs is
/// one with a bot pushing branches into it, and the hundred a page carries were
/// never going to be found by scrolling anyway — the `query` is what finds a
/// branch there. A cut is logged, the way the status logs its own.
const MAX_REFS: usize = 10_000;

/// The most refs one page will carry — `GIT_LIST_BRANCHES_MAX_LIMIT` in the
/// contract, and the default when the client does not ask for fewer.
const MAX_LIMIT: usize = 200;

/// The longest any one part of a branch name may be.
///
/// git stores a branch as a file under `.git/refs/heads`, so each part between
/// slashes is a filename, and every filesystem this server runs on stops at 255
/// bytes. This is the one length rule that is git's — by way of the disk — and
/// it is per part rather than per name, which is why `a/b/c/…` may be far
/// longer than any of its parts.
const LONGEST_PART: usize = 255;

/// The longest a whole branch name may be.
///
/// **Not a rule of git's.** git's own ceiling is the filesystem's path limit,
/// which varies by platform and by how deep the repository is. Having *a*
/// number is what keeps a string that reaches a command line bounded, and this
/// one is far past anything a developer types — so it is a backstop rather than
/// a validation, and it is deliberately generous enough that no name a listing
/// returned could fail it.
const LONGEST_NAME: usize = 1_024;

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// A validated `vcs.listRefs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRefs {
    cwd: String,
    /// What the developer has typed into the picker, lower-cased once here so
    /// that matching is not doing it per ref.
    query: Option<String>,
    cursor: usize,
    limit: usize,
    kind: Kind,
    /// Whether a remote ref that has a local branch of the same name is worth
    /// listing. Normally it is not: `origin/main` beside `main` is one row that
    /// says nothing and one row that is the branch the developer wants.
    include_matching_remote_refs: bool,
}

/// Which refs a listing is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Kind {
    #[default]
    All,
    Local,
    Remote,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListRefsPayload {
    cwd: String,
    query: Option<String>,
    cursor: Option<u64>,
    limit: Option<u64>,
    ref_kind: Option<String>,
    include_matching_remote_refs: Option<bool>,
}

impl ListRefs {
    pub fn read(payload: &Value) -> Result<ListRefs, Value> {
        let read: ListRefsPayload = serde_json::from_value(payload.clone())
            .map_err(|error| Unavailable::malformed(error).to_error(LIST_REFS, ""))?;
        let cwd = workspace(&read.cwd, LIST_REFS)?;
        Ok(ListRefs {
            cwd,
            query: read
                .query
                .map(|query| query.trim().to_lowercase())
                .filter(|query| !query.is_empty()),
            cursor: read.cursor.unwrap_or(0) as usize,
            // Clamped rather than refused: the contract already bounds this on
            // the client, so a number outside the range is a client this server
            // has not met rather than a developer's mistake, and a page of 200
            // is a better answer to one than an error is.
            limit: (read.limit.unwrap_or(MAX_LIMIT as u64) as usize).clamp(1, MAX_LIMIT),
            kind: match read.ref_kind.as_deref() {
                Some("local") => Kind::Local,
                Some("remote") => Kind::Remote,
                _ => Kind::All,
            },
            include_matching_remote_refs: read.include_matching_remote_refs.unwrap_or(false),
        })
    }

    pub fn run(self) -> Result<Value, Value> {
        let root = root(&self.cwd, LIST_REFS)?;
        let listing = read_refs(root.path(), &self)
            .map_err(|why| why.to_error(LIST_REFS, &self.cwd))?;
        Ok(self.page(listing))
    }

    /// Cut the listing down to the page that was asked for.
    ///
    /// `totalCount` is the whole filtered list and `nextCursor` is where the
    /// next page starts, so a picker that has scrolled knows both how far it
    /// has got and whether there is more.
    fn page(&self, listing: Listing) -> Value {
        let total = listing.refs.len();
        let start = self.cursor.min(total);
        let end = start.saturating_add(self.limit).min(total);
        json!({
            "refs": listing.refs[start..end]
                .iter()
                .map(Ref::to_value)
                .collect::<Vec<Value>>(),
            "isRepo": listing.is_repo,
            "hasPrimaryRemote": listing.has_primary_remote,
            "nextCursor": match end < total {
                true => json!(end),
                false => Value::Null,
            },
            "totalCount": total,
        })
    }
}

/// Every ref a listing found, before it is cut into pages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Listing {
    refs: Vec<Ref>,
    is_repo: bool,
    has_primary_remote: bool,
}

/// One branch, as the picker reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ref {
    /// `main`, or `origin/main` for a remote one — the short name, which is
    /// what the developer sees and what a switch is asked for by.
    name: String,
    /// The remote it belongs to, for a remote ref.
    remote: Option<String>,
    /// Checked out **here**, in this call's workspace. A branch checked out in
    /// another worktree is not current; it has a `worktree_path` instead.
    current: bool,
    /// This repository's default branch, or the remote ref that tracks it.
    default: bool,
    /// Where this branch is checked out, if it is anywhere. The client uses it
    /// as a `cwd`, which is why it is git's own spelling of the path rather
    /// than one composed here.
    worktree_path: Option<String>,
}

impl Ref {
    fn to_value(&self) -> Value {
        let mut value = json!({
            "name": self.name,
            "isRemote": self.remote.is_some(),
            "current": self.current,
            "isDefault": self.default,
            "worktreePath": self.worktree_path,
        });
        // Only for a remote ref. The capture sends `isRemote` on every ref and
        // `remoteName` on none of the local ones, and the field is optional.
        if let Some(remote) = &self.remote {
            value["remoteName"] = json!(remote);
        }
        value
    }

    /// The branch this ref is *of* — `origin/main` is a ref about `main`.
    ///
    /// What folds a remote ref against a local one, and what decides whether a
    /// remote ref is the default branch's.
    fn branch(&self) -> &str {
        match &self.remote {
            Some(remote) => self.name.get(remote.len() + 1..).unwrap_or(&self.name),
            None => &self.name,
        }
    }
}

/// Read every ref, and everything that qualifies one.
///
/// Five short `git` calls, and the first one is the one that decides whether
/// there is anything to say at all: a folder that is not a repository is a
/// listing with no refs rather than a failure, for the same reason a status of
/// one is a status — it is what the developer sees before they press
/// "initialise", and an error there would be the app refusing to describe a
/// perfectly ordinary folder.
fn read_refs(root: &Path, call: &ListRefs) -> Result<Listing, Unavailable> {
    let listed = git::output(
        root,
        &[
            "for-each-ref",
            "--format=%(refname)%09%(symref)",
            // The last key is the primary one, so this is "most recently
            // committed on first, ties broken by name" — a picker's order,
            // and a *total* one, which pagination needs.
            "--sort=refname",
            "--sort=-committerdate",
            &format!("--count={MAX_REFS}"),
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    if !listed.status.success() {
        let refused = git::refusal(&listed);
        if git::is_not_a_repository(&refused) {
            return Ok(Listing::default());
        }
        return Err(refused);
    }

    let primary = git::primary_remote(root);
    let bearings = Bearings {
        default: git::default_ref(root, primary.as_deref()),
        current: current_branch(root),
        worktrees: worktrees(root),
        remotes: remotes(root),
    };

    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    if listed.lines().count() >= MAX_REFS {
        eprintln!(
            "laplus: {} has at least {MAX_REFS} refs; the listing considers the first {MAX_REFS}",
            root.display()
        );
    }

    let mut found: Vec<Ref> = Vec::new();
    for line in listed.lines() {
        // `<refname> TAB <symref>`. A ref name cannot contain a tab — git's own
        // `check-ref-format` forbids every control character — so one field is
        // one split.
        let (refname, symref) = line.split_once('\t').unwrap_or((line, ""));
        // `refs/remotes/origin/HEAD` is a symbolic ref that points at the
        // remote's default branch. It is a pointer and not a branch, and
        // listing it would offer the developer a row that duplicates another.
        if !symref.is_empty() {
            continue;
        }
        if let Some(entry) = bearings.describe(refname) {
            found.push(entry);
        }
    }

    // A repository with no commit yet has a `HEAD` naming a branch that has no
    // ref behind it — which is every project the moment after `vcs.init`. The
    // status reports that branch, so the picker has to as well, or the two
    // would disagree about what the developer is on.
    if let Some(unborn) = bearings.unborn(&found) {
        found.insert(0, unborn);
    }

    Ok(Listing {
        refs: select(found, call),
        is_repo: true,
        has_primary_remote: primary.is_some(),
    })
}

/// What a repository knows about itself while its refs are being read.
///
/// Four facts, none of them about any one ref and all of them needed to
/// describe every ref, so they are read once and travel together rather than
/// as four arguments that must not be given in the wrong order.
#[derive(Debug, Default)]
struct Bearings {
    /// Every remote, for telling a remote-tracking ref's remote from its
    /// branch. See [`Bearings::split_remote`].
    remotes: Vec<String>,
    /// Which branch is checked out where, by full ref name.
    worktrees: HashMap<String, String>,
    current: Option<String>,
    default: Option<String>,
}

impl Bearings {
    /// Turn one `refs/…` name into a ref, or decide it is not one.
    fn describe(&self, refname: &str) -> Option<Ref> {
        let worktree_path = self.worktrees.get(refname).cloned();

        if let Some(name) = refname.strip_prefix("refs/heads/") {
            return Some(Ref {
                current: self.current.as_deref() == Some(name),
                default: self.default.as_deref() == Some(name),
                name: name.to_string(),
                remote: None,
                worktree_path,
            });
        }

        let name = refname.strip_prefix("refs/remotes/")?;
        let (remote, branch) = self.split_remote(name)?;
        Some(Ref {
            default: self.default.as_deref() == Some(branch),
            remote: Some(remote.to_string()),
            name: name.to_string(),
            current: false,
            worktree_path,
        })
    }

    /// Split a remote-tracking ref's short name into the remote it belongs to
    /// and the branch it is of.
    ///
    /// **Not "everything before the first slash".** A remote may be called
    /// `origin/mirror` and a branch may be called `feature/x`, so only the
    /// remotes this repository actually has can tell the two apart: the longest
    /// one that prefixes the name wins. A name under no remote at all is not a
    /// branch anybody can switch to, which is why this can answer with nothing.
    fn split_remote<'a>(&'a self, name: &'a str) -> Option<(&'a str, &'a str)> {
        let remote = self
            .remotes
            .iter()
            .filter(|remote| name.starts_with(&format!("{remote}/")))
            .max_by_key(|remote| remote.len())?;
        Some((remote, &name[remote.len() + 1..]))
    }

    /// The branch `HEAD` names that has no ref behind it, if there is one.
    fn unborn(&self, found: &[Ref]) -> Option<Ref> {
        let branch = self.current.as_ref()?;
        if found
            .iter()
            .any(|entry| entry.remote.is_none() && &entry.name == branch)
        {
            return None;
        }
        Some(Ref {
            name: branch.clone(),
            remote: None,
            current: true,
            default: self.default.as_ref() == Some(branch),
            worktree_path: self.worktrees.get(&format!("refs/heads/{branch}")).cloned(),
        })
    }
}

/// Keep the refs this call asked about, in the order a picker wants them.
///
/// Three filters and one hoist:
///
/// - **the kind**, which is the client saying it wants only one side;
/// - **the fold**, which drops a remote ref that has a local branch of the
///   same name — `origin/main` beside `main` is a row that adds nothing, and
///   `includeMatchingRemoteRefs` is the client asking for it anyway. It is
///   about a *pair* of rows, so it only applies when both sides are being
///   listed: folding a `refKind: "remote"` listing would answer "what is on the
///   remote" with the branches nobody has checked out, and an ordinary clone
///   would answer it with nothing at all;
/// - **the query**, which is the developer typing into the picker.
///
/// Then the branch the developer is on goes first and the default branch
/// second, because those are the two rows anybody scrolling is looking for.
/// Everything after them keeps git's order, which is the sort asked for above
/// and is total — pagination over a partial order would repeat and skip rows.
fn select(found: Vec<Ref>, call: &ListRefs) -> Vec<Ref> {
    let locals: HashSet<&str> = found
        .iter()
        .filter(|entry| entry.remote.is_none())
        .map(|entry| entry.name.as_str())
        .collect();

    let mut kept: Vec<Ref> = found
        .iter()
        .filter(|entry| match call.kind {
            Kind::All => true,
            Kind::Local => entry.remote.is_none(),
            Kind::Remote => entry.remote.is_some(),
        })
        .filter(|entry| {
            call.kind != Kind::All
                || call.include_matching_remote_refs
                || entry.remote.is_none()
                || !locals.contains(entry.branch())
        })
        .filter(|entry| match &call.query {
            None => true,
            Some(query) => entry.name.to_lowercase().contains(query),
        })
        .cloned()
        .collect();

    kept.sort_by_key(|entry| match (entry.current, entry.default) {
        (true, _) => 0,
        (false, true) => 1,
        (false, false) => 2,
    });
    kept
}

/// The branch checked out here, or nothing on a detached `HEAD`.
///
/// Answers for a repository with no commit yet too, where `HEAD` names a branch
/// that does not exist — which is exactly the state `vcs.init` leaves behind.
fn current_branch(root: &Path) -> Option<String> {
    let named = git::text(root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()?;
    let named = named.trim();
    (!named.is_empty()).then(|| named.to_string())
}

/// Which branch is checked out where, by full ref name.
///
/// A branch checked out in another worktree cannot be switched to here — git
/// refuses — so this is not decoration: it is what lets the picker say why.
fn worktrees(root: &Path) -> HashMap<String, String> {
    let Ok(listed) = git::text(root, &["worktree", "list", "--porcelain"]) else {
        return HashMap::new();
    };

    let mut found = HashMap::new();
    let mut at: Option<String> = None;
    for line in listed.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            at = Some(path.trim().to_string());
        } else if let Some(refname) = line.strip_prefix("branch ") {
            if let Some(path) = &at {
                found.insert(refname.trim().to_string(), path.clone());
            }
        }
    }
    found
}

/// Every remote this repository has, longest name last is nobody's business —
/// the order is git's and the only use is prefix matching.
fn remotes(root: &Path) -> Vec<String> {
    git::text(root, &["remote"])
        .map(|listed| {
            listed
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Creating
// ---------------------------------------------------------------------------

/// A validated `vcs.createRef`. The name has already passed [`check_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRef {
    cwd: String,
    name: String,
    switch: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRefPayload {
    cwd: String,
    ref_name: String,
    switch_ref: Option<bool>,
}

impl CreateRef {
    pub fn read(payload: &Value) -> Result<CreateRef, Value> {
        let read: CreateRefPayload = serde_json::from_value(payload.clone())
            .map_err(|error| Unavailable::malformed(error).to_error(CREATE_REF, ""))?;
        let (cwd, name) = workspace_and_name(&read.cwd, &read.ref_name, CREATE_REF)?;
        Ok(CreateRef {
            cwd,
            name,
            switch: read.switch_ref.unwrap_or(false),
        })
    }

    /// Make the branch at the current position, and move to it if asked.
    pub fn run(self, repositories: &Repositories) -> Result<Value, Value> {
        let root = root(&self.cwd, CREATE_REF)?;
        let path = root.path();

        // Asked before git, so that the sentence names the branch rather than
        // the ref. `git branch` says "a branch named 'x' already exists"; this
        // says the same thing plus what to do instead, and it says it without
        // depending on git's own wording staying put.
        if exists(path, &format!("refs/heads/{}", self.name)) {
            return Err(Unavailable::Unusable {
                detail: format!(
                    "A branch named '{}' already exists in this repository. Switch to it, \
                     or choose another name.",
                    self.name
                ),
            }
            .to_error(CREATE_REF, &self.cwd));
        }

        // A repository with no commit yet is the state `vcs.init` leaves
        // behind, and it is the one place "from the current position" has no
        // answer: there is no commit for a second branch to point at. Moving
        // *to* the new name still works, because an unborn branch is only a
        // name in `HEAD` and renaming it costs nothing — so only the case that
        // cannot work is refused, and it is refused with the reason rather
        // than with git's `not a valid object name: 'HEAD'`.
        if !self.switch && !has_a_commit(path) {
            return Err(Unavailable::Unusable {
                detail: format!(
                    "This repository has no commits yet, so there is no position to make \
                     '{}' at. Commit something first, or switch to the new branch as it \
                     is created.",
                    self.name
                ),
            }
            .to_error(CREATE_REF, &self.cwd));
        }

        let arguments: Vec<&str> = if self.switch {
            vec!["switch", "--create", &self.name]
        } else {
            vec!["branch", &self.name]
        };
        change(path, &arguments).map_err(|why| why.to_error(CREATE_REF, &self.cwd))?;

        // Only a switch changes what the status says. Making a branch and
        // staying put moves nothing a panel is showing.
        if self.switch {
            repositories.disturb(&root);
        }
        Ok(json!({"refName": self.name}))
    }
}

// ---------------------------------------------------------------------------
// Switching
// ---------------------------------------------------------------------------

/// A validated `vcs.switchRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchRef {
    cwd: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchRefPayload {
    cwd: String,
    ref_name: String,
}

impl SwitchRef {
    pub fn read(payload: &Value) -> Result<SwitchRef, Value> {
        let read: SwitchRefPayload = serde_json::from_value(payload.clone())
            .map_err(|error| Unavailable::malformed(error).to_error(SWITCH_REF, ""))?;
        let (cwd, name) = workspace_and_name(&read.cwd, &read.ref_name, SWITCH_REF)?;
        Ok(SwitchRef { cwd, name })
    }

    /// Move the working tree to another branch.
    ///
    /// **A switch that would lose uncommitted work is git's refusal, carried
    /// through.** Nothing here passes `--force` or `--discard-changes`, and
    /// nothing pre-empts the check: git names the files that are in the way and
    /// says to commit or stash them, which is a better sentence than this
    /// module could compose and is the one the ticket asks for.
    pub fn run(self, repositories: &Repositories) -> Result<Value, Value> {
        let root = root(&self.cwd, SWITCH_REF)?;
        let path = root.path();

        let arguments = self
            .target(path)
            .map_err(|why| why.to_error(SWITCH_REF, &self.cwd))?;
        let borrowed: Vec<&str> = arguments.iter().map(String::as_str).collect();
        change(path, &borrowed).map_err(|why| why.to_error(SWITCH_REF, &self.cwd))?;

        repositories.disturb(&root);
        // Read back rather than assumed. The contract's result is nullable
        // because a switch can land on no branch at all, and the only thing
        // that knows where the working tree actually ended up is the working
        // tree.
        Ok(json!({"refName": current_branch(path)}))
    }

    /// What to run to get onto this ref.
    ///
    /// Three cases, and the second is the one that makes a picker showing
    /// remote branches useful rather than decorative: `origin/feature` is not
    /// something a working tree can be *on*, so switching to it means making
    /// the local branch that tracks it.
    ///
    /// Which part of `origin/feature` is the branch is asked of the repository
    /// rather than of the string, by the same [`Bearings::split_remote`] the
    /// listing used to produce the name — a remote called `origin/mirror` would
    /// otherwise turn a switch to `origin/mirror/main` into a local branch
    /// called `mirror/main`.
    fn target(&self, root: &Path) -> Result<Vec<String>, Unavailable> {
        if exists(root, &format!("refs/heads/{}", self.name)) {
            return Ok(vec!["switch".to_string(), self.name.clone()]);
        }

        if exists(root, &format!("refs/remotes/{}", self.name)) {
            let bearings = Bearings {
                remotes: remotes(root),
                ..Bearings::default()
            };
            let branch = match bearings.split_remote(&self.name) {
                Some((_, branch)) => branch.to_string(),
                // A ref under `refs/remotes` that belongs to no remote this
                // repository has. Nothing can be inferred about which part of
                // it is a branch, so it is taken whole and git decides.
                None => self.name.clone(),
            };
            if exists(root, &format!("refs/heads/{branch}")) {
                return Ok(vec!["switch".to_string(), branch]);
            }
            return Ok(vec![
                "switch".to_string(),
                "--create".to_string(),
                branch,
                "--track".to_string(),
                self.name.clone(),
            ]);
        }

        Err(Unavailable::Unusable {
            detail: format!(
                "There is no branch named '{}' in this repository.",
                self.name
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Initialising
// ---------------------------------------------------------------------------

/// A validated `vcs.init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Init {
    cwd: String,
    driver: Driver,
}

/// Which version control system a call is about — the contract's
/// `VcsDriverKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Driver {
    #[default]
    Git,
    /// The one this server has to refuse, and the reason this is a type rather
    /// than a string: the union has a declared error for it.
    Jj,
    /// A client that could not tell — and a value this build has not seen,
    /// which arrives the same way. Read as git, because git is the only thing
    /// here that could make the repository being asked for.
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitPayload {
    cwd: String,
    kind: Option<String>,
}

impl Init {
    pub fn read(payload: &Value) -> Result<Init, Value> {
        let read: InitPayload = serde_json::from_value(payload.clone())
            .map_err(|error| detection_failed("", format!("This call is malformed: {error}")))?;
        let cwd = read.cwd.trim();
        if cwd.is_empty() {
            return Err(detection_failed(
                "",
                "This call needs a workspace root; none was given.",
            ));
        }
        Ok(Init {
            cwd: cwd.to_string(),
            driver: match read.kind.as_deref() {
                Some("jj") => Driver::Jj,
                Some("unknown") => Driver::Unknown,
                _ => Driver::Git,
            },
        })
    }

    /// Make a repository, and let the status panel find out.
    ///
    /// `git init` in a folder that is already a repository re-initialises it
    /// and succeeds, and that is left alone rather than turned into a refusal:
    /// the button that sends this is only shown when the status says `isRepo`
    /// is false, so a second one is a stale window rather than a mistake, and
    /// answering it with an error would be the app arguing with itself.
    ///
    /// **Answers with `null`.** `vcs.init` declares no success value, and
    /// `Schema.Void` encodes to `null` over this wire — the same thing
    /// `terminal.write` answers with.
    pub fn run(self, repositories: &Repositories) -> Result<Value, Value> {
        if self.driver == Driver::Jj {
            return Err(json!({
                "_tag": "VcsUnsupportedOperationError",
                "operation": INIT,
                "kind": "jj",
                "detail": "This server drives git and nothing else. A jj repository has to be \
                           created outside it.",
            }));
        }

        let root = WorkspaceRoot::check(&self.cwd)
            .map_err(|rejection| detection_failed(&self.cwd, rejection.message()))?;

        let output = git::output(root.path(), &["init"]).map_err(|why| {
            json!({
                "_tag": "VcsProcessSpawnError",
                "operation": INIT,
                "command": "git",
                "cwd": self.cwd,
                "cause": why.detail(),
            })
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(json!({
                "_tag": "VcsProcessExitError",
                "operation": INIT,
                "command": "git",
                "cwd": self.cwd,
                "exitCode": output.status.code().unwrap_or(-1),
                "detail": git::refusal(&output).detail(),
                "failureKind": "command-failed",
                "stderrLength": stderr.len(),
            }));
        }

        // The folder that was not a repository is one now, and the panel is
        // showing the answer from before it was.
        repositories.disturb(&root);
        Ok(Value::Null)
    }
}

/// `VcsRepositoryDetectionError` — the one member of the `VcsError` union that
/// carries a free-form sentence about a *place* rather than about a process.
///
/// No `message`: it is a getter over the declared fields on the client's own
/// class, the same rule every other error on this wire follows.
fn detection_failed(cwd: &str, detail: impl std::fmt::Display) -> Value {
    json!({
        "_tag": "VcsRepositoryDetectionError",
        "operation": INIT,
        "cwd": cwd,
        "detail": detail.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Branch names
// ---------------------------------------------------------------------------

/// The characters git will not have in a ref name, whatever else is true.
///
/// Straight out of `git-check-ref-format(1)`. A space is in the list for the
/// same reason the rest are: git refuses it, and a name with one in would
/// otherwise reach a command line.
const FORBIDDEN: [char; 8] = [' ', '~', '^', ':', '?', '*', '[', '\\'];

/// Check a branch name the way `git check-ref-format --branch` does, and say
/// what is wrong with it in the developer's vocabulary rather than in git's.
///
/// See ADR-0007. Every rule here is git's; what is this module's is the
/// sentence, and that a name that breaks one never reaches a subprocess.
fn check_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("This call needs a branch name; none was given.".to_string());
    }
    if name.chars().count() > LONGEST_NAME {
        return Err(format!(
            "A branch name may be at most {LONGEST_NAME} characters long."
        ));
    }
    if let Some(found) = name.chars().find(|character| FORBIDDEN.contains(character)) {
        return Err(match found {
            ' ' => "A branch name may not contain a space.".to_string(),
            other => format!("A branch name may not contain '{other}'."),
        });
    }
    // `is_control` is the Unicode `Cc` category, which is git's rule exactly:
    // everything below `0x20` plus `DEL`.
    if name.chars().any(char::is_control) {
        return Err("A branch name may not contain a control character.".to_string());
    }
    // `-` first would be read as a flag by every git command here, and git
    // refuses it for that reason too.
    if name.starts_with('-') {
        return Err("A branch name may not start with '-'.".to_string());
    }
    // `@` alone is git's own shorthand for `HEAD`, and `@{` opens a reflog
    // selector — `main@{yesterday}` is a place in time, not a branch.
    if name == "@" {
        return Err("A branch name may not be '@' on its own.".to_string());
    }
    if name.contains("@{") {
        return Err("A branch name may not contain '@{'.".to_string());
    }
    if name.contains("..") {
        return Err("A branch name may not contain '..'.".to_string());
    }
    if name.starts_with('/') || name.ends_with('/') || name.contains("//") {
        return Err(
            "A branch name may not start or end with '/', or contain an empty part.".to_string(),
        );
    }
    if name.ends_with('.') {
        return Err("A branch name may not end with '.'.".to_string());
    }
    // Per *component*, because git stores a branch as a file under
    // `.git/refs/heads` and each rule below is about one of those names.
    for part in name.split('/') {
        if part.starts_with('.') {
            return Err("No part of a branch name may start with '.'.".to_string());
        }
        if part.ends_with(".lock") {
            return Err("No part of a branch name may end with '.lock'.".to_string());
        }
        if part.len() > LONGEST_PART {
            return Err(format!(
                "No part of a branch name may be longer than {LONGEST_PART} characters."
            ));
        }
    }
    Ok(name.to_string())
}

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

/// The `cwd` every call here takes, or the method's own refusal.
fn workspace(raw: &str, operation: &'static str) -> Result<String, Value> {
    git::workspace(raw).map_err(|why| why.to_error(operation, ""))
}

/// The two things `vcs.createRef` and `vcs.switchRef` both take, checked.
///
/// The name is checked *after* the folder so that a refusal about the name can
/// name the folder it was refused for — which is the only reason the order
/// matters, since neither check touches the other.
fn workspace_and_name(
    cwd: &str,
    name: &str,
    operation: &'static str,
) -> Result<(String, String), Value> {
    let cwd = workspace(cwd, operation)?;
    let name = check_name(name)
        .map_err(|why| Unavailable::Unusable { detail: why }.to_error(operation, &cwd))?;
    Ok((cwd, name))
}

/// The folder the call is about, checked the same way every other method that
/// takes one checks it.
fn root(cwd: &str, operation: &'static str) -> Result<WorkspaceRoot, Value> {
    WorkspaceRoot::check(cwd).map_err(|rejection| {
        Unavailable::Unusable {
            detail: rejection.message(),
        }
        .to_error(operation, cwd)
    })
}

/// Does this repository have this ref?
///
/// `--verify` because the short form would resolve `main` against tags and
/// remotes as well, and the question here is always about one namespace.
fn exists(root: &Path, refname: &str) -> bool {
    git::output(root, &["show-ref", "--verify", "--quiet", refname])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Has this repository got a commit yet?
///
/// A repository that has not is one where `HEAD` names a branch with nothing
/// behind it, which is what `git init` leaves and what a first commit ends.
fn has_a_commit(root: &Path) -> bool {
    git::output(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Run a git that is expected to change something, and keep everything it said
/// if it would not.
fn change(root: &Path, arguments: &[&str]) -> Result<(), Unavailable> {
    let output = git::output(root, arguments)?;
    if !output.status.success() {
        return Err(git::refusal(&output));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Branch names
    // -----------------------------------------------------------------------

    /// The names a developer actually types, which have to survive the check
    /// that exists to stop the ones that do not.
    #[test]
    fn ordinary_branch_names_are_accepted() {
        for name in [
            "main",
            "feature/branches",
            "fix-21",
            "release/2026.07",
            "user/hoangvu12/spike",
            "v1.2.3",
        ] {
            assert_eq!(check_name(name).as_deref(), Ok(name), "{name}");
        }

        // Trimmed, because the contract's own type is a trimmed string and the
        // text field it comes from is not.
        assert_eq!(check_name("  main  ").as_deref(), Ok("main"));
    }

    /// Every rule `git check-ref-format --branch` has, refused here instead —
    /// and each with a sentence a developer can act on, which is the whole
    /// reason the rules are duplicated. See ADR-0007.
    #[test]
    fn a_name_git_would_refuse_is_refused_before_git_sees_it() {
        for (name, expected) in [
            ("", "none was given"),
            ("   ", "none was given"),
            ("my branch", "space"),
            ("feature~1", "'~'"),
            ("feature^", "'^'"),
            ("feature:x", "':'"),
            ("what?", "'?'"),
            ("glob*", "'*'"),
            ("range[1]", "'['"),
            ("back\\slash", "'\\'"),
            ("-delete-everything", "start with '-'"),
            ("@", "'@' on its own"),
            ("main@{yesterday}", "'@{'"),
            ("a..b", "'..'"),
            ("/leading", "'/'"),
            ("trailing/", "'/'"),
            ("double//slash", "'/'"),
            ("ends.", "end with '.'"),
            (".hidden", "start with '.'"),
            ("feature/.hidden", "start with '.'"),
            ("feature.lock", "'.lock'"),
            ("feature/x.lock", "'.lock'"),
            ("with\ttab", "control character"),
        ] {
            let why = check_name(name).expect_err(&format!("{name:?} should be refused"));
            assert!(
                why.contains(expected),
                "{name:?} was refused with {why:?}, which does not mention {expected:?}"
            );
        }
    }

    /// A name has to be bounded, and the two bounds are different rules: one
    /// part of a name is a filename and stops where the filesystem does, while
    /// the whole name only has to stay off an unbounded command line.
    ///
    /// The asymmetry is load-bearing. `a/b/c/…` may be far longer than any of
    /// its parts, and a listing can return such a name — so a per-*name* limit
    /// of 255 would have made a branch this server itself listed one that a
    /// switch to it then refused as invalid.
    #[test]
    fn a_name_is_bounded_per_part_and_then_as_a_whole() {
        let part = "b".repeat(LONGEST_PART);
        assert!(check_name(&part).is_ok(), "a part exactly at the limit");
        assert!(check_name(&format!("{part}b"))
            .expect_err("a part past the limit")
            .contains(&LONGEST_PART.to_string()));

        // Three parts, each within the limit: longer than any part's ceiling
        // and still a name git would take.
        let deep = format!("{part}/{part}/{part}");
        assert!(deep.len() > LONGEST_PART);
        assert!(check_name(&deep).is_ok(), "{} characters", deep.len());

        // Past the whole-name backstop, which is the one that is this
        // module's own rather than git's.
        let absurd = "b/".repeat(LONGEST_NAME);
        assert!(check_name(&absurd)
            .expect_err("past the backstop")
            .contains(&LONGEST_NAME.to_string()));
    }

    // -----------------------------------------------------------------------
    // Reading the call
    // -----------------------------------------------------------------------

    /// The picker sends `cwd` and a limit and nothing else, which is what the
    /// capture holds. Everything else has to have a defensible default.
    #[test]
    fn a_listing_with_only_a_cwd_has_defaults_for_the_rest() {
        let call = ListRefs::read(&json!({"cwd": "/project"})).expect("a listing");

        assert_eq!(call.cursor, 0);
        assert_eq!(call.limit, MAX_LIMIT);
        assert_eq!(call.kind, Kind::All);
        assert_eq!(call.query, None);
        assert!(!call.include_matching_remote_refs);
    }

    /// A limit outside the contract's range is a client this server has not
    /// met. A page is a better answer to one than a refusal.
    #[test]
    fn a_limit_outside_the_contracts_range_is_clamped_rather_than_refused() {
        let asked = |limit: u64| {
            ListRefs::read(&json!({"cwd": "/project", "limit": limit}))
                .expect("a listing")
                .limit
        };

        assert_eq!(asked(0), 1);
        assert_eq!(asked(25), 25);
        assert_eq!(asked(10_000), MAX_LIMIT);
    }

    /// A blank query is not a query. Treating one as a filter would make the
    /// picker go empty the moment the developer cleared the box.
    #[test]
    fn a_blank_query_is_no_query_at_all() {
        let call = ListRefs::read(&json!({"cwd": "/project", "query": "   "})).expect("a listing");
        assert_eq!(call.query, None);

        let call = ListRefs::read(&json!({"cwd": "/project", "query": " Feat "})).expect("a listing");
        assert_eq!(call.query.as_deref(), Some("feat"));
    }

    /// Every method here refuses a call with no workspace root before anything
    /// runs, and the three git ones refuse under the error they declare.
    #[test]
    fn a_call_without_a_workspace_root_is_refused_before_anything_runs() {
        let error = ListRefs::read(&json!({"cwd": "  "})).expect_err("a refusal");
        assert_eq!(error["_tag"], "GitCommandError");

        let error = CreateRef::read(&json!({"cwd": "  ", "refName": "x"})).expect_err("a refusal");
        assert_eq!(error["_tag"], "GitCommandError");

        let error = SwitchRef::read(&json!({"cwd": "  ", "refName": "x"})).expect_err("a refusal");
        assert_eq!(error["_tag"], "GitCommandError");

        // `vcs.init` declares a different error union entirely — `VcsError`,
        // which has no `GitCommandError` in it.
        let error = Init::read(&json!({"cwd": "  "})).expect_err("a refusal");
        assert_eq!(error["_tag"], "VcsRepositoryDetectionError");
        assert_eq!(error["operation"], INIT);
        assert!(error.get("message").is_none(), "{error}");
    }

    /// A branch name that will not do is refused when the call is read, not
    /// when it is run — which is what "before it reaches the git binary" means
    /// in practice, since running is the only thing that reaches one.
    #[test]
    fn an_invalid_name_is_refused_at_the_boundary() {
        for tag in [CREATE_REF, SWITCH_REF] {
            let payload = json!({"cwd": "/project", "refName": "a branch"});
            let error = match tag {
                CREATE_REF => CreateRef::read(&payload).expect_err("a refusal"),
                _ => SwitchRef::read(&payload).expect_err("a refusal"),
            };
            assert_eq!(error["_tag"], "GitCommandError", "{tag}");
            assert_eq!(error["operation"], tag);
            assert!(
                error["detail"].as_str().expect("a detail").contains("space"),
                "{error}"
            );
        }
    }

    /// `jj` is a kind the contract has and this server does not drive. The
    /// union has an error that says exactly that, so it is used rather than
    /// pretending git is what was asked for.
    #[test]
    fn initialising_a_kind_this_server_does_not_drive_is_refused_by_name() {
        let repositories = Repositories::new(&crate::filesystem::Index::new());
        let call = Init::read(&json!({"cwd": "/project", "kind": "jj"})).expect("a call");
        let error = call.run(&repositories).expect_err("a refusal");

        assert_eq!(error["_tag"], "VcsUnsupportedOperationError");
        assert_eq!(error["kind"], "jj");
        assert!(error["detail"].as_str().expect("a detail").contains("jj"));
    }

    // -----------------------------------------------------------------------
    // What goes on the wire
    // -----------------------------------------------------------------------

    fn local(name: &str) -> Ref {
        Ref {
            name: name.to_string(),
            ..Ref::default()
        }
    }

    fn remote(remote: &str, branch: &str) -> Ref {
        Ref {
            name: format!("{remote}/{branch}"),
            remote: Some(remote.to_string()),
            ..Ref::default()
        }
    }

    fn listing(refs: Vec<Ref>) -> Listing {
        Listing {
            refs,
            is_repo: true,
            has_primary_remote: true,
        }
    }

    fn call(payload: Value) -> ListRefs {
        let mut payload = payload;
        payload["cwd"] = json!("/project");
        ListRefs::read(&payload).expect("a listing")
    }

    /// Every field `VcsRef` requires, in the spelling it requires — and
    /// `remoteName` only where the contract makes it meaningful.
    #[test]
    fn a_ref_serializes_to_the_contracts_shape() {
        let checked_out = Ref {
            name: "main".to_string(),
            remote: None,
            current: true,
            default: true,
            worktree_path: Some("C:/project".to_string()),
        };
        assert_eq!(
            checked_out.to_value(),
            json!({
                "name": "main",
                "isRemote": false,
                "current": true,
                "isDefault": true,
                "worktreePath": "C:/project",
            })
        );

        assert_eq!(
            remote("origin", "feature").to_value(),
            json!({
                "name": "origin/feature",
                "isRemote": true,
                "remoteName": "origin",
                "current": false,
                "isDefault": false,
                "worktreePath": Value::Null,
            })
        );
    }

    /// A folder that is not a repository is a listing, not a failure — the
    /// picker is on screen before `vcs.init` has been pressed.
    #[test]
    fn a_folder_that_is_not_a_repository_lists_nothing() {
        let page = call(json!({})).page(Listing::default());

        assert_eq!(
            page,
            json!({
                "refs": [],
                "isRepo": false,
                "hasPrimaryRemote": false,
                "nextCursor": Value::Null,
                "totalCount": 0,
            })
        );
    }

    /// `totalCount` is the whole list and `nextCursor` is where the next page
    /// starts, so a picker knows both how much there is and whether it has all
    /// of it.
    #[test]
    fn a_listing_longer_than_a_page_says_where_the_next_one_starts() {
        let many = listing((0..5).map(|index| local(&format!("b{index}"))).collect());

        let first = call(json!({"limit": 2})).page(many.clone());
        assert_eq!(names(&first), ["b0", "b1"]);
        assert_eq!(first["nextCursor"], json!(2));
        assert_eq!(first["totalCount"], json!(5));

        let second = call(json!({"limit": 2, "cursor": 2})).page(many.clone());
        assert_eq!(names(&second), ["b2", "b3"]);
        assert_eq!(second["nextCursor"], json!(4));

        let last = call(json!({"limit": 2, "cursor": 4})).page(many.clone());
        assert_eq!(names(&last), ["b4"]);
        assert_eq!(last["nextCursor"], Value::Null, "there is no sixth branch");

        // A cursor past the end is an empty page rather than a panic, because
        // the list can shrink between one page and the next.
        let past = call(json!({"limit": 2, "cursor": 99})).page(many);
        assert_eq!(names(&past), [] as [&str; 0]);
        assert_eq!(past["nextCursor"], Value::Null);
    }

    fn names(page: &Value) -> Vec<&str> {
        page["refs"]
            .as_array()
            .expect("an array of refs")
            .iter()
            .map(|entry| entry["name"].as_str().expect("a name"))
            .collect()
    }

    // -----------------------------------------------------------------------
    // Which refs a listing keeps
    // -----------------------------------------------------------------------

    /// The default: local branches, plus the remote ones that have no local
    /// counterpart. `origin/main` beside `main` is a row that says nothing.
    #[test]
    fn a_remote_ref_with_a_local_branch_of_its_own_name_is_folded_away() {
        let found = vec![
            local("main"),
            remote("origin", "main"),
            remote("origin", "nobody-has-this-locally"),
        ];

        let kept = select(found.clone(), &call(json!({})));
        assert_eq!(
            kept.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            ["main", "origin/nobody-has-this-locally"]
        );

        // …unless the client asks for them, which is what the flag is for.
        let kept = select(found, &call(json!({"includeMatchingRemoteRefs": true})));
        assert_eq!(kept.len(), 3);
    }

    /// The client can ask for one side or the other.
    #[test]
    fn the_ref_kind_picks_a_side() {
        let found = vec![local("main"), remote("origin", "other")];

        let locals = select(found.clone(), &call(json!({"refKind": "local"})));
        assert_eq!(locals.len(), 1);
        assert_eq!(locals[0].name, "main");

        let remotes = select(found.clone(), &call(json!({"refKind": "remote"})));
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin/other");

        assert_eq!(select(found, &call(json!({"refKind": "all"}))).len(), 2);
    }

    /// What the developer types into the picker, matched the way a picker
    /// matches: anywhere in the name, and without caring about case.
    #[test]
    fn a_query_matches_anywhere_in_the_name_and_ignores_case() {
        let found = vec![
            local("main"),
            local("feature/Branches"),
            local("fix-21"),
        ];

        let kept = select(found, &call(json!({"query": "BRANCH"})));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "feature/Branches");
    }

    /// The two rows anybody scrolling is looking for go first, and everything
    /// after them keeps git's order — which is total, and has to be, or a page
    /// boundary would repeat and skip rows.
    #[test]
    fn the_current_branch_leads_and_the_default_follows_it() {
        let found = vec![
            local("aardvark"),
            Ref {
                default: true,
                ..local("main")
            },
            local("zebra"),
            Ref {
                current: true,
                ..local("feature/branches")
            },
        ];

        let kept = select(found, &call(json!({})));
        assert_eq!(
            kept.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(),
            ["feature/branches", "main", "aardvark", "zebra"]
        );
    }

    // -----------------------------------------------------------------------
    // Reading git's own output
    // -----------------------------------------------------------------------

    /// A remote is not "everything before the first slash": a remote may be
    /// called `origin/mirror` and a branch may be called `feature/x`, and only
    /// the list of remotes can tell the two apart.
    ///
    /// The same split has to answer for both the listing and the switch — see
    /// [`SwitchRef::target`] — because a switch is asked for by a name the
    /// listing produced.
    #[test]
    fn a_remote_ref_is_split_against_the_remotes_that_exist() {
        let bearings = Bearings {
            remotes: vec!["origin".to_string(), "origin/mirror".to_string()],
            ..Bearings::default()
        };

        let plain = bearings
            .describe("refs/remotes/origin/feature/x")
            .expect("a ref");
        assert_eq!(plain.remote.as_deref(), Some("origin"));
        assert_eq!(plain.name, "origin/feature/x");
        assert_eq!(plain.branch(), "feature/x");

        let nested = bearings
            .describe("refs/remotes/origin/mirror/main")
            .expect("a ref");
        assert_eq!(nested.remote.as_deref(), Some("origin/mirror"));
        assert_eq!(nested.branch(), "main");
        assert_eq!(
            bearings.split_remote("origin/mirror/main"),
            Some(("origin/mirror", "main")),
            "a switch to this name would make a local branch called mirror/main"
        );

        // A ref under a remote this repository does not have is not a branch
        // anybody can switch to.
        assert_eq!(bearings.describe("refs/remotes/gone/main"), None);
    }

    /// The three things that qualify a local branch: it is the one checked out
    /// here, it is the default one, and it is checked out somewhere.
    #[test]
    fn a_local_ref_carries_what_qualifies_it() {
        let bearings = Bearings {
            worktrees: HashMap::from([(
                "refs/heads/feature".to_string(),
                "C:/project/../feature".to_string(),
            )]),
            current: Some("main".to_string()),
            default: Some("main".to_string()),
            ..Bearings::default()
        };

        let current = bearings.describe("refs/heads/main").expect("a ref");
        assert!(current.current);
        assert!(current.default);
        assert_eq!(current.worktree_path, None);

        let elsewhere = bearings.describe("refs/heads/feature").expect("a ref");
        assert!(
            !elsewhere.current,
            "a branch in another worktree is not the one we are on"
        );
        assert_eq!(
            elsewhere.worktree_path.as_deref(),
            Some("C:/project/../feature")
        );
    }

    /// A repository with no commit yet has a branch nothing points at, and the
    /// picker has to show it or it would disagree with the status about where
    /// the developer is standing.
    #[test]
    fn the_branch_of_an_unborn_head_is_listed_even_though_no_ref_names_it() {
        let bearings = Bearings {
            current: Some("main".to_string()),
            default: Some("main".to_string()),
            ..Bearings::default()
        };

        let unborn = bearings.unborn(&[]).expect("the branch HEAD names");
        assert_eq!(unborn.name, "main");
        assert!(unborn.current);
        assert!(unborn.default);

        // …and once a commit exists, the real ref is the row and there is no
        // second one.
        assert_eq!(bearings.unborn(&[local("main")]), None);
    }
}
