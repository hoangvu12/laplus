Status: done

# 03 — Stop is proven, not believed

**What to build:** Pressing stop becomes an outcome laplus verifies rather
than a request it sends. After the abort request lands, quiescence is proven
from the session's messages over a bounded verification window; only then does
the turn settle as interrupted. A distinct "stopping" phase shows the click
landed while verification runs. A provider that fakes idle while still
streaming can no longer leak output past settlement into whatever the
developer types next.

Status endpoints remain hints that may shorten the wait — never the sole
evidence. This is the defence against the upstream fake-idle class of defects
(#29894, #26635, #3815).

**Blocked by:** 02 — Part-keyed merge for interrupt & stream-loss reconcile
(same seam: interrupt reconciliation is rewritten by both).

**Status:** done

- [x] Against the scripted peer: abort answered then output continuing — the
      turn does not settle until output actually stops; post-abort parts reach
      no later turn's transcript.
- [x] Abort answered and genuinely quiet settles as interrupted within the
      verification window, without waiting for the window to expire.
- [x] A "stopping" activity is published between abort-accepted and
      settled-once-proven; duplicate stops and duplicated idles still settle
      exactly once.
- [x] Spurious idle before any busy is ignored as today; the ignore-idle-until-
      busy guard survives this change.
- [x] Queued prompts stay queued until settlement completes (ADR-0045
      semantics intact).
- [x] Diagnostics name instance id, session id, phase (abort sent / verifying /
      settled) and last observed message count — never prompt or answer text.
- [x] Policy decisions (window expiry vs early proof, duplicate races) are
      covered on the fake-driver session harness; wire behaviour on the
      scripted peer.

## Comments

2026-08-22 review of `opencode-correctness`: not complete. The current branch
publishes the existing interruption changes before quiescence is proven, and
two equal point-in-time message snapshots can falsely settle during an ordinary
pause in provider output. Implement a real stopping state and a bounded quiet
window that resets whenever assistant output changes. The required fake-idle,
continued-output, queued-prompt, diagnostics, and policy tests are still absent.

2026-08-22 implementation: completed. OpenCode now keeps the turn running with
a `turn.stopping` activity after abort acceptance, defers the interrupt receipt
and record until proof, and samples authoritative assistant history across a
four-second quiet window that restarts on every changed snapshot. Status and
idle events cannot prove settlement. Diagnostics carry the configured instance
id, provider session id, phase, and message count without content. Focused
verification passed: the stop-policy unit test, both interrupt integration
tests, the pause-then-resume scripted peer, the queued-turn/late-idle regression,
and `cargo check -p laplus-server`.
