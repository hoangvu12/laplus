# 05 — Migrate Claude and Codex continuation

**What to build:** Claude and Codex adopt the provider resume cursor boundary.
Legacy stored strings are read as each driver's v0 cursor, new successful opens
write the versioned representation, and malformed or unsupported cursors fail
visibly instead of starting an empty conversation.

**Blocked by:** 04 — Expand persistence with provider resume cursors.

**Status:** ready-for-agent

- [ ] Legacy Claude strings resume through a documented Claude v0 cursor
- [ ] Legacy Codex strings resume through a documented Codex v0 cursor
- [ ] Successful Claude and Codex opens write their provider-owned versioned
      cursor through the generic boundary
- [ ] A malformed cursor and an unsupported future version surface an
      incompatible-continuation failure
- [ ] Existing provider-specific missing-session behavior remains unchanged
- [ ] Restart tests cover legacy, migrated, malformed and future-version cases
