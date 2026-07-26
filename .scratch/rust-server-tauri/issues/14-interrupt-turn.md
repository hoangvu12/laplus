# 14 — Interrupt a turn

**What to build:** The developer can stop the agent mid-turn when it is heading the
wrong way, and immediately send a correction. Interruption leaves the conversation
in a usable state — partial output is kept and clearly marked as interrupted,
rather than vanishing or masquerading as a complete answer.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-agent

- [ ] An in-flight turn can be interrupted from the UI
- [ ] Streaming stops promptly rather than running to completion in the background
- [ ] Output produced before the interrupt is retained in the transcript and marked
      as interrupted
- [ ] A new message can be sent immediately afterwards in the same conversation
- [ ] Interrupting during a tool call leaves no orphaned child process
- [ ] Interrupting when no turn is in flight is a no-op rather than an error
- [ ] The agent subprocess survives the interrupt and is reused for the next turn
- [ ] Tests drive interrupt-then-continue through the socket boundary
