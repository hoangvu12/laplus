# 04 — Recover unsent OpenCode queue entries

**What to build:** Preserve queued OpenCode messages when work cannot or must
not continue. Failed delivery, failed reconciliation, full session shutdown,
and application restart leave the text visible as not sent and provide an
explicit Retry path. Restart and full shutdown never submit the work
automatically.

**Blocked by:** 03 — Preserve the queued turn through Interrupt.

**Status:** ready-for-human

- [x] A queued-turn delivery failure retains every affected user message and exposes an actionable Retry state.
- [x] An unrecoverable interrupt reconciliation failure settles visible working state and retains queued text for Retry.
- [x] Full session shutdown retains queued messages without opening another session or sending another provider prompt.
- [x] After application or server restart, stored but unsent messages remain visible in an authoritative snapshot.
- [x] Restarted unsent messages do not submit until the developer explicitly retries them.
- [x] Retry submits the preserved content once with its original order, settings, attachments, and metadata.
- [x] Navigation never removes a failed or unsent queued message.
- [x] Focused socket-boundary tests cover delivery failure, reconciliation failure, session shutdown, restart, and Retry.
- [ ] The failure and restart states are verified in a rebuilt running application, and all verification processes are stopped afterward.

## Verification

2026-08-15: scripted socket tests passed for failed queued delivery, failed
interrupt reconciliation, full shutdown, authoritative restart snapshots,
no automatic restart submission, and explicit one-shot Retry. Client reducer
and timeline tests passed, including the visible Not sent/Retry state. The
production web bundle rebuilt and booted in an isolated browser profile; exact
interactive failure/restart acceptance remains for a human.
