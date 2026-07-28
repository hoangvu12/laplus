# 19 — Working tree status with live refresh

**What to build:** A developer sees what has changed in their working tree, and it
stays accurate while they work — updating as the agent edits files rather than
needing a manual refresh. This is how the developer tells what the agent actually
did.

Git is driven by shelling out to the installed `git` binary. No library linkage in
v1.

**Blocked by:** 05 (Project registry), 04 (First streaming subscription).

**Status:** done

- [x] Working tree status shows modified, added, deleted and untracked files
- [x] Status refreshes as files change on disk, without manual action
- [x] The current branch is shown
- [x] A project that is not a repository is reported as such rather than as an
      error
- [x] A missing `git` binary produces a clear diagnostic
- [x] Status in a very large repository does not stall the UI, and rapid changes
      are coalesced rather than triggering a refresh per file
- [x] Repositories in detached-HEAD or mid-merge states are reported without
      crashing
- [x] Tests drive status and live refresh through the socket boundary against a
      temporary repository

## Comments

### What was built

Two methods, in `crates/lightcode-server/src/git.rs`:

- `vcs.refreshStatus` — the panel's refresh button. Deferred work, like every
  other method that waits on the world.
- `subscribeVcsStatus` — the panel itself. **Never runs git.** It describes
  itself from the last read and is fed by a refresh thread; see ADR-0006.

`crate::filesystem::Index` grew two things to make that possible: `on_change`,
so a second subsystem can hear the one watcher rather than starting another,
and `observe`, so a subscription can ask for a workspace to be watched without
scanning it.

### The decision worth reading

**ADR-0006** — a change marks a working tree stale, it does not read it. That
one sentence is where the coalescing, the no-lost-change property and the
single-reader-per-repository property all come from. The sharpest edge in it:
`git status` rewrites `.git/index` unless told not to, which without
`--no-optional-locks` is a read that causes the next read forever.

### What the capture said, and where this diverges

`fixtures/socket-wire/01-browser-session.ndjson` holds a real
`subscribeVcsStatus` — the only capture of a status there is. The reference
server opens with `remote: null` and follows with a `remoteUpdated`, because
its remote half costs a network round trip.

**lightcode sends both halves in the snapshot.** `aheadCount` and `behindCount`
come from the tracking ref, which is local and read in the same breath as
everything else, so there is no later moment at which the remote half could
arrive. `pr` is always null and `sourceControlProvider` is omitted, both because
source-control hosting is out of v1's scope. All three divergences are declared
and enforced by `the_working_tree_status_snapshot_conforms_to_the_capture` in
`tests/socket_conformance.rs`, so a declaration that stops being true fails.

### What the review caught

Two real bugs, both fixed with a test that fails without the fix:

- **Untracked line counts were read from the wrong place.** Porcelain names
  paths relative to the _repository_ root, not the workspace root, so a project
  opened as a package inside a monorepo looked for
  `packages/web/packages/web/…`, found nothing, and reported the agent's brand
  new file as `+0`. `counts` now resolves the work tree root once, and only when
  there is something to count with it.
  (`a_project_inside_a_larger_repository_is_read_from_the_repository_root`)
- **A truncated record could blank the whole panel.** A path is a
  `TrimmedNonEmptyString`, so one empty string would fail the client's decode of
  the entire status rather than of one row. `Porcelain::name` drops blank paths.
  (`a_truncated_record_costs_its_own_row_and_not_the_status`)

And one honesty fix: a spawn failure that is not `NotFound` no longer reports
"git is not installed", which would send a developer to install something they
already have.

### One trade-off worth knowing about

**A read that fails leaves a subscription silent.** `VcsStatusStreamEvent` has
no error variant, so a stream that has already opened has only two options —
invent a status the server does not believe, or say nothing. It says nothing,
logs once, and `vcs.refreshStatus` is the door that carries the real refusal to
the developer, which is the panel's own refresh button. Named in
`refresh_until_settled`.

### Three things left short of what a reader might assume

- **The missing-`git`-binary diagnostic is not driven through the socket.**
  `PATH` is process-global mutable state, so a test that emptied it would be
  changing it for every test running beside it. What is pinned is the mapping —
  `Unavailable::NotInstalled` to a `GitCommandError` naming git and `PATH` —
  the same call the repo already made for unreadable directories in
  `projects.rs`. The subscription checks for git with a `PATH` walk at subscribe
  time, because a stream that has already opened has no error frame in this
  union to send; the unary call finds out from its own spawn failing.
- **The changed-file list is capped at 5,000** (`MAX_FILES`). `VcsStatusResult`
  has no `truncated` flag, unlike the file tree's listing, so there is nowhere
  to say the list was cut. What is bounded is only the list: `insertions`,
  `deletions` and `hasWorkingTreeChanges` are computed over every changed file,
  so the summary stays exactly right. A cut is logged.
- **Untracked line counts are read from disk under a budget** (8 MiB per
  refresh, 1 MiB per file). `git diff` has nothing to say about a file it has
  never seen, and an agent's brand-new file showing `+0` would be the wrong
  answer to the question this ticket exists for. Past the budget, files count as
  zero rather than the refresh becoming unbounded.

### Not done here, deliberately

`Project::repository_identity` is still `null`. `projects.rs` names tickets
19–21 as its filler, and it is a _project registry_ field — the repository's
canonical root and metadata path — rather than a working tree status. Ticket 21
adds `vcs.init` and branch listing, which is where a project's repository
identity is actually established.
