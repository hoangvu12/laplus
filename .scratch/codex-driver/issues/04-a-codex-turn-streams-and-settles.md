# 04 — A Codex turn streams, and settles

**What to build:** The first conversation. A developer picks a Codex model in the
composer, sends a prompt, and reads the reply as it arrives rather than waiting
for a wall of text. The agent's reasoning is visible while it happens, so a stuck
turn looks different from a thinking one. The session status beside the
conversation matches what the agent is doing, and a turn that failed records the
failure and its reason so the developer can decide whether to retry.

Each Codex conversation gets its own `codex app-server` child, alive across
turns and reaped when the session ends. This is how laplus already drives
`claude`, so **session** keeps meaning "the agent process behind a thread" and
session stop, session status and session epoch need no reinterpretation.

**A turn settles on `turn/completed`, whose error decides between completed and
failed.** This is why ticket 03 sends empty capabilities: with
`experimentalApi` set, `turn/completed` is never emitted and a turn ends on a
status change carrying nothing about how it went — and `Ending` is exactly that
distinction. A status change to idle is handled as a **terminal fallback**, so
the capability can be turned on later without breaking the settle. That is a few
lines rather than a design, and it is what keeps the door open.

Upstream carries `turn/completed` handling that its own handshake prevents from
firing. Read them for how; verify against the capture for what.

The conversation runs in the project's folder, so relative paths in the
transcript mean what the developer thinks they mean. A Codex conversation and a
Claude conversation running at once are unaffected by each other — two agents are
genuinely two agents.

An access mode is sent on the thread from this ticket onward, but the picker does
not yet mean anything; ticket 07 is where it does.

`captures/01-plain-turn.jsonl` becomes a fixture with an expected fold, and is
replayed through the socket by a Codex stand-in. Every capture does both jobs.
The stand-in differs from the `claude` one in a single way: it correlates request
ids and answers requests rather than printing a stream.

**Blocked by:** 01, 03.

**Status:** ready-for-agent

- [ ] Selecting a Codex model and sending a prompt starts an app-server for that
      conversation and streams the reply as it arrives.
- [ ] The agent's reasoning is visible while the turn runs.
- [ ] The turn settles on `turn/completed`; its error decides completed versus
      failed, and a failure records the reason.
- [ ] A status change to idle settles the turn as a fallback when no completion
      arrives, so flipping the capability flag later does not break the settle.
- [ ] Session status tracks the agent through the turn and back to rest.
- [ ] The conversation runs in the project's folder.
- [ ] A Codex conversation and a Claude conversation run concurrently, and
      neither's events, statuses or settling reach the other.
- [ ] The app-server survives between turns and is reaped when the session ends.
- [ ] `01-plain-turn` is committed as a fixture with an expected fold, and the
      golden suite folds it through a fresh Codex state.
- [ ] The same capture is replayed through the socket by a Codex stand-in, with
      the assertions on what the UI receives.
