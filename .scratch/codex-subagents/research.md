# Codex subagent visibility research

Written 2026-08-02. This answers whether laplus can show Codex subagents while
they are running and when they finish, and where that support belongs.

> **Corrected 2026-08-03 by a live recording.** This document was written from
> source alone — it says so below — and one recorded turn contradicts its
> central mechanism. `fixtures/codex-app-server/09-subagent-spawn.jsonl` is that
> recording, and its README entry is now the authority where the two disagree.
>
> - **`agentsStates` arrives empty**, on both the started and completed `wait`.
>   Every row in the mapping table below that keys off an `agentsStates` entry
>   therefore never fires. Completion does _not_ come from a collaboration-call
>   snapshot.
> - **No `spawnAgent` call is emitted.** The spawn appears only as a
>   `subAgentActivity` with `kind: started`; the only collaboration call is the
>   `wait`. The "render the operation as one row and the agent as a second" shape
>   still holds, but the child's id comes from the activity, not from a spawn.
> - **Child-thread routing was not optional.** The closing recommendation to
>   treat it "as a separate feature" is wrong: the child's own `turn/completed`
>   is the _only_ completion signal Codex sends, so without routing it a subagent
>   row starts and never finishes. It is implemented, and it is what works.
> - The warning that child routing could wrongly settle the parent turn was
>   sound, and is guarded — but the hazard is subtler than described. A child's
>   first `thread/status/changed` arrives one frame _before_ the activity naming
>   it, so routing keyed on already-known agents lets that status through.

