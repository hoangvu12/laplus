# 17 — Generate short structured text through OpenCode

**What to build:** OpenCode can generate commit messages, pull-request text,
branch names and thread titles outside a conversation. Each request is isolated
in a deny-all temporary session; local requests share an idle-reaped server and
external requests use their configured endpoint.

**Blocked by:** 08 — Discover and configure OpenCode instances; 10 — Connect to
operator-owned OpenCode servers.

**Status:** ready-for-human

- [x] Each supported text-generation operation routes to the selected OpenCode
      instance and returns its expected contract result
- [x] Every request uses a temporary session with all tool permissions denied
      and no arbitrary conversation history
- [x] Structured output is validated and sanitized for commit, pull request,
      branch or title use
- [x] Local operations reuse one generation server across nearby requests
      without sharing their sessions
- [x] The local generation server is reaped after the thirty-second idle
      decision without tests asserting elapsed wall-clock duration
- [x] External operations use the configured endpoint and never claim its
      lifetime
- [x] Scripted-peer tests cover valid, malformed, tool-attempt and cleanup paths

Where landed: `server/crates/laplus-server/src/text_generation.rs` provides the
generic operation boundary, destination validation and the owned-server pool;
the narrow synchronous prompt and temporary-session deletion calls live beside
the rest of the OpenCode HTTP adapter in `src/opencode.rs`. Scripted external
and owned peers in `tests/opencode_text_generation.rs` cover routing, deny-all
sessions, every result shape, malformed/tool output, timeout cleanup, local
reuse and the explicit thirty-second reap decision.
