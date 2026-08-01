# 15 — Resume safely across restarts and CWD changes

**What to build:** An OpenCode thread survives a Laplus restart without hiding
lost context. The driver re-adopts exactly the cursor's session, starts fresh
only for a structured missing session, reapplies permissions, and preserves
history when the working directory changes by verified fork and move behavior.

**Blocked by:** 06 — Retire the agent-session-id concept; 10 — Connect to
operator-owned OpenCode servers; 11 — Normalize streaming, status and titles;
13 — Render tools and answer permissions.

**Status:** ready-for-agent

- [ ] Restart re-adopts the exact OpenCode session named by a valid v1 cursor
      and continues its history
- [ ] A structured missing-session response creates a fresh session and replaces
      the cursor honestly
- [ ] Transport, authentication, decoding and other server failures preserve the
      cursor and fail visibly
- [ ] An in-place recovery reapplies the thread's current permission rules
- [ ] A canonical CWD mismatch forks history and adopts the result only after
      its returned directory is verified
- [ ] A fork that remains in the source directory is followed by move-session
      and a second verification, matching captured 1.18.10 behavior
- [ ] A failed fork or move never replaces the durable cursor with an unverified
      session
- [ ] Socket restart tests cover owned and external peers, missing sessions and
      both CWD migration variants
