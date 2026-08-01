# 18 — Maintain provider instances explicitly

**What to build:** The generic provider-maintenance contract becomes usable and
OpenCode advertises the update action appropriate to its resolved installation.
Updates run only when requested, serialize safely, refresh the instance and
report the version actually observed afterwards.

**Blocked by:** 03 — Retire the closed built-in registry; 08 — Discover and
configure OpenCode instances.

**Status:** ready-for-agent

- [ ] Maintenance is addressed by provider instance and never runs during probe
      or refresh without an explicit request
- [ ] Native, npm, pnpm, Bun, Vite+ and Homebrew OpenCode installations expose
      the matching T3-compatible strategy when detected
- [ ] Overlapping maintenance is serialized by instance and package manager
- [ ] Command success and failure are reported without assuming success changed
      the installed provider
- [ ] Every completed command triggers a targeted refresh and reports the
      observed before/after version
- [ ] An external instance may advertise maintenance from its configured local
      binary while its refreshed external snapshot remains authoritative
- [ ] Contract and socket tests use fake installations and commands and cover
      concurrency, failure and unchanged-version outcomes
