# 04 — Expand persistence with provider resume cursors

**What to build:** Add an opaque, versioned provider resume cursor beside the
legacy agent-session-id representation. Persistence and the shared driver seam
can carry the new value end to end while Claude and Codex continue reading and
writing their existing representation unchanged.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] A thread can persist and restore opaque provider-owned continuation JSON
- [ ] The cursor survives snapshots, events, restart and session launch without
      being interpreted by persistence or orchestration
- [ ] Existing agent session ids continue to round-trip exactly as before
- [ ] A driver can return a new cursor through the shared session boundary
      without changing transcript or activity behavior
- [ ] Cursor and legacy values cannot be silently assigned to the wrong provider
- [ ] Storage and socket-level tests cover old rows and new cursor-bearing rows
