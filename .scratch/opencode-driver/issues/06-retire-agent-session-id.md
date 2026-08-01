# 06 — Retire the agent-session-id concept

**What to build:** Complete the continuation migration so persistence,
orchestration and driver interfaces speak only in provider resume cursors. The
temporary dual-write/read compatibility machinery is removed after every
existing driver has migrated, while old database rows remain readable.

**Blocked by:** 05 — Migrate Claude and Codex continuation.

**Status:** ready-for-human

- [x] New and updated threads persist only the provider resume cursor form
- [x] Historical rows containing legacy strings remain readable through driver
      v0 decoding
- [x] Shared session and thread vocabulary no longer treats continuation as a
      provider-neutral string id
- [x] The temporary expansion path from ticket 04 is removed without a flag day
      database rewrite
- [x] Claude and Codex restart and missing-session behavior remains green
- [x] The domain glossary and durable schema documentation use provider resume
      cursor consistently
