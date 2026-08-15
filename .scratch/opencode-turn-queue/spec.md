Status: ready-for-human

# Reliable OpenCode context limits and queued turns

## Problem Statement

OpenCode conversations have two visible reliability defects.

First, the context meter often shows only the tokens used. It does not show the
selected model's context limit, even though OpenCode supplies that limit in its
provider catalogue. The shared meter already displays the correct information
for Claude and Codex when the server supplies it.

Second, a message sent while OpenCode is working can be treated as a steer of
the active turn. This becomes especially harmful after an interrupt. The new
message can race with settlement, remain on “Working” indefinitely, and vanish
when the developer leaves and reopens the conversation. User text must not
depend on a temporary client projection for durability.

## Solution

Make the OpenCode driver supply the selected model's context limit from the
authoritative provider catalogue. Retry catalogue loading when OpenCode is
healthy before the catalogue is ready. If the limit remains unavailable, keep
the existing used-token-only fallback. Do not change the context-meter UI.

Make OpenCode use Laplus queued-turn behavior. Messages sent while a turn is
running or settling are stored immediately as separate user messages. They do
not steer the active turn. When the active turn settles, all messages waiting
at that boundary start one new turn in their original order. The new turn uses
the settings captured when the queued messages were sent.

An interrupt stops only the active turn. It preserves the partial assistant
reply and all queued messages, then starts the queued turn after settlement. A
full session shutdown does not start queued work. It keeps the messages in a
retryable, not-sent state. Delivery and reconciliation failures follow the same
rule. Messages remain visible after navigation and restart.

## User Stories

1. As an OpenCode user, I want the context meter to show the model limit, so that I can judge how much context remains.
2. As an OpenCode user, I want the meter to match Claude and Codex, so that provider choice does not change the meter's meaning.
3. As an OpenCode user, I want the meter to use OpenCode's reported model limit, so that Laplus does not guess a stale value.
4. As an OpenCode user, I want token use to remain visible when the model limit cannot be loaded, so that a catalogue failure does not remove useful information.
5. As an OpenCode user, I want Laplus to retry a catalogue that is still starting, so that a normal cold start does not permanently remove the limit.
6. As a developer, I want a message sent while OpenCode is working to wait for the current turn, so that it does not change work already in progress.
7. As a developer, I want each queued message to remain a separate transcript entry, so that the transcript matches what I wrote.
8. As a developer, I want several waiting messages to start one next turn, so that the agent can answer them together without unnecessary runs.
9. As a developer, I want queued messages to retain their order, so that the agent receives my instructions in the order I sent them.
10. As a developer, I want a queued turn to include the completed reply before it, so that the agent has the full conversation history.
11. As a developer, I want to interrupt A and then send B, so that B starts after A stops and the agent understands both messages.
12. As a developer, I want the partial assistant reply from an interrupted turn to remain visible, so that I can see what happened before the interruption.
13. As a developer, I want the queued turn to include that partial reply in its history, so that the agent understands the exact conversation state.
14. As a developer, I want an interrupt to preserve B and C, so that stopping A never discards later instructions.
15. As a developer, I want B and C to survive navigation to another conversation and back, so that navigation cannot erase accepted text.
16. As a developer, I want queued text to survive an application restart, so that a shutdown cannot erase what I wrote.
17. As a developer, I want restarted queued text to require Retry, so that Laplus does not perform unexpected work after launch.
18. As a developer, I want failed queued delivery to keep my message and offer Retry, so that a provider failure does not destroy input.
19. As a developer, I want failed interrupt reconciliation to stop showing endless work, so that the conversation reaches an actionable state.
20. As a developer, I want queued messages to use the model and mode selected when I sent them, so that waiting does not change their meaning.
21. As a developer, I want queued attachments and prompt metadata preserved, so that the later turn receives the complete request.
22. As a developer, I want full session shutdown to retain queued messages without sending them, so that stopping the provider does not start more work.
23. As a Claude or Codex user, I want this fix not to alter my provider's behavior, so that an OpenCode repair does not create unrelated regressions.

## Implementation Decisions

