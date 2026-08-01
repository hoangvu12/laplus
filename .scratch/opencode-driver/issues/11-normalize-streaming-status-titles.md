# 11 — Normalize streaming, status and titles

**What to build:** Current and pinned OpenCode event streams render one correct
conversation even when message roles and parts arrive out of order, text arrives
as both deltas and cumulative updates, or both idle event forms are emitted.
Reasoning, retries, failures and upstream titles reach their shared Laplus
surfaces.

**Blocked by:** 09 — Run the first owned OpenCode text turn.

**Status:** ready-for-agent

- [ ] Assistant and reasoning parts render correctly when role metadata arrives
      before or after the part
- [ ] True deltas stream immediately and the final cumulative update emits only
      unseen text
- [ ] An older cumulative update never shortens or duplicates rendered content
- [ ] Status-idle and standalone idle settle the same active turn idempotently
- [ ] Busy, retry and structured error events publish the expected session,
      warning and failed-turn behavior
- [ ] Every non-empty upstream title update becomes the thread title
- [ ] Unknown new events remain observable and non-fatal
- [ ] Scripted-peer tests exercise the captured 1.18.10 ordering and pinned T3
      variants through the socket boundary
