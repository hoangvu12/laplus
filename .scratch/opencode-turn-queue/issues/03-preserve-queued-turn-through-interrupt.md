# 03 — Preserve the queued turn through Interrupt

**What to build:** Make A → Interrupt → B reliable when OpenCode acknowledges
the interrupt before it reports idle. Interrupt only A, retain its partial reply
as interrupted history, keep B durable, and start B as a new turn after A
settles. Reconcile boundedly when the expected settlement event is late so the
conversation cannot remain working forever.

**Blocked by:** 02 — Queue OpenCode messages during active work.

**Status:** ready-for-human

- [x] A scripted OpenCode peer can acknowledge abort while withholding its idle event, reproducing the reported race.
- [x] B submitted during that interval is stored as queued work and is never attached to A as a steer.
- [x] Navigating away and loading a fresh snapshot before idle still shows B.
- [x] Releasing idle settles A as interrupted, preserves its partial assistant reply, and starts B with a new turn identity.
- [x] If B and C are queued, both survive Interrupt and start together in their original order.
- [x] A late idle event cannot settle or corrupt the new turn.
- [x] A missing idle event triggers bounded session reconciliation instead of an endless Working state.
- [x] Focused WebSocket orchestration regression tests cover delayed idle, late idle, and missing idle behavior.
- [ ] The exact A → Interrupt → B sequence is verified in a rebuilt running application, and all verification processes are stopped afterward.

## Verification

2026-08-15: the focused OpenCode socket suite passed all delayed-idle,
late-idle, missing-idle, B/C ordering, and authoritative-snapshot cases. The
production web bundle rebuilt successfully and booted in an isolated browser
profile; exact interactive A → Interrupt → B acceptance remains for a human.
