# ADR-0004 — A turn cut short by a dead agent is reported as an error

Date: 2026-07-27
Status: Accepted

## Context

ADR-0001 typed the two lifecycle vocabularies and put the decoder — how a
running turn settles when a session status arrives — in `crate::settling`,
mirrored from upstream. It left one question open by name: when the agent
process goes away in the middle of a turn, which status does the **encoder**
publish?

Two are defensible and they settle the turn differently:

| Status | Settles the turn as | Carries `lastError` |
| --- | --- | --- |
| `stopped` | `interrupted` | no |
| `error` | `error` | yes |

`stopped` reads as the more literal description — the process stopped — and
ADR-0001 made it reachable by fixing the decoder to settle it as `interrupted`,
matching upstream's two copies. Ticket 15 owns the choice.

## Decision

**`error`.** A turn the agent died in the middle of is reported as a session
error carrying a sentence, not as a session that stopped.

Two reasons, and the second is the load-bearing one:

- **Nobody asked.** `interrupted` is the developer's word in this contract — it
  is what the stop button produces, and it is what a cancelled permission
  produces. A turn ended by a process crashing is not something the developer
  did, and reporting it in the vocabulary of things they did would make the work
  log's account of the conversation untrue.
- **`error` is the only status that can carry the explanation.** `lastError` is
  where the client renders why a session is unusable, and a session reported as
  `stopped` has nowhere to put a sentence. The sentence matters here more than
  anywhere else in the lifecycle, because a dead process leaves no NDJSON at all
  — the only account of why is the CLI's last line on stderr, which
  `Agent::stop` returns and `turn::died_mid_turn` quotes.

The same choice applies to a `--resume` the CLI refuses, which reaches the same
code path from ticket 11 and for the same reason.

## Consequences

- `SessionStatus::Stopped` remains reachable and correct: it is what a session
  that ended with **no turn in flight** publishes — an ordinary shutdown. Its
  decoder arm is still the mirror ADR-0001 made it; nothing in this decision
  touches `crate::settling`.
- A turn cut short comes back from a restart as `error` rather than
  `interrupted`. `interrupted` stays the hard-kill case, where the driver never
  ran at all and `Thread::restored` coerces a stored `running` turn.
- The driver now also **settles the assistant message** it was streaming before
  it publishes any of this. A delta owes the database nothing, so a reply cut
  short otherwise had nothing on disk and stayed `streaming` forever in memory;
  settling it is what makes "reported rather than lost" true of the reply as well
  as of the session. It is settled with an *empty* buffered message, which is the
  case where the accumulation stands — so no reconciliation is recorded for a
  message that never arrived.
