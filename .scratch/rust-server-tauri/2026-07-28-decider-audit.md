# Decider audit — the orchestration core, read against laplus

**Date:** 2026-07-28 · **Read:** `apps/server/src/orchestration/decider.ts` (1,180),
`commandInvariants.ts`, `Layers/CheckpointReactor.ts`, `Layers/ThreadDeletionReactor.ts`,
`provider/Layers/ProviderService.ts`, `ClaudeAdapter.ts:3840` — against
`crate::orchestration`, `crate::threads`, `crate::protocol`.

Written because the decider was the largest thing in the system that had only
been grepped, and grep finds names rather than semantics. Three findings, of
which the third changes what is possible rather than what is done.

---

## 1. The mode reaches the turn by a different route here, and that makes M1 cheaper

Upstream's `thread.turn.start` handler **ignores the mode on the command** and
reads the thread's stored values (`decider.ts:777`):

```js
runtimeMode: targetThread.runtimeMode,
interactionMode: targetThread.interactionMode,
```

That is why the client persists the mode _first_, with
`thread.runtime-mode.set` / `thread.interaction-mode.set` / `thread.meta.update`,
and why its send path treats a failure of any of them as fatal — on upstream
those commands are not bookkeeping, they are **the only way the mode reaches the
turn**.

laplus reads all three off the command instead (`StartTurn`, with
`Option<String>` and an explicit "absent means whatever the thread already had").
`crate::threads:1169` then applies them to the thread row.

So the two servers reach the same place by opposite routes, and laplus's is the
more robust of the two — the mode cannot be lost by a setter that failed.

**The consequence for M1 is that the fix is smaller than the ticket implies.**
laplus does not need the setters to make the mode work; it needs them only so the
client's pre-flight succeeds. Three handlers that write a thread row and publish
`thread.meta-updated` / `thread.runtime-mode-set` / `thread.interaction-mode-set`
restore the send path, and the turn keeps taking its mode from the command as it
already does. There is no ordering problem to solve and no new state to keep.

## 2. Neither server guards a turn behind a running turn — and laplus's queue is better

`thread.turn.start` has no `requireNoActiveTurn`. Upstream instead models the gap
between dispatch and adoption as a **queued turn start**, with a two-minute
grace window (`QUEUED_TURN_START_GRACE_MS`) and a client-clock-skew guard on both
sides of the age check, because message timestamps are client-supplied.

laplus bounds the same window with `PROMPT_QUEUE = 8`, and — the part worth
keeping — puts decisions and interrupts on a **separate** `SIGNAL_QUEUE`, with
the reasoning written down at `threads.rs:184`: a signal is owed _to_ the turn in
flight, a prompt is queued _behind_ it, and sharing one channel would put a
"stop" behind a prompt the driver is deliberately not reading. It then puts all
signal kinds on one channel so that approve-then-stop cannot be reordered.

That is a better-specified concurrency story than upstream's, and it is not a
gap. Recorded so nobody 'fixes' it toward the reference.

## 3. Checkpoint revert cannot be built the way upstream builds it

This is the finding. Upstream's revert (`CheckpointReactor.ts:611–740`) is six
steps:

1. refuse if `turnCount > currentTurnCount`, or if the project is not a git repo;
2. `checkpointStore.restoreCheckpoint({ cwd, checkpointRef, fallbackToHead })`;
3. refresh the workspace entry index, so the `@`-mention picker matches the
   reverted tree;
4. **`providerService.rollbackConversation({ threadId, numTurns })`** — roll the
   _agent's own memory_ back by the same number of turns;
5. `deleteCheckpointRefs` for every checkpoint newer than the target;
6. dispatch `thread.revert.complete`.

laplus can do 1, 2, 3, 5 and 6 today — it has the refs, the diff arithmetic and
the watcher. **Step 4 is the problem**, and it is a property of the transport
rather than of effort.

`ClaudeAdapter.rollbackThread` (`:3840`) truncates its in-memory turn list and
rewrites the resume cursor:

```js
context.turns.splice(nextLength);
// → resumeCursor = { resume: resumeSessionId, resumeSessionAt: lastAssistantUuid, turnCount }
```

`resumeSessionAt` is **an Agent SDK option, not a CLI flag.** Verified against
`claude --help` on 2.1.220: the binary offers `--resume [sessionId]`,
`--session-id <uuid>` and `--fork-session`, and nothing that resumes _at_ a
message. Upstream can rewind because it drives the SDK; laplus drives the binary,
and the binary does not expose the seam.

Two further facts, both checked:

- **laplus discards the identifier this would need anyway.** `MessageEnvelope`
  is `{ message }` and nothing else, so the per-message `uuid` the CLI stamps on
  every assistant line is parsed and thrown away.
- **The CLI's own session store is a parent-linked chain.** Each line of
  `~/.claude/projects/<slug>/<sessionId>.jsonl` carries `uuid` and `parentUuid`
  — a real conversation tree, which is presumably what the SDK walks.

So there are three honest options and no fourth:

| Option                                                                                                                                         | What it costs                                                                                                                                                                                                              |
| ---------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Truncate the CLI's own JSONL** at the target assistant uuid before `--resume`, optionally with `--fork-session` to leave the original intact | Writing into another program's private store, with no compatibility promise. Mechanically sound — the chain is parent-linked — and the only route the binary leaves open. Needs `uuid` captured off assistant lines first. |
| **Restore files only**, and tell the agent nothing                                                                                             | Cheap and truthful about the filesystem. Leaves the agent believing it made changes that are gone, which is the exact state upstream added step 4 to avoid.                                                                |
| **Restore files and start a fresh session**                                                                                                    | Correct in that the agent is never wrong, blunt in that the whole conversation's context goes with it.                                                                                                                     |

The ledger listed M5 as "the infrastructure exists; this is the rewind". That was
wrong: the filesystem half exists, and the agent half needs a decision that
belongs to a person. **M5 should be re-filed as `needs-triage` with these three
options**, not left as ready work.

## 4. Two smaller specs, for when M3 and M6 are built

**Thread deletion** (`ThreadDeletionReactor.ts:58`) is exactly two cleanups, in
order: `providerService.stopSession({ threadId })`, then
`terminalManager.close({ threadId, deleteHistory: true })`. Each is logged and
skipped on failure rather than aborting the other. Note what is _not_ there —
upstream does **not** delete a deleted thread's checkpoint refs, so those leak on
both sides and laplus need not consider it part of M3.

**Session stop** (`decider.ts:922`) is a bare `requireThread` followed by a
`thread.session-stop-requested` event. All the work is in the reactor. For
laplus this is close to free: `crate::threads` already owns session teardown for
the interrupt and restart paths.

## What this pass did not settle

Whether a second prompt sent mid-turn reaches the CLI's stdin during the turn or
after it. `PROMPT_QUEUE`'s comment says the window is "between a turn being
dispatched and the child existing", which suggests after — but the CLI is being
fed `stream-json` on an open stdin, and what it does with a user message that
arrives mid-turn was not tested. That is a capture away and worth one.