- OpenCode follows the same queued-follow-up semantics already used by Claude and Codex. This supersedes the earlier decision to expose OpenCode steering through ordinary developer messages.
- A developer message received while an OpenCode turn is running or settling creates queued work. It never retains the active turn identifier and never enters the stopping turn as a steer.
- One queued turn can contain one or more separately stored developer messages. Messages keep their original order and transcript identity.
- The queue boundary is the settlement of the active turn. All messages waiting at that boundary start one next turn.
- An interrupt targets only the active turn. A queued turn remains pending and starts after the interrupted turn settles.
- The interrupted assistant response remains part of the transcript and the provider history, including partial output.
- A full session shutdown retains queued messages but does not automatically open another session or submit them.
- Queued messages and their effective provider selection, model, mode, attachments, and prompt metadata are stored before the dispatch command reports success.
- Navigation reconstructs queued messages from authoritative server state. It does not rely on client-only optimistic state.
- After process or application restart, work that was stored but not submitted is presented as not sent and requires an explicit retry.
- A delivery failure or unrecoverable settlement failure retains the affected messages in the same retryable state.
- Interrupt recovery is bounded. If the expected idle event does not arrive, Laplus reconciles with OpenCode's session state. If reconciliation fails, the active work stops appearing as indefinitely running and queued text becomes retryable.
- This behavior change applies to OpenCode only. Shared behavior may be reused, but Claude and Codex semantics must not change.
- The context-meter component and its presentation remain unchanged.
- The context limit comes from the selected OpenCode model's authoritative catalogue entry.
- The adapter reads model identity from the real assistant-event shape and also tolerates the alternate nested shape where required for compatibility.
- An owned OpenCode server can report healthy before its provider catalogue is populated. Laplus waits and retries catalogue loading for a bounded period.
- External OpenCode servers receive equivalent context-limit and queue behavior. Laplus does not take ownership of their lifetime.
- Catalogue failure is non-fatal. When the selected model's limit remains unknown, Laplus emits token use without a maximum and the current meter fallback remains visible.
- Existing uncommitted work is user-owned. Implementation must preserve it and build on it without replacing unrelated changes.

## Testing Decisions

- Tests verify behavior through public interfaces. They do not assert private queue fields, internal channels, or helper call order.
- The primary automated seam is the existing WebSocket orchestration boundary backed by a scripted OpenCode peer. This is the highest existing seam that observes both provider traffic and the state delivered to the UI.
- The scripted peer returns a real-shaped provider catalogue and assistant usage event. The socket output must contain a context-window activity with the catalogue's literal context limit.
- The scripted peer acknowledges an abort while withholding the idle event. A message submitted in that interval must appear as queued durable work and must not be sent as a steer.
- A fresh thread snapshot must contain queued user messages before the peer releases settlement. This models navigation away and back.
- After the peer releases settlement, the next provider prompt must contain all queued messages in their original order and use one new turn identity.
- The regression test covers A, Interrupt, B because that is the reported disappearing-message and endless-working sequence.
- A second sequence covers A, B, C without Interrupt. B and C remain separate transcript messages and start one next turn after A completes.
- Interrupt coverage proves that queued work survives and the partial reply remains interrupted history.
- Full session shutdown coverage proves that queued messages remain visible and no later provider prompt starts automatically.
- Failure coverage proves that an unreconciled interrupt or failed queued delivery ends indefinite working state and leaves retryable text.
- Restart coverage proves that stored but unsent text remains visible and does not submit automatically.
- Provider-selection coverage proves that a queued request uses the settings captured at submission time.
- Existing context-window derivation and meter tests remain unchanged unless a regression shows that their public behavior is wrong.
- Existing OpenCode socket tests and the established queued-turn tests for other drivers are prior art. Reuse their vocabulary and external assertions.
- Focused Rust and TypeScript checks cover only affected scopes. The full workspace suite remains CI's responsibility.
- Final acceptance rebuilds the UI and drives a running Laplus conversation. It checks the context meter, A/Interrupt/B, navigation away and back, queued execution, and the absence of an endless Working state. All started servers and watchers are stopped after verification.

## Out of Scope

- Changes to the visual design, labels, formatting, percentage ring, or layout of the context meter.
- A maintained fallback table of guessed model context limits.
- Changes to Claude or Codex queue semantics.
- A general-purpose user interface for editing, reordering, or cancelling queued messages.
- Automatic submission of unsent work after application restart or full session shutdown.
- Changes to OpenCode authentication, provider discovery outside the context-limit path, model selection, or server ownership.
- Hiding interrupted partial assistant output.
- Exposing ordinary OpenCode steering through the composer.

## Further Notes

- Direct verification used OpenCode 1.18.18. Its provider endpoint returned model limits under each model's context-limit field.
- The current UI already renders used tokens, maximum tokens, and percentage when the server supplies a maximum.
- The observed interrupt defect is a race between immediate client-visible interruption and later provider settlement. During that gap, the current OpenCode path can attach a new message to the stopped active turn.
- The provider's current API distinguishes queued delivery from steering. Laplus still owns the durable transcript and lifecycle guarantees described here.
- ADR-0045 records queued OpenCode messages and supersedes ADR-0038.
