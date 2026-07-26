# 19 — Working tree status with live refresh

**What to build:** A developer sees what has changed in their working tree, and it
stays accurate while they work — updating as the agent edits files rather than
needing a manual refresh. This is how the developer tells what the agent actually
did.

Git is driven by shelling out to the installed `git` binary. No library linkage in
v1.

**Blocked by:** 05 (Project registry), 04 (First streaming subscription).

**Status:** ready-for-agent

- [ ] Working tree status shows modified, added, deleted and untracked files
- [ ] Status refreshes as files change on disk, without manual action
- [ ] The current branch is shown
- [ ] A project that is not a repository is reported as such rather than as an
      error
- [ ] A missing `git` binary produces a clear diagnostic
- [ ] Status in a very large repository does not stall the UI, and rapid changes
      are coalesced rather than triggering a refresh per file
- [ ] Repositories in detached-HEAD or mid-merge states are reported without
      crashing
- [ ] Tests drive status and live refresh through the socket boundary against a
      temporary repository
