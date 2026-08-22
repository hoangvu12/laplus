Status: ready-for-agent

# 05 — Conversation-owned servers reap when idle between turns

**What to build:** A conversation that has been sitting idle between turns —
no active work, nothing pending — gives up its `opencode serve` process after
a bounded idle window, and its next message transparently resumes by durable
session id against a freshly spawned server. Memory tracks what the developer
is working on rather than every conversation ever opened. Pending approvals or
questions hold the server alive so an answer still reaches the agent that
asked. External servers are never touched by this machinery (operator-owned,
per ADR-0036).

This mirrors the shared text-generation server's pattern (ADR-0043): the reap
decision is a pure function of idle time and session conditions, and spawn
passes opencode's own idle-instance disposal knob so LSP/watcher instances
inside the process also shrink. Every reap and resume logs instance and
session ids. Landing this adds one ADR for conversation-owned idle reaping.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [x] Pure-function tests: reaps past the window; refuses with an active turn;
      refuses with a pending approval or question; refuses in external mode.
- [x] Integration: an idle conversation's server is reaped within the window;
      the next prompt resumes by session id and the transcript continues
      seamlessly against the restarted peer.
- [x] A slow turn is never killed underneath the developer (active turn blocks
      reaping at any age).
- [x] An unanswered permission/question holds the server past the window;
      answering still reaches the agent.
- [x] Spawn passes the disposal knob with a value shorter than laplus's own
      conversation-idle window; external-mode spawn is unchanged.
- [x] Reap and resume each log once with instance and session identifiers.
- [ ] Focused suites stay green on both platforms per the repo's test
      discipline (no wall-clock assertions; decisions asserted, timeouts only
      catch hangs).

## Comments

2026-08-22 review: implementation and functional acceptance coverage are in
place. The focused Windows suite passes (8 passed, one helper ignored),
including idle reap/resume, active-turn retention, and unanswered-permission
retention. Leave the cross-platform criterion unchecked until Linux CI evidence
is recorded; then this ticket can move to `done`.

2026-08-22 completion audit: `cargo check -p laplus-server` and the focused
Windows binary pass. The full serialized Rust run completed every unit and
integration binary without a failure but exceeded the runner window after
entering doc-tests; the isolated laplus-server doc-test invocation passes with
zero tests. Review found no incorrect ticket-05 behaviour. It did identify two
coverage follow-ups: the scripted peer drives the outstanding-permission path
but not an outstanding question, and the public harness has no supported seam
for asserting reap/resume stderr cardinality. Neither substitutes for the
still-required Linux CI result.
