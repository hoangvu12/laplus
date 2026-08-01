# 17 — Generate short structured text through OpenCode

**What to build:** OpenCode can generate commit messages, pull-request text,
branch names and thread titles outside a conversation. Each request is isolated
in a deny-all temporary session; local requests share an idle-reaped server and
external requests use their configured endpoint.

**Blocked by:** 08 — Discover and configure OpenCode instances; 10 — Connect to
operator-owned OpenCode servers.

**Status:** ready-for-agent

- [ ] Each supported text-generation operation routes to the selected OpenCode
      instance and returns its expected contract result
- [ ] Every request uses a temporary session with all tool permissions denied
      and no arbitrary conversation history
- [ ] Structured output is validated and sanitized for commit, pull request,
      branch or title use
- [ ] Local operations reuse one generation server across nearby requests
      without sharing their sessions
- [ ] The local generation server is reaped after the thirty-second idle
      decision without tests asserting elapsed wall-clock duration
- [ ] External operations use the configured endpoint and never claim its
      lifetime
- [ ] Scripted-peer tests cover valid, malformed, tool-attempt and cleanup paths
