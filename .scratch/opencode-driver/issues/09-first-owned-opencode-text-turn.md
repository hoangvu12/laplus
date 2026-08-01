# 09 — Run the first owned OpenCode text turn

**What to build:** A developer can select a local OpenCode instance, send a text
prompt, see the assistant response and a completed turn, then stop or close the
session without leaving an owned process behind. This is the first complete
conversation tracer bullet through the shared driver and socket boundaries.

**Blocked by:** 06 — Retire the agent-session-id concept; 08 — Discover and
configure OpenCode instances.

**Status:** done

- [x] Starting a thread launches one loopback-only `opencode serve` process with
      an empty injected configuration and waits for readiness
- [x] The driver creates a directory-bound OpenCode session and persists its v1
      provider resume cursor
- [x] A selected `provider/model` and text prompt reach the asynchronous prompt
      operation
- [x] Basic assistant content streams into the transcript and an idle signal
      completes the active turn once
- [x] Startup failure and timeout become visible session failures rather than a
      permanently starting session
- [x] Stop, thread closure and server shutdown cancel the event pump and reap
      the owned child, escalating safely when necessary
- [x] The behavior is tested through the WebSocket/session boundary against a
      scripted HTTP/SSE peer and controllable child
