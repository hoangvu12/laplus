# 16 — Concurrent sessions across projects

**What to build:** A developer runs more than one conversation at a time — across
different projects — and they stay independent. Output from one never appears in
another, each runs in its own project directory, and stopping one leaves the others
running.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** done

**This is a testing ticket, and the design is settled.** An architecture review
proposed splitting `crate::threads` into a conversation log and a running-agent
registry to serve the "server state is per-session" criterion below. That was
grilled and rejected on measurement, and the audit it prompted answers most of
this ticket in advance. What follows is decided; do not re-open it without new
evidence.

- **There is no process-global mutable state in the crate.** No `static mut`, no
  `OnceLock` singleton, no `set_var`, no ambient `current_dir` on the agent path.
  Every piece of state hangs off an `Arc` owned by one server instance and is
  keyed per-thread: `Entry` gives each conversation its own state, its own event
  feed and its own `Live`. The three genuinely shared things are shared on
  purpose — `Sequences` (one ordering across the whole feed, which the client
  relies on to drop rather than reorder), the `shell` broadcast (the project
  list), and the observability gauges. **None of those is bleed-through**, so the
  seventh criterion is expected to pass as built. The job is to demonstrate it,
  not to build it.

- **"Independent" within one project means server-side isolation only.**
  Transcripts, events, sessions and subprocesses never cross. Both agents run in
  the _same folder at the same time_ and may edit the same files; last write
  wins. That is a documented limitation, not a defect. Upstream isolates
  same-project threads with git worktrees and the spec excludes worktrees by
  name — `orchestration.rs` refuses to prepare one in so many words — so this
  server cannot offer conflict-freedom and should not pretend to. The ticket's
  word is _independent_, not _conflict-free_, and they are different promises.

- **The deliverable is `socket_concurrency.rs`, one case per criterion.** Not a
  single "two conversations stream" test: that would tick three boxes on the
  strength of the other five looking correct. Criteria 4 and 6 are the ones worth
  the effort, because interrupting one session and deleting one project are where
  `forget`, `Inner::winding_down` and `shutdown` interact, and those paths have
  only ever run with a single agent. No stress case — sixteen scripted `cmd.exe`
  processes would test the machine rather than the server.

- **Two harness constraints, already checked.** `binaryPath` is one server-wide
  setting, so both sessions spawn the _same_ `ScriptedAgent`, whose `starts()`,
  `arguments()` and `answers()` logs are shared files that two concurrent
  processes would interleave or lock. Assert on per-project observables instead —
  each project's `WORKING_DIRECTORY_MARKER`, and each subscription's events. Use
  **one** socket connection with two subscriptions, which is both what the real
  app does (one window, one WebSocket) and safe, because `values_until` buffers
  frames that do not match rather than dropping them.

- **A structural finding comes back to the developer.** Fix anything that can be
  fixed inside the existing shape. Anything that cannot — anything that changes
  who owns a driver's lifetime, or wants a seam the spec's Testing Decisions rule
  out — is a new issue and a decision to be asked for, not taken.

- **Close with ADR-0002** recording why the split was rejected: the running-agent
  half is ~260 of `threads.rs`'s 2,511 lines, not half of it; it is covered by
  eight cases in `socket_interrupt.rs` plus `socket_permissions.rs`,
  `socket_turn.rs` and `socket_continuity.rs`, so "zero tests" was true only of
  inline unit tests; and upstream's own primary seam is elsewhere — `projector.ts`
  is a pure fold with no I/O, and lightcode has ~730 equivalent lines of
  fold-and-render sitting un-separated in `threads.rs`. Name that pure-fold cut as
  the one worth taking if the file ever does need splitting, so the next review
  starts there rather than re-deriving it.

- [x] Two conversations in different projects can stream simultaneously — proved
      by the shared sequence counter rather than by a clock: each turn's streamed
      deltas occupy a span of sequences and the two spans overlap
- [x] Output, transcripts and session state never cross between conversations —
      checked three separate ways, because a server could get any two of them
      right
- [x] Each agent subprocess runs in its own project's working directory — one
      conversation takes a turn at a time, so the marker is attributable rather
      than merely present in both folders
- [x] Ending or interrupting one session does not disturb the others — the
      untouched conversation keeps its session, its child and its ability to take
      another turn
- [x] Two conversations within the same project remain independent — _server-side_,
      which is the whole of what this server can promise. See ADR-0003
- [~] Subprocesses are tracked per session so none is orphaned when one ends —
  deleting one project drops the live-agent gauge to exactly one and leaves
  the other conversation answering. The _tracking_ half is proved; whether a
  released driver was reaped rather than detached is not observable at this
  seam. See the comments below
