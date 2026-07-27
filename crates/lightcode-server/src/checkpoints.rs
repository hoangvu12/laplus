//! Checkpoints: turns as points in time against the working tree, and the two
//! diffs a developer reviews them by.
//!
//! Two method tags land here — `orchestration.getTurnDiff`, which is one step in
//! isolation, and `orchestration.getFullThreadDiff`, which is the session as one
//! coherent change. Both are the same question asked over a different range, and
//! neither is answerable without the thing this module exists for.
//!
//! ## The whole idea: a turn is a commit nobody can see
//!
//! A diff needs two sides. The working tree is one of them; the other has to be
//! *what the tree looked like before the turn*, and nothing in git records that,
//! because the developer has not committed anything. So this server records it:
//! at every turn boundary it writes the whole working tree — tracked, staged,
//! untracked and all — as a commit under a ref of its own, and a turn's diff is
//! `git diff` between two of those commits.
//!
//! ```text
//! refs/lightcode/checkpoints/<thread>/turn/0   before the first turn
//! refs/lightcode/checkpoints/<thread>/turn/1   after it, and before the second
//! refs/lightcode/checkpoints/<thread>/turn/2   …
//!
//! turn 2's diff        = diff(turn/1, turn/2)
//! the conversation's   = diff(turn/0, turn/N)
//! ```
//!
//! Five of this ticket's criteria fall straight out of that shape rather than
//! being implemented:
//!
//! - **Added, modified, deleted and renamed files** are what `git diff` between
//!   two commits reports. Rename detection is git's own and is on by default.
//! - **Untracked files appear**, because `git add -A` puts them in the tree the
//!   checkpoint commits. A file the agent has just created is untracked, and it
//!   is the single most interesting thing in the diff.
//! - **A turn that changed nothing is an empty diff**, because two identical
//!   trees diff to nothing. There is no error to produce and no special case to
//!   write.
//! - **Binary files are indicated, not rendered**: git says `Binary files …
//!   differ` and this passes no `--binary`, so no content is ever attempted.
//! - **Hand edits between turns are attributed to the turn they happened in**,
//!   because a checkpoint is a photograph of the tree and not a record of who
//!   moved. That is the right answer for review — the question the panel asks is
//!   "what is different now", not "who typed it" — and it is the only answer a
//!   snapshot model *can* give. See ADR-0008.
//!
//! ## Why plumbing rather than `git commit`
//!
//! `git add` and `git commit` would move `HEAD`, rewrite `.git/index`, and put
//! the developer's work on their branch. All three are unacceptable: the
//! developer's repository is theirs, and a review feature that committed for
//! them would be the worst kind of surprise.
//!
//! So a capture is four plumbing commands and touches none of that:
//!
//! | | |
//! |---|---|
//! | `read-tree HEAD` | seed a **temporary** index from the current commit |
//! | `add -A -- .` | stage the whole working tree into it, untracked files and all |
//! | `write-tree` | turn that index into a tree object |
//! | `commit-tree` | wrap the tree in a **parentless** commit |
//! | `update-ref` | name the commit under this thread's own ref |
//!
//! `GIT_INDEX_FILE` points every one of them at a scratch file outside the
//! repository, so the developer's own index — their staged work — is never read
//! back or written. The commit has no parent, which keeps it off every history
//! the developer can see: `git log`, `git branch --contains` and a push all
//! ignore it, and the only way to reach one is to name its ref.
//!
//! An identity is supplied in the environment because `commit-tree` refuses
//! without one, and a machine that has never run `git config --global user.email`
//! is an ordinary machine.
//!
//! ## What this costs, said plainly
//!
//! **A capture re-hashes the working tree.** The temporary index starts from
//! `HEAD` with no stat cache, so `add -A` reads every file git does not already
//! know the content of. On a large repository that is the cost of a cold
//! `git status`, once per turn — which is affordable because a turn is a
//! human-scale event, and is the reason this is not done more often than that.
//! Ignored files are excluded by `add`'s ordinary rules, so `node_modules` and
//! `target` cost nothing.
//!
//! **The refs are kept.** Nothing deletes them, so a long-lived project
//! accumulates one ref and one commit per turn, and the objects they point at
//! stay alive against `git gc`. That is deliberate for v1 — a diff a developer
//! can still open tomorrow is the point of the feature — and it is the loose end
//! this module knows about. `git for-each-ref refs/lightcode` is the whole of
//! finding them and `git update-ref -d` the whole of removing one.
//!
//! Shapes are hand-written from `OrchestrationGetTurnDiffInput`,
//! `OrchestrationGetFullThreadDiffInput` and `ThreadTurnDiff` in
//! `t3code/packages/contracts/src/orchestration.ts`, and the errors from
//! `OrchestrationGetTurnDiffError` and `OrchestrationGetFullThreadDiffError` in
//! the same file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::git::{self, Unavailable};

