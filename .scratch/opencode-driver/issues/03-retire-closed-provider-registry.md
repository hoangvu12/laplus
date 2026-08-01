# 03 — Retire the closed built-in registry

**What to build:** Complete the provider-instance migration by removing the
temporary closed-registry fallback. Every provider snapshot, refresh, thread
route and session launch now resolves an instance through the generic registry,
leaving one provider-routing model for OpenCode and future drivers.

**Blocked by:** 02 — Migrate built-in providers to configured instances.

**Status:** ready-for-agent

- [ ] No provider operation depends on a closed list of built-in instance ids
- [ ] Claude and Codex default instances remain visible and usable with the
      same durable identities
- [ ] Unknown, disabled and mismatched instances fail through one consistent
      instance-resolution boundary
- [ ] Provider snapshots, refreshes and session launches all resolve the same
      configuration for a given instance id
- [ ] The temporary compatibility path from ticket 01 is removed
- [ ] Focused socket tests remain green for both existing providers
