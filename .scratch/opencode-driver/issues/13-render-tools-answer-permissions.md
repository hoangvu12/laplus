# 13 — Render tools and answer permissions

**What to build:** OpenCode tool work appears in the shared work log and its
permission requests can be answered from Laplus. Runtime modes become
T3-compatible OpenCode rules, known tool and request kinds get useful shared
representations, and unknown kinds stay visible rather than disappearing.

**Blocked by:** 11 — Normalize streaming, status and titles.

**Status:** ready-for-human

- [x] Command, file, web, MCP, image and collaboration tool states map to their
      shared work-log representations from start through completion or failure
- [x] Unknown tools render as generic dynamic tools and retain diagnostic raw
      state
- [x] Full access allows every OpenCode permission; the other runtime modes ask
      for sensitive operations while allowing the question capability
- [x] Bash, read and edit permission requests map to specific request kinds and
      unknown permissions remain answerable
- [x] Accept, accept-for-session, decline and cancel become once, always or
      reject replies as specified
- [x] Pending permission identity is explicit and resolved requests disappear
      when OpenCode reports their reply
- [x] Retuning and resumed-session permission application use the same mapping
- [x] Socket tests cover tool ordering, every decision and unknown kinds

## Where it landed

- `server/crates/laplus-server/src/opencode_protocol.rs` recognizes the current
  v2 permission request event while retaining the distinct deprecated v1 event.
- `server/crates/laplus-server/src/opencode.rs` owns the shared create/adopt/
  retune permission rules, v2 reply operation, pending identities, upstream
  reply cleanup, and tool-part-to-work-log translation with raw fallback.
- `server/crates/laplus-server/tests/opencode_protocol.rs` pins the session
  `PATCH` operation; `socket_opencode_turn.rs` drives tool ordering, known and
  unknown categories, runtime rules, all four decisions, and reply cleanup
  through the scripted peer and WebSocket seam.
