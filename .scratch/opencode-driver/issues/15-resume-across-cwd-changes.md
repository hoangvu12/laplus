# 15 — Resume safely across restarts and CWD changes

**What to build:** An OpenCode thread survives a Laplus restart without hiding
lost context. The driver re-adopts exactly the cursor's session, starts fresh
only for a structured missing session, reapplies permissions, and preserves
history when the working directory changes by verified fork and move behavior.

**Blocked by:** 06 — Retire the agent-session-id concept; 10 — Connect to
operator-owned OpenCode servers; 11 — Normalize streaming, status and titles;
13 — Render tools and answer permissions.

**Status:** ready-for-human

- [x] Restart re-adopts the exact OpenCode session named by a valid v1 cursor
      and continues its history
- [x] A structured missing-session response creates a fresh session and replaces
      the cursor honestly
- [x] Transport, authentication, decoding and other server failures preserve the
      cursor and fail visibly
- [x] An in-place recovery reapplies the thread's current permission rules
- [x] A canonical CWD mismatch forks history and adopts the result only after
      its returned directory is verified
- [x] A fork that remains in the source directory is followed by move-session
      and a second verification, matching captured 1.18.10 behavior
- [x] A failed fork or move never replaces the durable cursor with an unverified
      session
- [x] Socket restart tests cover owned and external peers, missing sessions and
      both CWD migration variants

## Where it landed

- Strict cursor recovery and verified adoption: `server/crates/laplus-server/src/opencode.rs`
- HTTP operation fixtures and protocol coverage: `server/crates/laplus-server/tests/opencode_protocol.rs`, `server/fixtures/opencode-http-sse/operations.json`
- Owned/external restart, failure, missing-session, and migration coverage: `server/crates/laplus-server/tests/socket_opencode_turn.rs`