- [x] Server state is per-session rather than global, with no shared mutable
      bleed-through — the three genuinely shared things are asserted to aggregate
      rather than merge
- [x] Tests drive two concurrent sessions through the socket boundary and assert
      their isolation — `tests/socket_concurrency.rs`, seven of them

## Comments

### Nothing was built; two things were fixed in the harness

As predicted, the server needed no change. What the ticket cost was two harness
gaps, both of which were "one project" baked into helpers written when there was
only ever one:

- `conversation::start_turn` hard-coded `bootstrap.createThread.projectId` as
  `project-1`, so every conversation any test could create belonged to the same
  project. `start_turn_for` takes the project; the two existing spellings delegate
  to a shared private builder, so there is still one copy of what the composer
  sends.
- `SocketClient::open_conversation` registered the folder as `project-1`.
  `open_conversation_in` takes the id. A second project has to be a second
  _folder_ as well, because `project.create` refuses a root another project
  already holds.

### Proving "simultaneously" without asserting on a clock

The first criterion is the only one that is a claim about _time_, and the first
attempt at it failed on a real machine: with both conversations dispatching their
opening turn together, the second child was still being spawned when the first
had finished its reply. Spawning a `claude.cmd` through `cmd.exe` costs a
noticeable fraction of a second, and more with the rest of the suite doing the
same beside it.

Two changes made it deterministic rather than lengthening the pause until it
usually worked:

- **Both conversations take a warm-up turn first**, so both children already
  exist when the turn under test is dispatched. The only gap left between the two
  dispatches is the time to write a second frame.
- **The assertion is on sequence numbers, not on timing.** `Sequences` is shared
  across the whole feed by design, so each turn's streamed deltas occupy a span of
  it, and two spans that overlap mean the second conversation published a delta
  before the first had published its last — and the other way round. No ordering
  of "one turn, then the other" produces that.

### The shared binary shapes the interrupt case

`binaryPath` is one server-wide setting, so both conversations spawn the _same_
`ScriptedAgent` and replay the same lines. That rules out asserting on
`starts()`, `arguments()` or `answers()` — two concurrent processes would
interleave or lock those log files — and the per-project observables are used
instead.

It also has a sharper consequence. A script stop (`AWAIT_ANSWER`) blocks the child
until the server writes to it, and the server only writes to the child it is
interrupting, so an _undisturbed_ conversation that reached that script would hang
there forever. The stop is therefore on the third turn of a process, which only
the interrupted conversation ever reaches. The first draft put it on the first
turn and hung, which is how the constraint was found.

### Two assertions the review caught passing for the wrong reason

Both were found by the spec review, and both were tests that were _true_ but did
not prove what their names claimed:

- **"Each agent runs in its own project's working directory."** The first version
  drove both conversations together and asserted a marker in each folder. Both
  children write the same filename, so a server that started each child in the
  _other_ project's folder would produce exactly the same two markers. Fixed by
  taking the turns one at a time: after the first conversation's turn, exactly one
  folder has a marker, and it is that conversation's.
- **"No subprocess is orphaned."** `Threads::forget` decrements `live_agents`
  itself, in the same call that removes the entry, so `await_live_agents(1)`
  returns on its first poll and the comment claiming it "settles a moment
  afterwards" was simply wrong. What the gauge proves is that the tracking is
  _per session_ — one child released, one not. Whether the released driver was
  reaped rather than detached turns on `shutdown` awaiting the handle
  `winding_down` holds, and that is invisible through the socket: a handle dropped
  instead of parked would pass. The criterion is marked `[~]` for that reason and
  the test says so where a reader will meet it. Closing the gap needs a
  `#[cfg(test)]` unit in `threads.rs`, which is a decision to ask for rather than
  take — the spec's Testing Decisions put the bulk of testing at this seam
  deliberately.

### Two ADRs

- **ADR-0002** records why `threads.rs` is not split into a conversation log and a
  running-agent registry, with the three measurements that killed the proposal,
  and names the pure-fold cut as the one worth taking if the file ever does need
  splitting.
- **ADR-0003** records that _independent_ and _conflict-free_ are different
  promises, and that this server offers only the first for two conversations in
  one project. **This one was not asked for.** The ticket names ADR-0002 only,
  and says the same-project limitation is "a documented limitation" without
  saying where it should be documented. It went in `docs/adr/` because that is
  where `docs/agents/domain.md` sends someone exploring the area, and a tracker
  file under `.scratch/` is not somewhere an architecture review looks. Delete it
  if the ticket's silence was meant as "the ticket is the documentation".
