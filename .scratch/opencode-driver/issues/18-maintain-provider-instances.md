# 18 — Maintain provider instances explicitly

**What to build:** The generic provider-maintenance contract becomes usable and
OpenCode advertises the update action appropriate to its resolved installation.
Updates run only when requested, serialize safely, refresh the instance and
report the version actually observed afterwards.

**Blocked by:** 03 — Retire the closed built-in registry; 08 — Discover and
configure OpenCode instances.

**Status:** ready-for-human

- [x] Maintenance is addressed by provider instance and never runs during probe
      or refresh without an explicit request
- [x] Native, npm, pnpm, Bun, Vite+ and Homebrew OpenCode installations expose
      the matching T3-compatible strategy when detected
- [x] Overlapping maintenance is serialized by instance and package manager
- [x] Command success and failure are reported without assuming success changed
      the installed provider
- [x] Every completed command triggers a targeted refresh and reports the
      observed before/after version
- [x] An external instance may advertise maintenance from its configured local
      binary while its refreshed external snapshot remains authoritative
- [x] Contract and socket tests use fake installations and commands and cover
      concurrency, failure and unchanged-version outcomes

## Where landed

- `packages/contracts/src/server.ts` keeps the compatible update-state shape
  and adds optional observed `beforeVersion` / `afterVersion` fields.
- `server/crates/laplus-server/src/provider_maintenance.rs` owns strategy
  resolution, explicit instance routing, command execution, serialization and
  mandatory targeted refresh.
- `server/crates/laplus-server/src/provider.rs` advertises resolved OpenCode
  maintenance without executing it, including for external instances.
- `server/crates/laplus-server/src/rpc.rs` exposes the generic
  `server.updateProvider` deferred boundary.
- `server/crates/laplus-server/tests/socket_provider_maintenance.rs` drives the
  socket against fake commands and external OpenCode installations.
- `apps/web/src/components/settings/SettingsPanels.tsx` keys in-flight update
  guards by provider instance rather than conflating sibling driver instances.
