Status: ready-for-agent

# 01 — One assistant message per OpenCode text part

**What to build:** The OpenCode driver stops folding a whole turn's narration
into one message. Each provider text part becomes its own assistant message,
keyed by the provider part id so identity survives replay and reload. Text
spoken between tool calls reads **below** those tool calls in the transcript,
live and after restart; settling or interrupting a turn closes every open part
message with whatever it held.

From the developer's seat: no more wall of concatenated text with tools stuck
underneath — commentary reads in the order it was said, like Claude already
renders. Reasoning stays in the work log, unchanged. Codex and Claude drivers
are untouched.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [x] Against the scripted peer: text → tool → text produces two assistant
      messages either side of the tool row, in that order, live.
- [x] The same transcript after a full reload shows the same order (ordinals).
- [x] A part that produces no text produces no message; empty bubbles never
      appear.
- [x] Interrupting mid-turn leaves each partial block with exactly what had
      arrived; nothing is invented after the stop.
- [x] Duplicate/spurious part snapshots cannot double-render a block
      (cumulative-snapshot rule preserved per part).
- [x] Settled-turn fold keeps every intermediate message inside the fold and
      only the terminal message visible — existing client behaviour verified,
      no reducer change expected.
- [ ] A ui-driver walkthrough shows correct placement during a live turn
      (per AGENTS.md: user-visible change gets driven).
- [x] Focused tests pass: OpenCode socket/protocol suites covering the
      interleave scenario, plus the reasoning-stays-work-log case.

## Comments

2026-08-22 review: implementation and focused scripted-peer coverage are in
place on `main`. The live/reload text-part test and interrupted-part test pass
on Windows. This ticket remains open only for the required ui-driver
walkthrough; do not mark it `done` until that user-visible evidence is recorded.
