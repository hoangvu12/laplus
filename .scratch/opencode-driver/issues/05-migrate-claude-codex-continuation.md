# 05 — Migrate Claude and Codex continuation

**What to build:** Claude and Codex adopt the provider resume cursor boundary.
Legacy stored strings are read as each driver's v0 cursor, new successful opens
write the versioned representation, and malformed or unsupported cursors fail
visibly instead of starting an empty conversation.

**Blocked by:** 04 — Expand persistence with provider resume cursors.

**Status:** done

- [x] Legacy Claude strings resume through a documented Claude v0 cursor
- [x] Legacy Codex strings resume through a documented Codex v0 cursor
- [x] Successful Claude and Codex opens write their provider-owned versioned
      cursor through the generic boundary
- [x] A malformed cursor and an unsupported future version surface an
      incompatible-continuation failure
- [x] Existing provider-specific missing-session behavior remains unchanged
- [x] Restart tests cover legacy, migrated, malformed and future-version cases

## Comments

### Delivered

Claude owns a v1 `{version, sessionId}` cursor and Codex owns a v1
`{version, threadId}` cursor. Each driver reads the legacy string only when no
provider cursor exists, writes its v1 cursor after a successful open, and rejects
malformed or newer versions before starting a provider process.

The restart tests make the legacy column disagree with a migrated cursor to prove
the cursor wins, and restore malformed and future cursors for both drivers to
prove the failure reaches the conversation without silently starting fresh.
Claude's existing refused-resume explanation and Codex's recoverable
`thread/resume` fallback remain covered by their provider-specific socket suites.
