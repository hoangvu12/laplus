# ADR-0059 — A stop that cannot be proved still ends the turn

Date: 2026-09-01
Status: Accepted
Supersedes, in part: [ADR-0058](0058-opencode-stop-is-verified-and-owned-runaways-are-reaped.md)

## Context

ADR-0058 gave the stop a ladder — abort, then a proof of quiescence read from
the session's own messages, then a reap of a server laplus launched — and one
sentence that turns out to be the ladder's missing bottom: _"A failed sample
leaves the conversation loop alive and visibly reports that verification is
continuing."_ ADR-0056 says the same thing from its own end: _"Inspection
failures ... remain supervised instead of ending the conversation."_

Against a session history that answers nothing but errors, that is supervision
with no terminal condition. The proof above it needs an unbroken quiet interval
across snapshots that never arrive; the escalation above it needs _changed_
output, which snapshots that never arrive cannot show either. So reconciliation
returns pending for ever, the stopped turn is immortal, and the conversation is
alive and useless — worse than either outcome ADR-0058 chose between. It is
observable rather than theoretical: `failed_interrupt_reconciliation_is_
reported_once_and_later_turns_still_run` sat for sixty seconds against a peer
whose `session.messages` answers 500, with the server saying why on every
sample, and ticket 04 of `.scratch/opencode-correctness/` carries the trail.

### Ending the turn costs something the rungs above it never had to pay

Every settlement ADR-0058 describes follows either a proof that the provider
went quiet or a kill that made it go quiet. This one follows neither, and two
consequences follow from that.

**Nothing is killed.** An unreadable history is not the proof of a runaway the
reap is built on, and an external server is never ours to kill (ADR-0036). So
the event subscription stays open on a session whose provider may still be
writing.

**A queued prompt is released.** The session loop takes the next prompt exactly
when `driving.turn` is empty, so settling is what lets a follow-up start — which
is precisely what story 11 of the spec asked to be protected from: _"I want my
message held until the stopped turn has provably ended."_ Here it has not
provably ended; supervision of it has.

Between them those two turn the rung's honesty into a question about the _next_
turn. And before this decision the answer was bad: settlement clears the
driver's per-part transcript state, so the runaway's next part — on the message
of the turn that was stopped — looks like a part nobody has drawn, and the turn
in flight when it arrives is the turn it is drawn into. That is story 10's
defect (_"stale output cannot keep flowing into the conversation afterwards"_)
and the upstream fake-idle family the whole spec was written against, reached
through a different door than the one the quiescence proof guards.

## Decision

**A stop that can never be proved ends the turn anyway, and a turn that has
ended cannot be spoken into.** Two rules, and the second is what makes the first
honest.

- **An unbroken run of unreadable history snapshots lasting the same bounded
  window that escalates a proven runaway abandons verification.** The turn
  settles as interrupted, the failure already reported once stands as the only
  report, and the conversation stays open for the next prompt. A readable
  snapshot restarts the window, so one transient error only delays the proof
  rather than ending a turn. Nothing is killed in either ownership mode. Ending
  the whole conversation remains the exclusive job of explicit stop-session.

- **Every settlement retires the provider messages its turn heard from, and a
  retired message can never speak into this conversation again.** Later events
  naming one are dropped before they reach the transcript — parts, deltas, tool
  rows, token counts alike. A message first heard from while no turn is driving
  is retired where it is heard, because nothing arriving before the developer
  has asked for the next turn can belong to it. The set is bounded at a fixed
  number of ids with the oldest evicted, so a conversation of any length pays a
  constant price for a question that is only ever about its last few turns.

The diagnostic for the first rung says `phase abandoned`, not `phase settled`.
What is claimed is that supervision ended, not that the provider went quiet.

The second rule is deliberately a property of **settlement** and not of
abandonment. A late part arriving after an ordinary interrupt, after an
escalation, or after an external runaway was reported has exactly the same
shape; a rule that only covered the rung that provoked it would leave the same
leak behind three other doors.

## Consequences

- **A queued prompt is released into a conversation whose provider may not have
  stopped.** That is the real cost of the rung, and it is the thing ADR-0058's
  first draft of this did not disclose. What protects story 11 now is the second
  rule rather than the turn staying open: the developer's message runs as its
  own turn, and the runaway cannot get a word into it.

- **One case stays open, and killing is the only answer to it.** A provider that
  mints a _wholly new_ assistant message after the settlement and after the next
  prompt has gone out is indistinguishable on the wire from the next turn
  answering. The retirement rule closes every late part of a message the
  conversation has heard from or could have; it cannot close that one. Killing
  is available only for a server laplus launched and only against ADR-0058's
  proof, so this is a real limit rather than a deferral.

- **A permanently unreadable history now ends a turn** where it used to end
  nothing. That is a behaviour change for a conversation pointed at a server
  that is up enough to accept an abort and broken enough never to answer for its
  own messages — and for that conversation the alternative was a turn that never
  settles.

- **ADR-0056's supervision sentence is narrowed, not withdrawn.** An inspection
  failure still may not end the _conversation_. What it may now do is end the
  _turn_. That amendment stays in place on ADR-0056, in the shape ADR-0007 set
  for narrowing one sentence, and points here.

- **Why this is a new ADR rather than a third in-place amendment.** ADR-0007's
  amendment is a clarification of one sentence of Consequences and reads as one.
  This adds a rung, changes when a turn settles and when a queued prompt is
  released, and adds a rule about what a settled turn may still receive —
  a policy change, which in this repository is ADR-0031's shape: a new decision
  that supersedes, in part, the one it grew out of. Everything else in ADR-0058
  survives unchanged, including the whole of the proof and the owned reap.
