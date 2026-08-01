# 08 — Discover and configure OpenCode instances

**What to build:** OpenCode becomes a configurable provider driver whose local
and external instances can be saved, probed and refreshed. Each instance
publishes its actual health, version, connected models, visible primary agents,
variants and configured custom fallbacks to the existing picker surface.

**Blocked by:** 03 — Retire the closed built-in registry; 07 — Own the narrow
OpenCode HTTP/SSE protocol.

**Status:** ready-for-agent

- [ ] Local settings support enabled state, binary path and custom model
      fallbacks and reject unsupported installed versions clearly
- [ ] External settings support HTTP or HTTPS URL and optional password without
      requiring a local CLI for ordinary discovery
- [ ] Local catalogue discovery obtains models and visible agents without
      keeping a conversation server alive
- [ ] External discovery obtains health, version, connected providers, models
      and agents through the configured endpoint
- [ ] Model ids preserve `provider/model` identity and expose supported agent
      and variant options
- [ ] Only connected providers contribute discovered models while configured
      custom fallbacks remain selectable
- [ ] Multiple OpenCode instances refresh and fail independently through socket
      tests against scripted peers and fake binaries
