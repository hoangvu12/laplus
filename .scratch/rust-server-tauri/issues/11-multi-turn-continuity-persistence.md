# 11 — Multi-turn continuity and transcript persistence

**What to build:** A developer sends a follow-up message and the agent remembers
what was already discussed. They close the app, reopen it the next day, and their
conversation is still there — able to be read, and able to be continued.

Continuity across turns uses the agent CLI's own session identity and resume
capability rather than replaying history into each prompt. Transcripts are stored
alongside the project registry.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-agent

- [ ] A follow-up message in the same conversation retains prior context
- [ ] Several turns can be exchanged in sequence without the session degrading
- [ ] A conversation and its full transcript survive an app restart
- [ ] A restored conversation can be continued, not just read
- [ ] Resuming a session whose underlying agent session is no longer available
      fails with an explanation and leaves the transcript readable
- [ ] Transcript writes do not block or stutter the live stream
- [ ] A very long transcript loads without stalling the UI
- [ ] Tests cover multi-turn exchange and restart-then-continue through the socket
      boundary
