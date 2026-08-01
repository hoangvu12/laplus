# 01 — Expand the provider-instance registry

**What to build:** A generic provider-instance path beside the current built-in
registry, proven by configuring a second Claude instance and routing a real turn
through it without changing the existing Claude or Codex identities. This is
the expand step: old routing remains available until the built-ins migrate.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] A valid configured provider instance is accepted with its own stable id,
      display name, driver kind, settings and continuation namespace
- [ ] A second Claude instance appears in provider snapshots and can be selected
      for a thread and used for a complete turn
- [ ] Refreshing or disabling the new instance does not affect another instance
      of the same driver
- [ ] Invalid instance ids, unsupported driver kinds and invalid settings are
      refused at the settings boundary with actionable errors
- [ ] Existing built-in Claude and Codex routing remains unchanged during the
      expansion
- [ ] Socket-level tests cover settings, snapshot, selection and turn routing
