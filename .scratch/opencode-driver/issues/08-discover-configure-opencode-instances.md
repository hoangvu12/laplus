# 08 — Discover and configure OpenCode instances

**What to build:** OpenCode becomes a configurable provider driver whose local
and external instances can be saved, probed and refreshed. Each instance
publishes its actual health, version, connected models, visible primary agents,
variants and configured custom fallbacks to the existing picker surface.

**Blocked by:** 03 — Retire the closed built-in registry; 07 — Own the narrow
OpenCode HTTP/SSE protocol.

**Status:** ready-for-human

- [x] Local settings support enabled state, binary path and custom model
      fallbacks and reject unsupported installed versions clearly
- [x] External settings support HTTP or HTTPS URL and optional password without
      requiring a local CLI for ordinary discovery
- [x] Local catalogue discovery obtains models and visible agents without
      keeping a conversation server alive
- [x] External discovery obtains health, version, connected providers, models
      and agents through the configured endpoint
- [x] Model ids preserve `provider/model` identity and expose supported agent
      and variant options
- [x] Only connected providers contribute discovered models while configured
      custom fallbacks remain selectable
- [x] Multiple OpenCode instances refresh and fail independently through socket
      tests against scripted peers and fake binaries

## What it turned out to be

OpenCode is the third member of the generic provider registry. Its instance
configuration carries either a local binary or an HTTP(S) endpoint, plus an
optional password and custom model fallbacks. The existing per-instance probe
reservation and publication ordering is reused unchanged, so one slow or broken
instance cannot replace or suppress a sibling's snapshot.

External discovery uses the narrow client from ticket 07 for health, providers
and agents. Local discovery runs three short-lived commands (`--version`,
`models --verbose`, and `agent list`) and never starts `opencode serve`.
Versions older than 1.14.19 are present but unavailable. Discovered model slugs
retain `provider/model`; visible primary agents and variants are emitted as the
existing model option descriptors, and authored fallbacks are appended even
when upstream inventory is incomplete.

Starting a turn remains ticket 09. Selecting an OpenCode instance before that
ticket lands is refused explicitly at the session preparation boundary rather
than being routed through another driver.