Sources are pinned to `openai/codex` commit
[`2b5bdcf67547`](https://github.com/openai/codex/tree/2b5bdcf67547860f2e5c5a605009a70026796b2b)
and `pingdotgg/t3code` commit
[`e60821f0e0d8`](https://github.com/pingdotgg/t3code/tree/e60821f0e0d82a5d671ca3b94719c49d333921c8).
The laplus observations are against this worktree. This was source research;
no live Codex session was recorded.

## Verdict

**Yes.** The current Codex app-server protocol carries enough information to
show a subagent as started/running, identify it by thread ID and canonical path,
and later show terminal states when Codex reports an agent-state snapshot. The
smallest honest implementation is server-side decoding plus existing work-log
activities; no contract or new React component is required.

Do not equate the lifecycle of a `collabAgentToolCall` with the lifecycle of the
agent. A successful `spawnAgent` call becomes `completed` as soon as the spawn
operation returns, while its `agentsStates` entry may say the new agent is
`running`. Render the collaboration operation as one row and the agent itself
as a second, stable row.

There is one limitation: the v2 `subAgentActivity` enum has `started`,
`interacted`, and `interrupted`, but no `completed` member. A terminal agent
state is carried in `agentsStates` on collaboration-call snapshots (commonly a
later `wait`, send, or close result). Therefore a first implementation can show
completion whenever the parent observes such a snapshot, but cannot promise an
unsolicited completion event for every agent. Full child-thread routing is a
larger follow-up.

## Whether the parent agent is told a subagent finished

This is not laplus's to answer, and the distinction matters before anyone goes
looking for a bug here. The parent learns of a subagent's completion two ways,
[both internal to the Codex process](https://learn.chatgpt.com/docs/agent-configuration/subagents):
the model calls the `wait` tool, which blocks until the agents reach a final
status and returns their final message; or a mailbox notification is delivered
into the parent's own tool loop. Neither crosses the app-server wire, so laplus
can neither observe nor trigger it.

The consequence: if the parent model spawns and then ends its turn without
waiting, the turn is genuinely over. laplus reporting the composer idle is
correct, and having to ask "are they done?" by hand is the expected outcome
rather than a missing feature. The recorded turn shows the good path — the model
called `wait`, and answered from the child's result.

Two settings shape how often that happens, and both are Codex's, not laplus's.
`thread/start` answers with `multiAgentMode`, which was `explicitRequestOnly` on
the recorded run: subagents are spawned only when asked for. And the spawning
prompt is what decides whether `wait` is called at all — an instruction in
`AGENTS.md` telling the agent to wait for what it spawns changes this behaviour,
where no amount of server work would.

## What Codex sends

Every `item/started` and `item/completed` notification wraps a thread item and
also includes the owning `threadId` and `turnId`. The two relevant item shapes
are part of the official generated v2 API:

```text
collabAgentToolCall {
  id,
  tool: spawnAgent | sendInput | resumeAgent | wait | closeAgent,
  status: inProgress | completed | failed,
  senderThreadId,
  receiverThreadIds: string[],
  prompt?, model?, reasoningEffort?,
  agentsStates: Record<threadId, { status, message? }>
}

subAgentActivity {
  id,
  kind: started | interacted | interrupted,
  agentThreadId,
  agentPath
}
```

The source descriptions explicitly say that `receiverThreadIds` contains the
new thread for a spawn and that `agentsStates` is the last known status of each
target agent ([official generated SDK, fields](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/sdk/python/src/openai_codex/generated/v2_all.py#L7238-L7283)).
The collaboration tools and call statuses are defined by Codex itself
([items.rs](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/protocol/src/items.rs#L274-L321)).

Agent states are richer than call states: `pendingInit`, `running`,
`interrupted`, `completed`, `errored`, `shutdown`, and `notFound`; completed
and errored states may carry the final message/error
([protocol.rs](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/protocol/src/protocol.rs#L1734-L1755),
[v2 conversion](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/app-server-protocol/src/protocol/v2/item.rs#L1181-L1224)).
Codex derives an agent's `running` state from its child `TurnStarted` and its
`completed` state (including final assistant text) from child `TurnComplete`
([status.rs](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/core/src/agent/status.rs#L1-L26)).

The correlation keys are sufficient:

- parent operation: item `id`;
- parent/root identity: notification `threadId` and `senderThreadId`;
- child identity: `receiverThreadIds[]` / `agentThreadId`;
- human label and hierarchy: `agentPath` (for example `/root/worker`).

The spawn-begin history item deliberately has no receiver yet; spawn-end adds
the newly allocated receiver and its current state. A failed/no-receiver spawn
is marked failed
([thread_history.rs](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/app-server-protocol/src/protocol/thread_history.rs#L881-L932)).
This is why the started row must tolerate a missing child ID.

Codex's own TUI treats `subAgentActivity.started` as a running hint,
`interrupted` as not running, and intentionally ignores `interacted` for that
boolean display; it separately records readable “Started/Interacted
with/Interrupted `<path>`” history
([multi_agents.rs](https://github.com/openai/codex/blob/2b5bdcf67547860f2e5c5a605009a70026796b2b/codex-rs/tui/src/multi_agents.rs#L281-L333)).

## What T3 upstream currently does

T3 recognizes `collabAgentToolCall` as the canonical item type
`collab_agent_tool_call`, and maps generic `item/started` and `item/completed`
to runtime lifecycle events
([CodexAdapter.ts](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/apps/server/src/provider/Layers/CodexAdapter.ts#L217-L231),
[lifecycle mapper](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/apps/server/src/provider/Layers/CodexAdapter.ts#L459-L497)).
That generic mapper forces `inProgress` for every started item and `completed`
for every completed item, rather than reading the collaboration item's own
status or nested agent states.

The UI then discards every `tool.started` row before deriving its work log
([session-logic.ts](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/apps/web/src/session-logic.ts#L628-L641)).
Consequently upstream does not give an immediate visible running row through
this path. It does, however, remember every collaboration receiver thread and
reroute child notifications to the parent turn while suppressing child thread
and turn boundary notifications
([CodexSessionRuntime.ts](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/apps/server/src/provider/Layers/CodexSessionRuntime.ts#L601-L646),
[routing](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/apps/server/src/provider/Layers/CodexSessionRuntime.ts#L842-L887)).
That is useful precedent for a later nested-activity feature, not a prerequisite
for a running indicator.

Claude is already a useful UI comparison: its adapter emits
`task.started`/`task.progress`/`task.completed`, but the UI hides `task.started`
and renders progress/completion. The adapter mapping is explicit
([ClaudeAdapter.ts](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/apps/server/src/provider/Layers/ClaudeAdapter.ts#L2678-L2743)).
Codex should use the tool lifecycle instead because its protocol provides
stable agent IDs and explicit states, not periodic task progress.

## Laplus seams and recommended mapping

Laplus currently classifies both item types as deliberate unsupported drift in
[`server/fixtures/codex-app-server/README.md`](../../server/fixtures/codex-app-server/README.md),
and `ConversationState::fold_notification` sends unrecognized item types to its
unknown-event path in
[`codex_protocol.rs`](../../server/crates/laplus-server/src/codex_protocol.rs).
The decoder is the correct first seam: add narrow serde/JSON extraction and
`ConversationFold` variants for collaboration calls and subagent activities,
keeping unknown enum values forward-compatible.

`decide` in [`codex.rs`](../../server/crates/laplus-server/src/codex.rs) is the
second seam. Translate the folds through helpers in
[`worklog.rs`](../../server/crates/laplus-server/src/worklog.rs). That module
already deliberately emits visible running operations as `tool.updated` with
`status: inProgress`, because this UI filters `tool.started`, and supplies a
stable `data.toolCallId` for lifecycle collapsing. Reuse that policy.

Recommended rows:

| Input                                | Visible row                                        | Lifecycle                                                                 |
| ------------------------------------ | -------------------------------------------------- | ------------------------------------------------------------------------- |
| collaboration call starts            | “Starting subagent”, “Waiting for subagents”, etc. | `tool.updated`, `inProgress`, collapse key from call `id`                 |
| collaboration call ends              | “Spawned…”, “Waited…”, or failure                  | `tool.completed`, status from the call itself                             |
| activity `started`                   | “Subagent `/root/worker`”                          | `tool.updated`, `inProgress`, stable collapse key `agent:<agentThreadId>` |
| activity `interacted`                | update/detail on that same agent                   | retain `inProgress`; do not create a success row                          |
| activity `interrupted`               | same agent row                                     | `stopped` (label it interrupted, since it can be resumed)                 |
| `agentsStates.completed`             | same agent row                                     | `completed`, include final `message` as detail/data                       |
| `agentsStates.errored` or `notFound` | same agent row                                     | `failed`, include message                                                 |
| `agentsStates.shutdown`              | same agent row                                     | `stopped`                                                                 |
| `pendingInit` or `running`           | same agent row                                     | `inProgress`                                                              |

Store the full protocol object under `payload.data`, but keep prompt/final
message previews bounded using the work-log's existing truncation. Do not expose
reasoning or invent child output. For multiple receivers (notably `wait`), emit
one agent-state update per map entry so independently finishing agents do not
collapse together. Map entries are unordered, so sort by child thread ID before
emitting activities for deterministic persistence and tests.

The first implementation should not reroute arbitrary child messages/tools.
Laplus has one `ConversationState` and one active root turn; folding child
`turn/started` or `turn/completed` into that state could incorrectly settle the
parent. If nested child work is later desired, first add the T3-style
`receiverThreadId -> parent turn` routing table and explicitly suppress child
turn/thread boundaries.

## Edge cases and focused verification

- Spawn-start has no child ID; show only the operation row until spawn-end or
  `subAgentActivity.started` supplies one.
- A completed spawn operation with agent state `running` must leave the separate
  agent row running.
- A failed call, missing receiver, unknown agent, and errored agent must use the
  failure tone without losing the provider message.
- `interrupted` is resumable according to the official `AgentStatus` docs; label
  it explicitly rather than calling it completed.
- Multiple children in one `wait` need separate stable keys and deterministic
  order.
- Duplicate `item/started`/`item/completed` notifications should collapse by
  call/agent key rather than create rows.
- Unknown future collaboration tools, statuses, activity kinds, and state kinds
  should increment drift but not break the turn.
- A child turn completion must not clear or settle laplus's active root turn.

Focused tests should be:

1. `codex_protocol.rs` unit fixtures for both item types, every status mapping,
   spawn-start without receivers, multiple agent states, and unknown enums.
2. `socket_codex_turn.rs` replay proving a running row is published immediately,
   spawn-operation completion does not complete the agent row, a later wait
   snapshot completes/fails it, and the root turn settles only on its own ID.
3. `apps/web/src/session-logic.test.ts` proving `tool.updated/inProgress` is
   visible and collapses with the same agent's terminal update. Much of this
   generic lifecycle behavior is already covered; add a subagent-shaped case.
4. Drive one real Codex spawn through `server/tools/ui-driver/`, per the repo's
   user-visible-change rule, and verify the live label/status then its terminal
   state. Preserve the NDJSON as a new fixture because the current synthetic
   drift fixture contains only item IDs and cannot prove real field presence or
   ordering.

## Recommendation

Implement the two-row server mapping now. It is small, uses protocol fields
Codex officially commits to, and fits the work log the UI already renders. Call
the result “running and observed completion,” not a complete child-agent
transcript. Treat receiver routing and nested child activity as a separate
feature because it changes turn correlation and settlement correctness.
