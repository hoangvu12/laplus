# 11 — A runtime mode change does not reach a live agent

**What to build:** a decision, and then whatever it implies — a runtime mode the
developer changes mid-conversation currently does not reach the `claude` process
already serving it, so the agent goes on working under the mode the conversation
started with.

Found while building ticket 02, which is why it is numbered after the spec's ten
rather than inside them. It is **not** a regression that ticket introduced: the
per-turn override on `thread.turn.start` has had the same hole since the turn
machinery landed, and ticket 02 only made it reachable from a second control.

## What happens

One child serves a whole conversation — `socket_turn.rs`'s
`one_subprocess_serves_the_conversation_and_is_reaped_when_the_server_stops` is
the test that says so — and `--permission-mode` is passed once, when the process
is opened (`crate::turn::open`, via `crate::agent::permission_mode_for`). The
agent protocol has no control request that moves it afterwards; nothing in
`fixtures/claude-cli/` or the STEP 1 spike shows one.

So a developer who tightens `full-access` to `approval-required` sees the picker
move, sees it survive a restart, and sees the next turn _requested_ under the new
mode — and the agent keeps bypassing permissions until its process is replaced,
which today only happens at a restart or after a session error.

There is a second face of the same thing. The driver publishes
`session.runtimeMode` from the `Start` it captured at the **first** turn
(`crate::turn`, four sites off `start.runtime_mode`), while
`Shell::start_turn` publishes the `starting` session from the _thread_. So one
turn can announce two different modes: the new one as it starts and the old one
as it enters `running`. The UI renders that field beside the session state, so
the badge flips and flips back.

## The decision this needs first

Whether a runtime mode change should **replace the session**. It is not free:

- Killing and re-`--resume`ing costs a fresh `init`, and the CLI's warm context
  window with it.
- `approval-required` is expressed by passing _no_ flag and answering the
  permission callback, so the two directions are not symmetrical — loosening and
  tightening do not cost the same thing.
- Snooze's rule in the spec is "governs attention, never the agent". A mode is
  the opposite: it is entirely about the agent, so there is no precedent here to
  borrow.

The cheap alternative is to leave the process alone and stop claiming otherwise:
publish `session.runtimeMode` from the thread rather than from the captured
`Start`, so at least the two events for one turn agree, and say in the UI that a
mode takes effect for the next session. That is a smaller change and a worse
answer, and which of the two is wanted is a human call.

**Blocked by:** None — but it wants a decision before it wants code.

**Status:** needs-triage