/// One step in isolation.
pub const GET_TURN_DIFF: &str = "orchestration.getTurnDiff";

/// The whole conversation as one change.
pub const GET_FULL_THREAD_DIFF: &str = "orchestration.getFullThreadDiff";

/// Where a checkpoint ref lives, relative to the git directory.
///
/// Under `refs/lightcode/` rather than `refs/heads/` or `refs/tags/` so that
/// nothing a developer runs shows one: `git branch`, `git tag`, `git log --all`
/// and `git push` all walk namespaces this is not in. `crate::git` reads this to
/// know that a checkpoint being written is not a change to the working tree.
pub(crate) const REFS: &str = "refs/lightcode/checkpoints";

/// The most of a diff that is read out of git and sent to the client.
///
/// A bound rather than a preference. `ThreadTurnDiff` carries the patch as a
/// single string, so a conversation that regenerated a lock file would otherwise
/// put tens of megabytes through a JSON encoder, a socket and a browser — and
/// the browser is the part that stops. Ten megabytes is far past what anyone
/// reads and is the same ceiling the reference server puts on the same call
/// (`CHECKPOINT_DIFF_MAX_OUTPUT_BYTES` in `apps/server/src/vcs/GitVcsDriver.ts`).
///
/// It is enforced *while reading the child*, not afterwards: a cap applied to a
/// string that has already been built is not a cap on anything.
const MAX_PATCH: usize = 10_000_000;

/// What is put at the end of a patch that was cut.
///
/// The ticket asks for truncation to be obvious, and `ThreadTurnDiff` has no
/// flag to say it — the patch is the only channel there is, so the notice goes
/// in the patch. It is placed after a blank line and starts outside any hunk, so
/// a diff parser reading the text has already finished the last file it was
/// given; the reference server marks its own truncation in the same place and
/// much the same way.
///
/// The size is formatted from [`MAX_PATCH`] rather than written out, because a
/// notice quoting a number the code no longer uses is worse than no number.
fn cut_notice() -> String {
    format!(
        "\n\n*** This diff was truncated by lightcode: it is larger than {} MB, and only the \
         files above are shown. ***\n",
        MAX_PATCH / 1_000_000
    )
}

/// The identity a checkpoint commit is made under.
///
/// Supplied rather than inherited, and both halves matter: `commit-tree` refuses
/// outright on a machine with no `user.email` configured, and a checkpoint
/// attributed to the developer would be this server signing their name to a
/// commit they did not make.
const AUTHOR: &str = "lightcode";
const EMAIL: &str = "checkpoints@lightcode.invalid";

// ---------------------------------------------------------------------------
// Naming a checkpoint
// ---------------------------------------------------------------------------

/// The ref this thread's `turn_count`-th checkpoint is kept under.
///
/// The thread id is **hex-encoded**, which is ugly and is the point: a ref name
/// is a path, and git refuses a great many strings a thread id is allowed to be
/// — anything with a space, a colon, a backslash, two dots, a leading dot, a
/// trailing `.lock`. Sanitising would have to map several ids onto one name,
/// and two conversations sharing a checkpoint is the one failure here that would
/// show a developer somebody else's diff. Hex cannot collide.
pub fn reference(thread_id: &str, turn_count: u64) -> String {
    let mut named = String::with_capacity(thread_id.len() * 2);
    for byte in thread_id.as_bytes() {
        named.push_str(&format!("{byte:02x}"));
    }
    format!("{REFS}/{named}/turn/{turn_count}")
}

// ---------------------------------------------------------------------------
// Writing one
// ---------------------------------------------------------------------------

