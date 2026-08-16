# 01 — Idempotent OpenCode reconciliation seam

**What to build:** Add the narrow read/reconcile seam that can compare an active
Laplus turn with OpenCode session status and message history without resending a
prompt or creating a session.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] Add only the required status/message routes and narrow protocol types.
- [ ] Reconcile assistant text, reasoning, tools, requests, and terminal state
      against already-folded turn state.
- [ ] Make replay and settlement idempotent with stable provider identities.
- [ ] Classify busy, idle, provider-error, missing, auth, transport, and protocol
      results without weakening ADR-0041's fail-closed continuation behavior.
- [ ] Prove EOF-before-idle recovery and duplicate replay at the HTTP/socket seam.
