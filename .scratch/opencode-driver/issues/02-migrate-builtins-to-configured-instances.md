# 02 — Migrate built-in providers to configured instances

**What to build:** Claude and Codex run through the generic provider-instance
path introduced by ticket 01. Their default instance identities, settings,
catalogues, snapshots, refresh behavior and existing thread routing remain
compatible while the old registry still exists as a fallback.

**Blocked by:** 01 — Expand the provider-instance registry.

**Status:** ready-for-agent

- [ ] The default Claude and Codex instances are represented by the generic
      registry with their existing durable instance ids
- [ ] Existing settings decode into equivalent default-instance configuration
      without user migration work
- [ ] Provider snapshots and targeted refreshes for both built-ins travel
      through the generic instance path
- [ ] Existing threads reopen under the same driver and instance they recorded
- [ ] Claude and Codex turns, catalogues and runtime retuning retain their
      current behavior
- [ ] Compatibility tests prove old persisted settings and threads still work
