# 08 — Interrupting a Codex turn

**What to build:** A developer who got a prompt wrong interrupts mid-sentence and
the turn stops. The partial reply stays on screen exactly as it was read, because
what they read is what the transcript should record. They then send a correction
immediately, and it continues the same conversation rather than starting a new
one.

**An interrupted turn settles on the interrupt's own response, because nothing
else marks it.** This is the capture that matters most, and three things in it
contradict what the shape of the protocol suggests:

- **In-flight output continues to arrive after the interrupt is sent.** The
  recording has 106 deltas landing between the request and the acknowledgement.
  The acknowledgement is last.
- **There is no `item/completed` for the streaming message.** The partial text
  never gets an authoritative version.
- **There is no `turn/completed` and no idle.** The response to the interrupt is
  the only terminal signal there is.

**Reconciliation does not apply to an interrupted Codex turn.** `claude` hands
the partial message over whole, so the buffered message replaces the
accumulation; Codex hands over nothing, so **the accumulation is the final
text**. This is a documented divergence in behaviour between the two drivers, not
a bug in either, and it belongs in `server/CONTEXT.md` beside the
**Reconciliation** entry rather than being left for the next reader to rediscover
from a capture.

The interrupt leaves the child alive. That is what makes the correction the
developer types next a _correction_ rather than the first message of a
conversation that has forgotten what it was about — the same distinction the
`claude` driver already draws between an interrupt and a session stop.

`captures/04-interrupt.jsonl` becomes a fixture with an expected fold and is
replayed through the socket, with a stop before the acknowledgement for the
mirror-image reason the `claude` captures need one: that line is the agent
answering something the server asked, and the request travelled on our side of
the wire so it is not in the recording.

**Blocked by:** 04.

**Status:** ready-for-agent

- [ ] Interrupting a running Codex turn stops it.
- [ ] The turn settles on the interrupt's response, with no completion and no
      idle required.
- [ ] Deltas arriving after the interrupt was sent and before it was acknowledged
      are kept, not discarded.
- [ ] The partial reply is recorded as it was on screen; no reconciliation is
      attempted against a buffered message that never arrives.
- [ ] The app-server child survives the interrupt.
- [ ] A message sent immediately afterwards continues the same conversation.
- [ ] The divergence from `claude`'s reconciliation is written into
      `server/CONTEXT.md`.
- [ ] `04-interrupt` is committed as a fixture with an expected fold, and is
      replayed with a stop before the acknowledgement.
