# 14 — Answer questions and send chat attachments

**What to build:** OpenCode can pause a turn for ordered user input and can
receive files attached to a developer prompt. Questions round-trip through the
existing user-input surface; resolved local attachments become file URLs for
owned or shared-filesystem external servers.

**Blocked by:** 10 — Connect to operator-owned OpenCode servers; 13 — Render
tools and answer permissions.

**Status:** ready-for-human

- [x] Original-family OpenCode question events render with stable derived ids
      and preserve question and option order
- [x] Answers and rejection travel through the matching OpenCode question
      operations and resolve the pending request
- [x] Pending question identity cannot collide with or be inferred as a
      permission request
- [x] Newer question-v2 events remain observable unknown events until separately
      specified
- [x] Existing chat attachments resolve through Laplus's attachment store and
      reach OpenCode as local file URLs
- [x] Unresolved attachment references are omitted without corrupting the rest
      of the prompt
- [x] Owned and external flows use the same representation, with the external
      shared-filesystem limitation documented and tested
- [x] Socket tests cover multi-question answers, rejection and attachment parts

Where landed: `server/crates/laplus-server/src/opencode.rs` owns the original
question mapping, dedicated pending-question table, ordered answer/reject wire
and upstream-event resolution. `src/attachments.rs` persists and resolves
composer images, and the shared session prompt carries resolved paths which the
OpenCode adapter encodes as `file:` parts for owned and shared-filesystem
external servers. The contract/client runtime and composer expose a distinct
`thread.user-input.reject` action. Scripted socket tests cover ordered questions,
reply, rejection, persisted and missing attachments; protocol coverage keeps
the question-v2 family observable and unknown.