/// Record the whole working tree under `reference`.
///
/// See this module's documentation for the sequence and for why it is plumbing.
/// Nothing here touches the developer's index, their `HEAD` or their branches.
pub fn capture(root: &Path, reference: &str) -> Result<(), Unavailable> {
    let index = ScratchIndex::new();
    let environment = index.environment();
    let run = |arguments: &[&str]| -> Result<String, Unavailable> {
        let output = git::output_with(root, arguments, &environment)?;
        if !output.status.success() {
            return Err(git::refusal(&output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    };

    // Only when there is a commit to read. A repository the developer has just
    // run `git init` in has a `HEAD` that names a branch with nothing behind it,
    // and `read-tree` on that is an error rather than an empty tree.
    if present(root, "HEAD") {
        run(&["read-tree", "HEAD"])?;
    }
    // The path is `.` rather than the whole repository, so a project opened as
    // one package inside a monorepo checkpoints that package. `-A` is what puts
    // untracked files in — the agent's brand new file — and it obeys
    // `.gitignore`, which is what keeps a build directory out.
    run(&["add", "-A", "--", "."])?;

    let tree = run(&["write-tree"])?;
    if tree.is_empty() {
        return Err(Unavailable::Refused {
            detail: "git write-tree named no tree, so there is nothing to checkpoint."
                .to_string(),
            exit_code: None,
        });
    }

    // Parentless: see this module's documentation. The message is for a
    // developer who goes looking with `git show`.
    let commit = run(&["commit-tree", &tree, "-m", &format!("lightcode {reference}")])?;
    if commit.is_empty() {
        return Err(Unavailable::Refused {
            detail: "git commit-tree named no commit, so there is nothing to checkpoint."
                .to_string(),
            exit_code: None,
        });
    }

    run(&["update-ref", reference, &commit])?;
    Ok(())
}

/// Does this revision name a commit in this repository?
///
/// Two callers and two questions, which is why it takes a revision rather than
/// being named after either of them: "is there a checkpoint under this ref" and
/// "has this repository got a commit yet" are the same lookup, asked of a
/// checkpoint ref and of `HEAD`. [`crate::refs`] asks the second one too, for a
/// different purpose — what a new branch can be made from — and keeps its own.
pub fn present(root: &Path, revision: &str) -> bool {
    git::output(
        root,
        &["rev-parse", "--verify", "--quiet", &commitish(revision)],
    )
    .map(|output| output.status.success())
    .unwrap_or(false)
}

/// A ref, spelled so that git resolves it to a commit and to nothing else.
///
/// `refs/lightcode/checkpoints/…^{commit}` cannot be mistaken for a path, a tag
/// or a branch of the same name, which matters because the string in front of it
/// is composed from a thread id.
fn commitish(reference: &str) -> String {
    format!("{reference}^{{commit}}")
}

/// The scratch index a capture stages into, removed when the capture is over.
///
/// In the system's temporary directory rather than beside the repository, and
/// that is not tidiness: everything under `.git` that is not an object or a log
/// is watched, so an index file written and deleted inside it would mark the
/// working tree stale and cost a `git status` for every checkpoint. See
/// [`crate::git`]'s `affects_status`.
struct ScratchIndex {
    path: PathBuf,
}

impl ScratchIndex {
    fn new() -> ScratchIndex {
        /// Unique within this process. Paired with the process id below, which
        /// is what makes it unique across the two servers a developer may have
        /// open on one repository.
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        ScratchIndex {
            path: std::env::temp_dir().join(format!(
                "lightcode-checkpoint-{}-{unique}.index",
                std::process::id()
            )),
        }
    }

    fn environment(&self) -> Vec<(&'static str, &std::ffi::OsStr)> {
        vec![
            ("GIT_INDEX_FILE", self.path.as_os_str()),
            ("GIT_AUTHOR_NAME", AUTHOR.as_ref()),
            ("GIT_AUTHOR_EMAIL", EMAIL.as_ref()),
            ("GIT_COMMITTER_NAME", AUTHOR.as_ref()),
            ("GIT_COMMITTER_EMAIL", EMAIL.as_ref()),
        ]
    }
}

impl Drop for ScratchIndex {
    /// Removed however the capture ended, including through the `?` on any of
    /// the five commands. A file left behind is a few hundred kilobytes of the
    /// developer's temporary directory per turn, which is small and is still
    /// litter.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Reading one back
// ---------------------------------------------------------------------------

/// One file a turn touched, as `OrchestrationCheckpointFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub path: String,
    /// `added`, `modified`, `deleted`, `renamed` — or whatever else git's own
    /// status letter meant. Free text in the contract, and it is a label rather
    /// than a discriminator: the patch itself is what the panel renders.
    pub kind: String,
    pub additions: u64,
    pub deletions: u64,
}

impl Changed {
    pub fn to_value(&self) -> Value {
        json!({
            "path": self.path,
            "kind": self.kind,
            "additions": self.additions,
            "deletions": self.deletions,
        })
    }
}

/// A summary as [`crate::store`] gave it back.
///
/// Lenient in one direction only: a row this build did not write, or one
/// somebody edited by hand, comes back as *no summary* rather than as a wrong
/// one. Nothing is lost by that — the patch is what the panel renders, and the
/// summary is the line above it.
pub fn changed_from_stored(stored: &str) -> Vec<Changed> {
    let Ok(Value::Array(rows)) = serde_json::from_str::<Value>(stored) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|row| {
            Some(Changed {
                path: row.get("path")?.as_str()?.to_string(),
                kind: row.get("kind")?.as_str()?.to_string(),
                additions: row.get("additions")?.as_u64()?,
                deletions: row.get("deletions")?.as_u64()?,
            })
        })
        .collect()
}

/// What changed between two checkpoints, with a line count for each file.
///
/// Two `git diff`s rather than one, because git will not print both vocabularies
/// at once — `--name-status` and `--numstat` are the same option slot and the
/// last one given wins. The status letters say *what happened to* a file and the
/// numbers say *how much*, and the summary the contract asks for wants both.
///
/// Both are read with `-z`, so a path with a newline or a quote in it arrives
/// whole rather than as git's own C-style quoting.
pub fn changed(root: &Path, from: &str, to: &str) -> Result<Vec<Changed>, Unavailable> {
    let kinds = name_status(root, from, to)?;
    let counts = numstat(root, from, to)?;

    let mut files: Vec<Changed> = counts
        .into_iter()
        .map(|(additions, deletions, path)| Changed {
            kind: kinds
                .get(&path)
                .map(|status| named(status))
                .unwrap_or("modified")
                .to_string(),
            path,
            additions,
            deletions,
        })
        .collect();

    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// How much each file changed by, from `git diff --numstat -z`.
///
/// The `-z` format is a stream of NUL-terminated chunks with no row separator,
/// so a row is recovered by knowing its shape — and numstat's shape has an
/// exception. Ordinarily the whole row is one chunk, `12\t3\tsrc/a.rs`; for a
/// rename or a copy the path is *left off* it, `0\t0\t`, and the two paths
/// follow as chunks of their own. The second of them is the row's subject,
/// because that is the file that exists now.
fn numstat(root: &Path, from: &str, to: &str) -> Result<Vec<(u64, u64, String)>, Unavailable> {
    let listing = read(root, "--numstat", from, to)?;
    let mut chunks = listing.into_iter();
    let mut counted = Vec::new();
    while let Some(head) = chunks.next() {
        let mut fields = head.splitn(3, '\t');
        // `-` where a number belongs is git saying the file is binary, which is
        // a file with no lines rather than a file with none changed. Zero is the
        // honest answer to "how many lines"; the patch is where the developer is
        // told it is binary at all.
        let additions = fields.next().and_then(|field| field.parse().ok()).unwrap_or(0);
        let deletions = fields.next().and_then(|field| field.parse().ok()).unwrap_or(0);
        let path = match fields.next().unwrap_or("") {
            "" => {
                let old = chunks.next().unwrap_or_default();
                chunks.next().unwrap_or(old)
            }
            named => named.to_string(),
        };
        counted.push((additions, deletions, path));
    }
    Ok(counted)
}

/// What happened to each file, from `git diff --name-status -z`.
///
/// What happened to each file, keyed by path, from `git diff --name-status -z`.
///
/// A simpler shape than numstat's and it does not vary: the status letter is
/// always its own chunk, followed by one path — or, for a rename or a copy, by
/// two. A map rather than a list because [`changed`] joins the two listings on
/// the path, and a turn that touched a thousand files would otherwise be a
/// thousand scans of a thousand entries.
fn name_status(
    root: &Path,
    from: &str,
    to: &str,
) -> Result<HashMap<String, String>, Unavailable> {
    let listing = read(root, "--name-status", from, to)?;
    let mut chunks = listing.into_iter();
    let mut statuses = HashMap::new();
    while let Some(status) = chunks.next() {
        let Some(first) = chunks.next() else { break };
        let path = match status.starts_with('R') || status.starts_with('C') {
            true => chunks.next().unwrap_or(first),
            false => first,
        };
        statuses.insert(path, status);
    }
    Ok(statuses)
}

/// One `git diff -z` listing of the shape `shape` asks for, as its records.
///
/// `-z` throughout, so a path with a newline or a quote in it arrives whole
/// rather than as git's own C-style quoting — which would have to be undone
/// here, and undone the same way git does it. Split by
/// [`crate::git::records`], which is where the framing the two formats share is
/// already understood.
fn read(root: &Path, shape: &str, from: &str, to: &str) -> Result<Vec<String>, Unavailable> {
    let listed = git::output(
        root,
        &[
            "diff",
            "-z",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            shape,
            &commitish(from),
            &commitish(to),
        ],
    )?;
    if !listed.status.success() {
        return Err(git::refusal(&listed));
    }
    Ok(git::records(&listed.stdout).collect())
}

/// git's status letter, as a word.
///
/// The score after `R` and `C` is how alike the two files are and is dropped:
/// the summary says a file was renamed, and how nearly is what the patch shows.
fn named(status: &str) -> &'static str {
    match status.chars().next() {
        Some('A') => "added",
        Some('D') => "deleted",
        Some('R') => "renamed",
        Some('C') => "copied",
        Some('T') => "retyped",
        _ => "modified",
    }
}

/// The patch between two checkpoints, bounded.
///
/// Read a piece at a time out of the child rather than with
/// [`std::process::Command::output`], and that is the only reason this does not
/// go through [`crate::git::text`]: the size of this output is the size of the
/// developer's change, and a cap applied after the whole thing is in memory has
/// not capped anything. Past [`MAX_PATCH`] the child is killed, whatever it was
/// still writing is dropped, and [`cut_notice`] is what the developer
/// reads instead.
pub fn patch(
    root: &Path,
    from: &str,
    to: &str,
    ignore_whitespace: bool,
) -> Result<String, Unavailable> {
    let mut arguments = vec![
        "diff",
        "--patch",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ];
    if ignore_whitespace {
        arguments.push("--ignore-all-space");
    }
    let from = commitish(from);
    let to = commitish(to);
    arguments.push(&from);
    arguments.push(&to);

    let mut child = git::started(root, &arguments, &[])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(git::spawn_failure)?;

    let mut stdout = child.stdout.take().expect("stdout was piped");
    let (read, full) = take(&mut stdout, MAX_PATCH);

    if full {
        // Nothing is going to read the rest, and a child writing into a pipe
        // nobody drains never exits. Killed first and *then* waited for, which
        // is the only order that terminates.
        let _ = child.kill();
        let _ = child.wait();
        return Ok(cut(read));
    }

    let finished = child.wait_with_output().map_err(|error| Unavailable::Refused {
        detail: format!("git could not be waited for: {error}"),
        exit_code: None,
    })?;
    if !finished.status.success() {
        return Err(git::refusal(&finished));
    }
    Ok(read)
}

/// Read at most `most` bytes, saying whether that is where it stopped.
///
/// Lossy, like everything else here that turns git's bytes into a string: a
/// patch of a file in an encoding this is not is still worth showing, and the
/// alternative is refusing the whole diff over one line of it.
fn take(source: &mut impl std::io::Read, most: usize) -> (String, bool) {
    use std::io::Read;

    let mut read = Vec::new();
    // One past the cap, so "exactly the cap" is not reported as truncated.
    let taken = source.take(most as u64 + 1).read_to_end(&mut read);
    if taken.is_err() {
        // A pipe that failed mid-read is a diff that is partly there, and the
        // part that is there is worth more than an error about the rest.
        return (String::from_utf8_lossy(&read).into_owned(), true);
    }
    let full = read.len() > most;
    read.truncate(most);
    (String::from_utf8_lossy(&read).into_owned(), full)
}

/// Cut a patch back to something a diff reader can finish, and say so.
///
/// At a **file boundary** where there is one, so what is shown is a whole
/// number of files rather than a hunk that stops mid-line. A single file larger
/// than the cap has no boundary to fall back to, so that one is cut at the last
/// complete line — still not mid-line, which is the property that matters.
fn cut(patch: String) -> String {
    // The first file's header has no newline in front of it, so it is not a
    // boundary this can find — which is what stops a patch of one huge file
    // from being cut back to nothing.
    let boundary = patch
        .rfind("\ndiff --git ")
        .or_else(|| patch.rfind('\n'))
        .map(|at| at + 1)
        .unwrap_or(0);
    let mut kept: String = patch[..boundary].to_string();
    kept.push_str(&cut_notice());
    kept
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// A validated diff request, either kind.
///
/// One struct for two methods because they are one question over a different
/// range: a full-thread diff is a turn diff whose `fromTurnCount` is zero. The
/// contract says as much — both answer with `ThreadTurnDiff`, and the reference
/// server's `getFullThreadDiff` is documented as "turn-diff semantics with
/// `fromTurnCount = 0`".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    thread_id: String,
    from: u64,
    to: u64,
    ignore_whitespace: bool,
    asked: Asked,
}

/// Which of the two methods asked.
///
/// A type rather than the method tag as a string — which is what
/// [`crate::git::StatusCall`] carries — because this one is *matched on* rather
/// than merely carried, twice: once to decide where the range starts and once to
/// pick the error tag. A tag misspelled in either place would silently answer as
/// the other method, and the error would then fail to decode against a union it
/// is not in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Asked {
    /// One step, between the two counts the client named.
    Turn,
    /// Everything up to a turn, from the baseline.
    Thread,
}

impl Asked {
    /// The error this method declares. Each declares a union of one, so the
    /// other's would cost the client the call rather than showing the sentence.
    fn error(self) -> &'static str {
        match self {
            Asked::Turn => "OrchestrationGetTurnDiffError",
            Asked::Thread => "OrchestrationGetFullThreadDiffError",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiffPayload {
    thread_id: String,
    /// Absent on a full-thread diff, which always starts at the baseline.
    from_turn_count: Option<u64>,
    to_turn_count: u64,
    /// The reference server defaults this to **true**, which is a decision
    /// about what a review should show rather than a default value: a
    /// reformatting turn otherwise fills the panel with lines that did not
    /// change. Mirrored rather than chosen.
    ignore_whitespace: Option<bool>,
}

impl Diff {
    /// `orchestration.getTurnDiff`: one turn, named by the two counts it sits
    /// between.
    pub fn read_turn(payload: &Value) -> Result<Diff, Value> {
        Diff::read(payload, Asked::Turn)
    }

    /// `orchestration.getFullThreadDiff`: everything up to a turn, from the
    /// baseline.
    pub fn read_thread(payload: &Value) -> Result<Diff, Value> {
        Diff::read(payload, Asked::Thread)
    }

    fn read(payload: &Value, asked: Asked) -> Result<Diff, Value> {
        let read: DiffPayload = serde_json::from_value(payload.clone())
            .map_err(|error| refused(asked, format!("This call is malformed: {error}")))?;
        let thread_id = crate::rpc::non_blank(&read.thread_id, asked.error(), "thread")?;

        // Zero for a full-thread diff by definition; for a turn diff it is what
        // the client sent, and the contract's own check on the pair is that the
        // range is not backwards. A range that is refused here is a client bug
        // — the panel computes `max(0, n - 1)` — so the sentence names the
        // numbers rather than trying to be helpful about them.
        let from = match asked {
            Asked::Thread => 0,
            Asked::Turn => read.from_turn_count.unwrap_or(0),
        };
        if from > read.to_turn_count {
            return Err(refused(
                asked,
                format!(
                    "A diff cannot run from turn {from} to turn {}: the range is backwards.",
                    read.to_turn_count
                ),
            ));
        }

        Ok(Diff {
            thread_id,
            from,
            to: read.to_turn_count,
            ignore_whitespace: read.ignore_whitespace.unwrap_or(true),
            asked,
        })
    }

    /// Answer the call: find the conversation, resolve its two checkpoints, and
    /// diff them.
    ///
    /// Runs off the connection's read loop — see [`crate::rpc::Deferred`] — for
    /// the reason every other git-shaped method does: this is a child process
    /// over a repository whose size is the developer's.
    pub fn run(self, shell: &crate::orchestration::Shell) -> Result<Value, Value> {
        // Answered before anything is looked up, and that is a criterion rather
        // than an optimisation: a turn that changed nothing shows an empty diff
        // rather than an error, and the emptiest range of all is one that spans
        // no turns. It is also what the panel asks for on a thread whose first
        // turn has not finished.
        if self.from == self.to {
            return Ok(self.to_value(String::new()));
        }

        let reviewing = shell
            .reviewing(&self.thread_id)
            .map_err(|why| refused(self.asked, why.message()))?;
        if self.to > reviewing.checkpoints {
            return Err(refused(
                self.asked,
                format!(
                    "This conversation has {} recorded turn{}, so there is nothing to show for \
                     turn {}.",
                    reviewing.checkpoints,
                    match reviewing.checkpoints {
                        1 => "",
                        _ => "s",
                    },
                    self.to
                ),
            ));
        }

        let root = crate::projects::WorkspaceRoot::check(&reviewing.workspace_root)
            .map_err(|rejection| refused(self.asked, rejection.message()))?;
        let from = reference(&self.thread_id, self.from);
        let to = reference(&self.thread_id, self.to);
        for (turn, reference) in [(self.from, &from), (self.to, &to)] {
            if !present(root.path(), reference) {
                return Err(refused(
                    self.asked,
                    format!(
                        "The state of this project at turn {turn} was not recorded, so there is \
                         no diff to show. Checkpoints are kept in the project's git repository \
                         and a project that had none at the time has nothing to compare."
                    ),
                ));
            }
        }

        let patch = patch(root.path(), &from, &to, self.ignore_whitespace)
            .map_err(|why| refused(self.asked, why.detail()))?;
        Ok(self.to_value(patch))
    }

    /// `ThreadTurnDiff`. The range is echoed back because the client caches the
    /// answer under it.
    fn to_value(&self, diff: String) -> Value {
        json!({
            "threadId": self.thread_id,
            "fromTurnCount": self.from,
            "toTurnCount": self.to,
            "diff": diff,
        })
    }
}

/// The typed refusal, in the shape the client decodes.
///
/// Unlike `GitCommandError`, both of these declare `message` as a *field*, so
/// the sentence travels rather than being composed on the other side.
fn refused(asked: Asked, message: impl std::fmt::Display) -> Value {
    crate::rpc::declared(asked.error(), message)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one property of a ref name that has to hold: two conversations never
    /// share one. Everything else about the encoding is legibility, which it
    /// does not have.
    #[test]
    fn two_threads_never_name_the_same_checkpoint() {
        let awkward = ["a", "a/b", "a b", "a:b", "..", ".lock", "a\\b", "A"];
        let mut named: Vec<String> = awkward
            .iter()
            .map(|thread| reference(thread, 1))
            .collect();
        named.sort();
        named.dedup();
        assert_eq!(named.len(), awkward.len(), "{named:?}");
    }

    /// A ref name git will accept, whatever the thread id was. `check-ref-format`
    /// is the authority and this is the shape it wants: no space, no colon, no
    /// backslash, no `..`, nothing beginning with a dot.
    #[test]
    fn a_checkpoint_ref_is_a_ref_name_git_allows() {
        let named = reference("a b:c\\d..e", 3);
        assert!(named.starts_with("refs/lightcode/checkpoints/"), "{named}");
        assert!(named.ends_with("/turn/3"), "{named}");
        let component = named
            .strip_prefix(&format!("{REFS}/"))
            .and_then(|rest| rest.split('/').next())
            .expect("a component for the thread");
        assert!(
            component.chars().all(|character| character.is_ascii_hexdigit()),
            "{component}"
        );
    }

    /// A patch that was cut ends at a file boundary and says it was cut. Without
    /// the notice the developer reads a short diff as a small change.
    #[test]
    fn a_truncated_patch_stops_at_a_whole_file_and_says_so() {
        let patch = "diff --git a/one b/one\n@@ -1 +1 @@\n-a\n+b\n\
                     diff --git a/two b/two\n@@ -1 +1 @@\n-c\n+"
            .to_string();
        let kept = cut(patch);

        assert!(kept.starts_with("diff --git a/one b/one\n"), "{kept}");
        assert!(!kept.contains("a/two"), "the half-written file survived: {kept}");
        assert!(kept.contains("truncated by lightcode"), "{kept}");
    }

    /// One file bigger than the whole budget has no boundary to fall back to, so
    /// it is cut at a line ending instead — never mid-line, which would be a
    /// patch no reader can finish.
    #[test]
    fn one_enormous_file_is_cut_at_a_line_rather_than_dropped() {
        let kept = cut("diff --git a/one b/one\n@@ -1 +1 @@\n-a\n+bbbb".to_string());

        assert!(kept.starts_with("diff --git a/one b/one\n"), "{kept}");
        assert!(!kept.contains("+bbbb"), "{kept}");
        assert!(kept.contains("truncated by lightcode"), "{kept}");
    }

    /// The reader stops at the cap and says it did, and stops *without* saying so
    /// when the output happens to be exactly the cap. An off-by-one here is a
    /// truncation notice on a complete diff.
    #[test]
    fn the_reader_bounds_what_it_takes() {
        let (read, full) = take(&mut "abcdef".as_bytes(), 3);
        assert_eq!(read, "abc");
        assert!(full);

        let (read, full) = take(&mut "abc".as_bytes(), 3);
        assert_eq!(read, "abc");
        assert!(!full, "exactly the cap is not a truncation");
    }

    /// A range that spans no turns is an empty diff and not a lookup. Driven
    /// here rather than through the socket because the point is that *nothing*
    /// is consulted: there is no shell in this test to consult.
    #[test]
    fn a_range_that_spans_no_turns_needs_nothing_to_answer() {
        let call = Diff::read_turn(&json!({
            "threadId": "thread-1",
            "fromTurnCount": 2,
            "toTurnCount": 2,
        }))
        .expect("a well-formed call");

        let answered = call
            .run(&crate::orchestration::Shell::new(
                crate::store::Database::in_memory().expect("an in-memory database"),
            ))
            .expect("an empty diff rather than a refusal");
        assert_eq!(answered["diff"], "");
        assert_eq!(answered["fromTurnCount"], 2);
        assert_eq!(answered["toTurnCount"], 2);
    }

    /// A backwards range is the contract's own check, and it is refused under
    /// the tag of whichever method was asked — an error the other method
    /// declares would not decode.
    #[test]
    fn a_backwards_range_is_refused_under_the_asking_methods_tag() {
        let error = Diff::read_turn(&json!({
            "threadId": "thread-1",
            "fromTurnCount": 4,
            "toTurnCount": 1,
        }))
        .expect_err("a refusal");
        assert_eq!(error["_tag"], "OrchestrationGetTurnDiffError");
        assert!(error["message"].as_str().expect("a message").contains("backwards"));
    }

    /// A full-thread diff starts at the baseline whatever the client sent,
    /// because that is what the method means.
    #[test]
    fn a_full_thread_diff_always_starts_at_the_baseline() {
        let call = Diff::read_thread(&json!({
            "threadId": "thread-1",
            "fromTurnCount": 3,
            "toTurnCount": 4,
        }))
        .expect("a well-formed call");
        assert_eq!(call.from, 0);
        assert_eq!(call.to, 4);
    }

    /// Whitespace is ignored unless the client says otherwise — the reference
    /// server's default, and a decision about what a review shows rather than a
    /// value.
    #[test]
    fn whitespace_is_ignored_by_default() {
        let asked = |payload| Diff::read_turn(&payload).expect("well formed").ignore_whitespace;
        assert!(asked(json!({"threadId": "t", "fromTurnCount": 0, "toTurnCount": 1})));
        assert!(!asked(json!({
            "threadId": "t",
            "fromTurnCount": 0,
            "toTurnCount": 1,
            "ignoreWhitespace": false,
        })));
    }
}
