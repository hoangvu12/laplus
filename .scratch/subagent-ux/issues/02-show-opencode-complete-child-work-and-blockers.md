# 02 — Show OpenCode's complete child work and blockers

**What to build:** Expand the OpenCode tracer so its child tab shows the complete work OpenCode exposes—commands, output, reads, searches, edits and diffs, tools, errors, blockers, and result—while actionable descendant requests remain impossible to miss in the main conversation.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-agent

- [ ] An OpenCode child stream preserves chronological prose, command invocation/output/status, file reads and searches, edits with diff navigation, other tool calls/results, warnings/errors, and terminal outcomes when those events are present.
- [ ] The child tab renders those entries through the same semantic UI used for equivalent main-agent work rather than a raw event log.
- [ ] File and diff actions from the child open neighboring existing right-panel surfaces without closing or replacing the child tab.
- [ ] The compact parent row shows the latest meaningful child activity while running, ignoring transport noise and unhelpful partial states.
- [ ] Terminal state atomically replaces stale activity with a bounded result, failure, interruption, or empty-result preview while the full outcome remains in the child stream.
- [ ] A child-owned permission or question is persisted in the child stream and also appears as an actionable request in the main conversation identifying the waiting child.
- [ ] Answering or rejecting a descendant request routes through the originating child's provider request identity and records resolution in that child's stream.
- [ ] A blocker remains actionable when the child tab is closed or another right-panel tab is active.
- [ ] Providers' unknown future child event variants remain forward-compatible under the existing drift policy instead of breaking the parent turn or child stream.
- [ ] Scripted OpenCode tests cover rich child activity and a descendant permission/question through the external orchestration boundary.
