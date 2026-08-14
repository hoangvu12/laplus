# 01 — Restore OpenCode context limits

**What to build:** Make OpenCode conversations supply the selected model's
authoritative context limit to the existing context meter. Use the real provider
catalogue and assistant-event shapes for owned and external endpoints. Treat a
catalogue that is still starting as retryable, while preserving the current
used-token fallback when no limit can be obtained.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [x] A real-shaped OpenCode provider catalogue and assistant usage event produce a context-window activity with the selected model's literal context limit.
- [x] The existing meter shows the same used, maximum, and percentage presentation used for Claude and Codex without a UI redesign.
- [x] Owned OpenCode startup waits and retries when health is ready before the provider catalogue is populated.
- [x] External OpenCode endpoints resolve the same model limit without changing ownership or lifetime rules.
- [x] A missing or persistently unavailable limit remains non-fatal and shows the existing used-token-only fallback.
- [x] Focused socket-boundary tests cover the real response shapes and the delayed-catalogue case.
- [x] The behavior is verified in a rebuilt running application, and all verification processes are stopped afterward.
- [x] Existing uncommitted work and unrelated changes remain intact.

## Verification

2026-08-14: focused owned, delayed-catalogue, and external socket turns passed;
the OpenCode usage-shape unit cases and the web context-meter presentation tests
passed; the web typecheck and production rebuild passed. The rebuilt bundle was
served by an isolated `laplus-server` and opened through the UI driver, then the
exact server PID was stopped. The full serial `laplus-server` suite passed with
`--no-fail-fast -- --test-threads=1`.
