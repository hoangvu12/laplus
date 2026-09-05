# Codex finishes but laplus remains running

Status: ready-for-agent

Investigated 2026-09-05. This records the pre-fix findings and reproductions.
Both defects are now fixed; see [implementation and verification](../provider-process-leaks/implementation.md).
The decoder regression was renamed from `codex_automatic_turn_research.rs` to
`codex_lifecycle.rs` and expanded with identity and stale-event cases.

## Confirmed defect: the root becomes its own child

A `subAgentActivity` whose `agentThreadId` equals the root Codex thread is accepted as a child. `Children::acted` marks it working. A root `turn/completed` settles the root, but never concludes that fake child. `Threads::follow_delegation` then sets the session back to running because its child registry still contains work. The UI deliberately trusts this session status even when the latest turn has a completion timestamp (`apps/web/src/session-logic.ts:isLatestTurnSettled`). This is a backend state defect, not evidence of a stuck spinner alone.

Read-only metadata from the installed application's SQLite database confirmed a Codex conversation with `latest_turn.state = completed` and a working `thread_subagents` row whose child ID exactly matches `provider_resume_cursor.threadId`. No prompts, replies, credentials, or user file contents were copied. The live root was also executing automatic subsequent work, so this snapshot alone does not establish that it should have been idle at the moment sampled.

The synthetic socket regression isolates the defect without real model usage:

```powershell
cd server
cargo test -p laplus-server --test socket_codex_turn codex_root_activity_does_not_keep_a_completed_conversation_running --no-fail-fast
```

Observed before implementation: **FAILED**, 1.03 seconds. The fixture sends a root-targeted interaction and then a normal terminal event. The fresh socket snapshot has `latestTurn.state = completed` and `session.status = running`; the assertion reports `the root became its own permanently working child`. The server and fixture are stopped before the assertion.

Fix direction: enforce root identity at the boundary where collaboration targets become child state. Cover root and nested `subAgentActivity`, collaboration agent maps, and receiver IDs; a child can address its parent. Preserve genuine child work and its hierarchy. Clean up already-persisted self-child rows or exclude them from delegation activity when the authoritative provider root ID proves their identity. Do not globally stop genuine children on root completion.

## Confirmed protocol gap: automatic successor completion is discarded

`ConversationState::fold_notification` does not handle root `turn/started`. The driver learns its current turn ID only from a correlated `turn/start` response. After the first turn completes, an automatic successor announces another ID without a new client request; its completion is rejected by the old ID check. `Codex::next` has another active ID check, and `decide` currently does nothing for `TurnStarted`, so adding only a decoder case is insufficient to restore session state.

Read-only Codex rollout metadata showed successive `task_complete` / `task_started` boundaries milliseconds apart after one laplus turn. This is evidence that automatic successors occur in this installation. The exact app-server wire exchange was not captured during this investigation.

```powershell
cd server
cargo test -p laplus-server --test codex_automatic_turn_research --no-fail-fast
```

Observed before implementation: **FAILED**, 0.00 seconds: `the automatic turn's terminal event was discarded`. This public decoder test proves loss of the terminal event, not the complete UI lifecycle.

Fix direction: consume root `turn/started`, correlate asynchronous root successors, and represent their active work in `Driving`/thread state even when no composer request created a laplus turn. A socket regression must cover first completion, successor start, successor completion, and a queued user message; ensure a delayed terminal for the previous turn cannot settle a newer user turn. This deserves explicit lifecycle design, not merely assigning a new ID.

## Other mechanisms and limits

The normal handshake advertises empty capabilities and intentionally waits for `turn/completed`; root idle notifications do not settle. This came from the repository's July 31 capture against Codex 0.146.0, which observed different behavior with experimental capabilities. Missing terminal notifications remain a resilience gap, but this research did not capture such loss in the user's running instance. Do not declare completion from a final-looking assistant message or silence: tools, automatic turns, and child work can follow.

If recovery is added, use an authoritative read of the relevant provider turn and preserve failed/interrupted outcomes. Idle is useful as a reconciliation trigger, not sufficient evidence for successful completion in every race. Keep parent and child thread IDs separate. One currently running Codex conversation sampled during research had recent tool events and no terminal rollout event; it was correctly running.

## Primary documentation

[Official OpenAI Codex app-server documentation](https://learn.chatgpt.com/docs/app-server) documents root turn lifecycle notifications, terminal outcomes, thread runtime status updates, and reading persisted thread turns. It says to keep reading notifications and use terminal turn status to determine the outcome; item completion is a separate lifecycle. The page does not establish the old experimental-capability suppression behavior, so that claim is attributed only to the local recorded spike.

Local evidence and relevant implementation: `.scratch/codex-driver/spike-findings.md`, `server/crates/laplus-server/src/codex_protocol.rs`, `server/crates/laplus-server/src/codex.rs`, `server/crates/laplus-server/src/threads.rs:follow_delegation`, and `apps/web/src/session-logic.ts:isLatestTurnSettled`.

The exact intermittent user episode is not identified. The socket test reproduces one concrete way to produce the reported symptom, and the automatic-turn decoder test proves a second gap that can interact with it. Neither test uses live model credentials.
