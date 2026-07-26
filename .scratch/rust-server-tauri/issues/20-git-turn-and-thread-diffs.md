# 20 — Turn and thread diffs

**What to build:** A developer reviews the agent's work as a diff. They can look at
what changed during a single agent turn, to review one step in isolation, or at the
cumulative change across a whole conversation, to review the session as one
coherent change.

This requires turns to be identifiable points in time against the working tree, so
it depends on the agent session lifecycle as well as on git.

**Blocked by:** 19 (Working tree status with live refresh), 10 (One complete agent
turn, streamed).

**Status:** ready-for-agent

- [ ] The diff for a single agent turn can be viewed
- [ ] The cumulative diff for an entire conversation can be viewed
- [ ] Diffs cover added, modified, deleted and renamed files
- [ ] A turn that changed nothing shows an empty diff rather than an error
- [ ] Untracked files created by the agent appear in the diff
- [ ] Binary file changes are indicated without attempting to render content
- [ ] Very large diffs are truncated for display with the truncation made obvious
- [ ] Diffs remain correct when the developer edits files by hand between turns
- [ ] Tests drive both diff views through the socket boundary against a temporary
      repository
