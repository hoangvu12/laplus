# 01 — Open an OpenCode child work stream

**What to build:** Deliver the first complete inspectable subagent work stream using OpenCode: capture a child's prose and terminal result independently from the parent transcript, persist it, expose lazy replay and live continuation through orchestration, and open it from the compact inline row as a read-only right-panel tab using the main agent's presentation.

**Blocked by:** None — can start immediately

**Status:** ready-for-agent

- [ ] A newly started OpenCode child has stable identity, assignment, lifecycle state, stream reference, and an ordered stream containing its prose and terminal result.
- [ ] The parent transcript retains one compact child row and does not contain the child's detailed prose as ordinary parent messages.
- [ ] Clicking the compact row opens a normal resource-addressed child tab in the existing right-panel workspace; clicking it again activates the same tab rather than duplicating it.
- [ ] The child tab is read-only, has no additional identity/task header or composer, and renders child prose and result using the main agent's existing message/work language.
- [ ] Closing the tab only hides it and does not stop or alter the child; clicking the compact row reopens the same stream.
- [ ] Opening the tab lazily replays persisted entries and continues with live entries without loss, duplication, or ordering changes at the replay/live boundary.
- [ ] Reloading after child completion replays the same complete stream and terminal result.
- [ ] Ordinary parent-thread snapshots carry only the compact child index rather than the complete child stream.
- [ ] Deleting the parent thread deletes the recorded child stream.
- [ ] Existing OpenCode socket-provider tests prove the behavior through the orchestration boundary, with focused contract, client-state, and rendering tests supporting that external seam.
