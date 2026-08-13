# 02 — Fast and resilient local OpenCode discovery

**What to build:** Make the live check less likely to need the remembered
fallback by running independent catalogue work concurrently and retrying
transient failures once.

**Blocked by:** 01 — Per-instance remembered provider catalogue.

**Status:** done

- [x] Run model and agent CLI calls concurrently with their existing bounds.
- [x] Retry only failed calls once after approximately one second.
- [x] Treat persistent model failure as authoritative discovery failure.
- [x] Degrade persistent agent and skill failure to missing enrichment.
- [x] Preserve cancellation, process-tree cleanup, diagnostics, and ordering.
- [x] Add deterministic command doubles proving overlap and selective retry.
