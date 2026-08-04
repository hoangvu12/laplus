# Upstream research — what t3code does when a prompt arrives mid-turn

Written 2026-08-04 against `pingdotgg/t3code` commit
[`c30a6d9b`](https://github.com/pingdotgg/t3code/tree/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52)
(`main`, 2026-08-04T13:35:10Z). Implementation research against the MIT source,
not a spec. No t3code process was run. Read because laplus's composer flips to
**"connecting"** when a prompt is sent while a turn is running, and because
laplus gives OpenCode steering while Claude and Codex queue — nobody had checked
whether that split is upstream's or an accident of the order laplus's drivers
were written.

**The split is an accident.** Upstream steers on _both_ Claude and OpenCode, and
Codex never asks the question at all. And separately from the steering question,
upstream guards the two publishes that produce laplus's "connecting" — so the UI
bug is fixable without deciding the steering question.

## The two answers that matter

### 1. Upstream never publishes `starting` when a turn is already running

`ProviderCommandReactor.ts:496-511` is the only place upstream publishes
`starting` on the turn-start path, and it is guarded:

```ts
if (options?.pendingTurnStart === true && thread.session?.status !== "running") {
  yield *
    setThreadSession({
      threadId,
      session: {
        threadId,
        status: "starting",
        providerName: activeSession?.provider ?? preferredProvider,
        providerInstanceId: activeSession?.providerInstanceId ?? desiredInstanceId,
        runtimeMode: desiredRuntimeMode,
        activeTurnId: null,
        lastError: null,
        updatedAt: createdAt,
      },
      createdAt,
    });
}
```

Source:
[`ProviderCommandReactor.ts#L496-L511`](https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L496-L511).

Two things there that laplus does not do, and they are independent:

- **`thread.session?.status !== "running"`.** A prompt sent into a running
  session publishes no session change at all. laplus publishes one
  unconditionally (`orchestration.rs:1480-1493`).
- **`activeTurnId: null`.** Even on the first turn, upstream's `starting` names
  no turn. laplus publishes `active_turn_id: Some(new_turn_id)` — a turn that
  has not been handed to the agent yet.

`starting` upstream means _the process is coming up_, and nothing else. It is
produced from provider session status `"connecting"`
([`ProviderService.ts#L107-L120`](https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ProviderService.ts#L107-L120)),
and from `"ready"` only while a turn start is pending
(`ProviderCommandReactor.ts:583`). laplus's client renders it as "connecting"
(`apps/web/src/session-logic.ts:1430`), which is upstream's word for upstream's
meaning. laplus reused the status for "a turn was accepted but not yet
dispatched", which is a different thing.

### 2. Upstream's Claude adapter steers; laplus's does not

`ClaudeAdapter.ts:3729-3738`:

```ts
// A sendTurn while a real turn is running is a steer: the message is
// queued into the live SDK agent loop and the work continues as the same
// turn — no synthetic turn boundary. Stale synthetic turns (from
// background agent responses between user prompts) are auto-closed
// instead, so they don't block the user's next turn.
const steeringTurnState =
  context.turnState && context.turnState.synthetic !== true ? context.turnState : null;
if (context.turnState && steeringTurnState === null) {
  yield * completeTurn(context, "completed");
}
```

and `ClaudeAdapter.ts:3771-3803`, where the whole `turn.started` / session-update
block is skipped when steering:

```ts
const turnId = steeringTurnState?.turnId ?? TurnId.make(yield * randomUUIDv4);
if (steeringTurnState === null) {
  const turnState: ClaudeTurnState = { turnId /* … */ };
  context.turnState = turnState;
  context.session = { ...context.session, status: "running", activeTurnId: turnId, updatedAt };
  yield * offerRuntimeEvent({ type: "turn.started", /* … */ turnId /* … */ });
}
```

The prompt then goes into the live agent loop unconditionally
(`ClaudeAdapter.ts:3811-3814`):

```ts
yield * Queue.offer(context.promptQueue, { type: "message", message });
```

`promptQueue` is the `Stream.fromQueue(promptQueue)` that _is_ the SDK query's
input iterable (`ClaudeAdapter.ts:3181-3182`). So **yes to question 5**: upstream
writes the second prompt into the Claude agent's input mid-turn and does not wait
for `result`. laplus waits (`session.rs:772`, `waiting = Some(prompt)`).

Source:
[`ClaudeAdapter.ts#L3722-L3823`](https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ClaudeAdapter.ts#L3722-L3823).

## Per-provider answers

|                               | Claude                                | Codex                                   | OpenCode                                         |
| ----------------------------- | ------------------------------------- | --------------------------------------- | ------------------------------------------------ |
| prompt while busy             | **steer**                             | passes straight through to `turn/start` | **steer**                                        |
| new turn id?                  | no, reuses `turnState.turnId`         | yes, whatever `turn/start` returns      | no, reuses `context.activeTurnId`                |
| session status published      | none (stays `running`)                | `running`, after the response           | `running`                                        |
| written to the agent mid-turn | yes, `promptQueue` → SDK input stream | yes, a second `turn/start` JSON-RPC     | yes, `session.promptAsync` into the busy session |
| laplus today                  | queue                                 | queue                                   | steer                                            |

**Codex is not a third policy — it is an absence of one.** `CodexAdapter.ts`'s
`sendTurn` has no busy check; it resolves attachments, requires a session, and
calls `session.runtime.sendTurn`
([`CodexAdapter.ts#L1531-L1563`](https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/CodexAdapter.ts#L1531-L1563)).
The runtime issues `turn/start` and takes the turn id **from the response**, then
publishes `status: "running", activeTurnId: turnId`
([`CodexSessionRuntime.ts#L1305-L1320`](https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/CodexSessionRuntime.ts#L1305-L1320)).
Whatever concurrency policy exists is the Codex app-server's, and upstream
inherits it unexamined. Note the shape anyway: **the id is never published before
the provider has accepted the turn.**

OpenCode, for completeness (`OpenCodeAdapter.ts:1412-1418`):

```ts
// A sendTurn while a turn is active is a steer: OpenCode queues the
// prompt into the busy session and the work continues as one turn, so
// the active turn id is reused instead of opening a new turn.
const steeringTurnId = context.activeTurnId;
const turnId = steeringTurnId ?? TurnId.make(`opencode-turn-${yield * randomUUIDv4}`);
```

This matches what `.scratch/opencode-driver/upstream-research.md` recorded on
2026-08-01 against commit `0ad91b6e`, and is what laplus already implements as
`STEERS_ACTIVE_TURN = true` (`opencode.rs:1334`).

**Question 2 — is the decision per-provider?** There is no capability flag.
Nothing upstream resembles laplus's `STEERS_ACTIVE_TURN`. Each adapter's
`capabilities` object carries only `sessionModelSwitch: "in-session"`. Steering
is a property each adapter expresses in its own `sendTurn`, and both adapters
that can express it do. The orchestration layer above them is uniform: it
resolves the thread, publishes `starting` _only if not already running_, and
forks `providerService.sendTurn` (`ProviderCommandReactor.ts:1099-1119`).

**Question 4 — is a queued prompt in the transcript immediately?** Yes, and
under the _steering_ turn id, because that is the id `sendTurn` returns. The
client half of that is visible in laplus's own vendored client:
`ChatView.logic.ts:521-527` has a branch for it —

```ts
// Steering adds a user message to the current running turn without
// necessarily changing any of the turn timestamps. Treat that projected
// message as the server acknowledgment so the composer does not remain
// stuck in its local "Sending" state until the turn settles.
```

## What this means for the laplus bug

The reported symptom has **two independent causes**, both from the single
unconditional publish at `orchestration.rs:1480-1493`, and upstream guards both.

**The label.** `status: Starting` → `derivePhase` returns `"connecting"`
(`session-logic.ts:1430`). Upstream would have published nothing here.

**The streaming.** `active_turn_id: Some(new_turn_id)` moves the client's
`session.activeTurnId` off the turn that is actually running. The reducer's
mid-turn-settle guard is:

```ts
// packages/client-runtime/src/state/threadReducer.ts:268-272
const turnStillRunning =
  event.payload.turnId !== null &&
  thread.session?.status === "running" &&
  thread.session.activeTurnId === event.payload.turnId;
const settlesTurn = !event.payload.streaming && !turnStillRunning;
```

After the publish, **both** conjuncts are false — status is `starting`, and
`activeTurnId` names the queued turn. So the next completed assistant message on
the still-running turn sets `settlesTurn = true` and marks `latestTurn` as
`completed` while the agent is still working. That is precisely the mid-turn
settle that `session.rs:620-624` says the reducer exists to avoid, reintroduced
from the other side.

Note what does _not_ happen, contrary to the handoff's guess: `latestTurn` is
**not** moved to the new turn. `settledTurnStateForSessionStatus("starting")`
returns `null` (`threadReducer.ts:551-553`), so the `thread.session-set` case
leaves `latestTurn` alone. The damage is done later, by the next assistant
message, through the guard above.

### Two separable decisions

1. **Stop publishing a session change when a turn is in flight.** Upstream's
   guard, and correct under either steering policy. Fixes the label and the
   mid-turn settle. Does not touch the `Starting` publish for the first turn,
   which `session.rs:604-641` explains is load-bearing (it is what makes the
   composer answer before a `git add -A` baseline).

2. **Whether Claude and Codex should steer.** Upstream says yes for Claude, and
   `STEERS_ACTIVE_TURN = false` is a divergence rather than a considered choice.
   But it is a behaviour change, not a bug fix: laplus's queue path carries a
   per-turn runtime mode that is spent when its own turn comes (`session.rs:604-616`,
   ticket 02's rule), and steering has nowhere to put that. Decide it separately.

The laplus `Starting`-means-connecting collision is worth naming on its own:
laplus has both `Idle` and `Ready`, and the contract declares a `waiting` runtime
state that laplus's `SessionStatus` does not carry
([`providerRuntime.ts` `RuntimeSessionState`](https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/packages/contracts/src/providerRuntime.ts)).
If a queued turn ever needs to say something, it is not `starting`.

## Primary-source index

- Repository at researched commit:
  <https://github.com/pingdotgg/t3code/tree/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52>
- Claude adapter `sendTurn` (steering):
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ClaudeAdapter.ts#L3722-L3823>
- Codex adapter `sendTurn` (no busy check):
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/CodexAdapter.ts#L1531-L1563>
- Codex session runtime `turn/start`:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/CodexSessionRuntime.ts#L1280-L1329>
- OpenCode adapter `sendTurn` (steering):
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1412-L1418>
- The guarded `starting` publish:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L496-L511>
- `starting` from provider `connecting`:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ProviderService.ts#L107-L120>
- Earlier OpenCode-side research in this repo:
  `.scratch/opencode-driver/upstream-research.md`
