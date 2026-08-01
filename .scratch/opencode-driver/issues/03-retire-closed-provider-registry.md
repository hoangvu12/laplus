# 03 — Retire the closed built-in registry

**What to build:** Complete the provider-instance migration by removing the
temporary closed-registry fallback. Every provider snapshot, refresh, thread
route and session launch now resolves an instance through the generic registry,
leaving one provider-routing model for OpenCode and future drivers.

**Blocked by:** 02 — Migrate built-in providers to configured instances.

**Status:** ready-for-human

- [x] No provider operation depends on a closed list of built-in instance ids
- [x] Claude and Codex default instances remain visible and usable with the
      same durable identities
- [x] Unknown, disabled and mismatched instances fail through one consistent
      instance-resolution boundary
- [x] Provider snapshots, refreshes and session launches all resolve the same
      configuration for a given instance id
- [x] The temporary compatibility path from ticket 01 is removed
- [x] Focused socket tests remain green for both existing providers

**Where it landed.** `Settings.provider_instances` now contains the durable
`claudeAgent` and `codex` defaults from first launch. Legacy `providers` input is
normalized into those envelopes at the settings boundary, with an explicit
default envelope winning independent of JSON field order. Driver registration
is keyed by driver slug rather than instance id, and startup probes, targeted
refreshes, snapshots and session preparation all resolve the same configured
instance. Removing an instance also invalidates its probe and removes its stale
snapshot. The final review added the shared `ConfiguredInstance` resolution
boundary, which refuses disabled and mismatched routes before thread creation or
session launch and keeps disabled instances visible only as snapshots.
