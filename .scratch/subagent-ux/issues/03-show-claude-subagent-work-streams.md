# 03 — Show Claude subagent work streams

**What to build:** Give Claude users the same compact-row-to-read-only-tab experience, preserving every child message and work event Claude truthfully exposes, including background subagents that continue after the root becomes quiet and their eventual outcomes.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-agent

- [ ] A newly captured Claude subagent has stable child identity, assignment, state, stream reference, and an ordered persisted work stream independent from the parent transcript.
- [ ] The existing Claude compact row remains the summary and launcher, showing meaningful live activity followed by a truthful terminal preview.
- [ ] Clicking the row opens or activates a normal read-only child tab using the shared main-agent message/work presentation.
- [ ] Claude child prose, exposed tools/work, errors, and terminal outcome appear in chronological order without being duplicated as parent messages.
- [ ] A background Claude child can continue appending to its stream after the parent root output becomes quiet or its immediate turn settles.
- [ ] Reopening or reloading the child tab replays the same recorded content and resumes live continuation without gaps or duplicates.
- [ ] Claude details or hierarchy that are not present on the protocol are omitted rather than inferred.
- [ ] Child protocol boundaries cannot incorrectly settle or clear the root turn.
- [ ] The recorded background-subagent fixtures prove live state, post-root activity, terminal result, and replay through the external orchestration boundary.
