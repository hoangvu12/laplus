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
