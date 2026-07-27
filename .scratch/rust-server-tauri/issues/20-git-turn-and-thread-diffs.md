# 20 — Turn and thread diffs

**What to build:** A developer reviews the agent's work as a diff. They can look at
what changed during a single agent turn, to review one step in isolation, or at the
cumulative change across a whole conversation, to review the session as one
coherent change.

This requires turns to be identifiable points in time against the working tree, so
it depends on the agent session lifecycle as well as on git.

**Blocked by:** 19 (Working tree status with live refresh), 10 (One complete agent
turn, streamed).

**Status:** ready-for-human

- [x] The diff for a single agent turn can be viewed
- [x] The cumulative diff for an entire conversation can be viewed
- [x] Diffs cover added, modified, deleted and renamed files
- [x] A turn that changed nothing shows an empty diff rather than an error
- [x] Untracked files created by the agent appear in the diff
- [x] Binary file changes are indicated without attempting to render content
- [x] Very large diffs are truncated for display with the truncation made obvious
- [x] Diffs remain correct when the developer edits files by hand between turns
- [x] Tests drive both diff views through the socket boundary against a temporary
      repository

## Comments

### What was built

Two methods and the thing that makes them answerable.

- `orchestration.getTurnDiff` and `orchestration.getFullThreadDiff`, in
  `crates/lightcode-server/src/checkpoints.rs`. Both deferred, like every method
  that runs git. They are one call over a different range — a thread diff is a
  turn diff whose `fromTurnCount` is zero, which is the contract's own framing.
- **A checkpoint**: at every turn boundary the whole working tree is written as a
  parentless commit under `refs/lightcode/checkpoints/<thread>/turn/<n>`, and a
  diff is `git diff` between two of them. `crate::turn` takes them — a baseline
  before the prompt reaches the agent, and one each time a turn stops being in
  flight.
- `Thread::checkpoints` is now filled rather than `[]`, published as
  `thread.turn-diff-completed`, and stored (schema v4). That list is what the
  diff panel offers turns from, so without it the two methods would be
  unreachable from the UI.

### The decision worth reading

**ADR-0008** — a turn is a photograph of the working tree, not a record of who
typed. Five of the nine criteria above are properties of `git diff` between two
commits rather than code: added/modified/deleted/renamed, the empty diff, the
untracked file, the binary file. The one that is a *decision* is the eighth: a
hand edit between two turns belongs to the turn it happened during, because a
photograph cannot know otherwise — and because the question the panel asks is
"what is different now", not "who typed it".

The sharpest edge is the same one ADR-0006 has: writing a checkpoint writes a ref
under `.git`, which the status watcher sees. `refs/lightcode/checkpoints` is
excluded from what counts as a working tree change, or every turn would cost a
`git status` for a record that changed nothing.

### What the review caught

Three real bugs, all in the same seam — what a checkpoint *claims* — and each
fixed with a test that fails without the fix.

- **A checkpoint's `status` is how the turn went, not whether the capture
  worked.** Every row went out as `ready`, and the client feeds that straight
  back into the turn: `threadReducer.ts` sets `latestTurn.state` to
  `checkpointStatusToTurnState(status)` on every checkpoint it folds, and
  `ready` means `completed`. So a *failed* turn was being quietly relabelled as
  a clean one a few hundred milliseconds after the session said it had failed.
  Derived from `Ending` now.
  (`a_turn_that_failed_is_recorded_as_a_failure_rather_than_a_clean_one`)
- **A turn the developer stopped gets no checkpoint at all.** The same rule with
  no good answer: the contract's three statuses map to `completed`, `completed`
  and `error`, so *none of them means interrupted* and any row would undo what
  ticket 14 settled. Upstream sends `missing` and takes the relabelling; this
  does not. The cost is named in ADR-0008 and driven by a test — the stopped
  turn's work is reviewed as part of the turn that follows it.
  (`a_turn_the_developer_stopped_is_not_offered_for_review_on_its_own`)
- **A stop click could land on the wrong turn.** Not a diff bug at all, but this
  ticket created it: the driver now `await`s a `git add -A` at the end of every
  turn, and a stop arriving during it was queued behind the next prompt and
  applied to the turn that prompt started. Signals are drained at the top of the
  loop now, which is what the module always claimed — "a signal is never queued
  behind a prompt" — made literally true. Caught by ticket 14's own
  `stopping_when_nothing_is_running_does_nothing_and_says_so_by_succeeding`,
  which began failing about one run in three.

And one shape fix: a checkpoint is keyed by **turn id**, in the fold and in the
table, because that is the client's key (`threadReducer.ts` filters on
`entry.turnId`). Keying on the turn count would have let one turn be one row here
and two in the panel.

### Where this diverges from upstream

The reference server's `getTurnDiff` is the same shape and the same plumbing —
temporary index, `add -A`, `write-tree`, `commit-tree` — which is why this
follows it rather than inventing something. Three differences, all deliberate:

- **The refs are `refs/lightcode/…` rather than `refs/t3/…`**, and the thread id
  is **hex-encoded** rather than base64url. A ref name is a path and a thread id
  is not, and two conversations sharing a checkpoint is the one failure here that
  would show a developer somebody else's diff. Hex cannot collide.
- **The `kind` on a checkpoint's file summary is real.** Upstream writes
  `"modified"` for every file; this reads `--name-status` beside `--numstat` and
  says added, modified, deleted or renamed.
- **The scratch index lives in the system temporary directory**, not in the git
  common directory. Everything under `.git` that is not an object or a log is
  watched, so an index written and deleted inside it would cost a status refresh
  per checkpoint.

### Three things left short of what a reader might assume

- **The refs are never deleted.** A long-lived project accumulates one ref and
  one commit per turn, and the objects stay alive against `git gc`. That is
  deliberate for v1 — a diff a developer can still open tomorrow is the point —
  and it is the loose end. `git for-each-ref refs/lightcode` finds them all and
  `git update-ref -d` removes one, which is the whole of the cleanup a later
  ticket would add. Upstream has `deleteCheckpointRefs`; nothing here calls an
  equivalent, including on project delete.
- **There is a window at the end of a turn.** The capture happens after the turn
  settles, because blocking "the agent is done" on a `git add -A` would make a
  large repository feel like a slow agent. An edit made inside that window — a
  fraction of a second — falls into the next turn. `thread.turn-diff-completed`
  is what says the window has closed, and the test harness waits for it
  (`events_through_the_checkpoint`) rather than for the settle.
- **A capture re-hashes the working tree.** The scratch index starts from `HEAD`
  with no stat cache, so `add -A` reads every file git does not already know the
  content of — the cost of a cold `git status`, once per turn boundary. This is
  why checkpoints are taken at turn boundaries and nowhere else.

### What the harness gained

`ScriptedAgent` can now change the project mid-turn (`writes`, `deletes`). Every
ticket before this one could drive a turn against an agent that only talked; a
diff of a turn that only talked is empty by definition. The agent writes through
the working directory the server started it in, so the server is not told and
finds out the same way it would find out about the real thing.
