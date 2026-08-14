# 02 — Queue OpenCode messages during active work

**What to build:** When an OpenCode turn is active, store later developer
messages as durable queued work instead of steering the active turn. Keep each
message as a separate transcript entry. Start one new turn with all messages
waiting at the settlement boundary, in their original order and with the
settings and content captured at submission time.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [x] Sending B while A is running stores B immediately and does not submit it to OpenCode as a steer.
- [x] Sending B and C while A is running keeps two ordered user messages but starts one next agent turn after A settles.
- [x] A fresh authoritative thread snapshot contains B and C before the queued turn starts.
- [x] The queued turn uses the provider selection, model, mode, attachments, and prompt metadata captured when the messages were sent.
- [x] The completed reply to A remains in the conversation history seen by the queued turn.
- [x] Claude and Codex behavior does not change.
- [x] Focused WebSocket orchestration tests prove the behavior through a scripted OpenCode peer rather than private queue state.
- [ ] The behavior is verified in a rebuilt running application, including navigation away and back, and all verification processes are stopped afterward.
