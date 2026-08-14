# 04 — Recover unsent OpenCode queue entries

**What to build:** Preserve queued OpenCode messages when work cannot or must
not continue. Failed delivery, failed reconciliation, full session shutdown,
and application restart leave the text visible as not sent and provide an
explicit Retry path. Restart and full shutdown never submit the work
automatically.

**Blocked by:** 03 — Preserve the queued turn through Interrupt.

**Status:** ready-for-agent

- [ ] A queued-turn delivery failure retains every affected user message and exposes an actionable Retry state.
- [ ] An unrecoverable interrupt reconciliation failure settles visible working state and retains queued text for Retry.
- [ ] Full session shutdown retains queued messages without opening another session or sending another provider prompt.
- [ ] After application or server restart, stored but unsent messages remain visible in an authoritative snapshot.
- [ ] Restarted unsent messages do not submit until the developer explicitly retries them.
- [ ] Retry submits the preserved content once with its original order, settings, attachments, and metadata.
- [ ] Navigation never removes a failed or unsent queued message.
- [ ] Focused socket-boundary tests cover delivery failure, reconciliation failure, session shutdown, restart, and Retry.
- [ ] The failure and restart states are verified in a rebuilt running application, and all verification processes are stopped afterward.
