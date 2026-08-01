# 14 — Answer questions and send chat attachments

**What to build:** OpenCode can pause a turn for ordered user input and can
receive files attached to a developer prompt. Questions round-trip through the
existing user-input surface; resolved local attachments become file URLs for
owned or shared-filesystem external servers.

**Blocked by:** 10 — Connect to operator-owned OpenCode servers; 13 — Render
tools and answer permissions.

**Status:** ready-for-agent

- [ ] Original-family OpenCode question events render with stable derived ids
      and preserve question and option order
- [ ] Answers and rejection travel through the matching OpenCode question
      operations and resolve the pending request
- [ ] Pending question identity cannot collide with or be inferred as a
      permission request
- [ ] Newer question-v2 events remain observable unknown events until separately
      specified
- [ ] Existing chat attachments resolve through Laplus's attachment store and
      reach OpenCode as local file URLs
- [ ] Unresolved attachment references are omitted without corrupting the rest
      of the prompt
- [ ] Owned and external flows use the same representation, with the external
      shared-filesystem limitation documented and tested
- [ ] Socket tests cover multi-question answers, rejection and attachment parts
