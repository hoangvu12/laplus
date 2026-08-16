# 03 — Recover unfinished OpenCode turns after restart

**What to build:** Detect persisted unfinished OpenCode turns at server startup
and route them through the same reconciliation supervisor before another prompt
can start.

**Blocked by:** 02 — Supervise and reconnect the OpenCode event stream.

**Status:** ready-for-agent

- [ ] Identify only unfinished OpenCode turns with valid provider-instance and
      resume-cursor identity.
- [ ] Recover completed output, reattach to busy work, or fail missing/terminal
      sessions clearly while preserving partial history.
- [ ] Never create a replacement session or resend the old prompt.
- [ ] Prevent startup recovery and a concurrent client action from owning the
      same turn twice.
- [ ] Add restart tests for completed, busy, missing, auth, stop, and eventual
      reconnect outcomes.
