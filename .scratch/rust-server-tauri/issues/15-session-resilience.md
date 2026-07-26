# 15 — Session resilience: errors, process death, and drift counters

**What to build:** Things go wrong without taking the app down. An agent error is
reported in the conversation where the developer can see it and retry, not in a
crash. If the agent subprocess dies unexpectedly, the window survives and says so.
An unrecognised event from a newer CLI increments a counter instead of killing the
session.

The drift counters are the project's early-warning system for its most externally
volatile dependency: when the CLI's format moves, that should be learned from a
number, not from a bug report.

Long-session behaviour belongs here too — context compaction must not lose the
visible transcript.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-agent

**Groundwork already landed.** The turn lifecycle now lives in `crate::settling`
(ADR-0001): session status and turn state are typed, and settling is one rule
mirrored from upstream's two copies rather than five hand-written matches. Two
things follow for this ticket:

- A turn caught by a session going `stopped` settles as `interrupted`, matching
  upstream. That path is currently unreachable, because `turn.rs` reports an
  unfinished turn as `error` and keeps `stopped` for when none was running.
- **That encoder choice is this ticket's to make.** `error` is what carries
  `last_error` — the only place the developer is told the agent went away
  mid-turn — so reporting it as `stopped` instead would settle the turn the same
  way but lose the sentence. Decide it here rather than inheriting it.

- [ ] An error reported by the agent appears in the conversation and the session
      stays usable
- [ ] A turn can be retried after an error
- [ ] Unexpected death of the agent subprocess is reported in the UI and does not
      crash the server or the window
- [ ] A session whose subprocess died can be restarted without losing the
      transcript
- [ ] An unrecognised event type increments a drift counter and the session
      continues
- [ ] A malformed line increments a parse-error counter and the session continues
- [ ] Drift and parse-error counts are exposed where they can actually be seen
- [ ] Context compaction during a long session leaves the visible transcript intact
- [ ] Rate-limit notifications from the agent are surfaced rather than silently
      swallowed
- [ ] Scripted fake-agent captures cover error results, abrupt exit mid-stream,
      unknown event types, and malformed lines
- [ ] Tests drive each failure mode through the socket boundary
