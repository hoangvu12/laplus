# 02 — Supervise and reconnect the OpenCode event stream

**What to build:** Replace the one-shot stream lifetime with ADR-0056's visible,
cancellable recovery loop for owned and external OpenCode sessions.

**Blocked by:** 01 — Idempotent OpenCode reconciliation seam.

**Status:** ready-for-agent

- [ ] Publish reconnecting state on EOF/retryable transport failure.
- [ ] Reconcile before resubscribing and use capped exponential backoff/jitter
      while OpenCode remains busy.
- [ ] Add a conservative activity watchdog that reconciles rather than aborts.
- [ ] Preserve pending approval/question state and deduplicate replay.
- [ ] Keep Stop effective during reads and backoff; make stop/idle/error/server
      exit races settle once and reap owned resources.
- [ ] Emit bounded redacted lifecycle diagnostics.
- [ ] Cover busy reconnect, proxy flap, terminal errors, pending input, stop,
      process death, and duplicate completion with deterministic integration tests.
