# 16 — Roll back OpenCode history with checkpoints

**What to build:** Reverting an OpenCode checkpoint restores the working tree
and then moves provider history back by the removed turn count. Completion is
published only after both sides succeed; the defined partial-failure state
remains visible and recoverable.

**Blocked by:** 15 — Resume safely across restarts and CWD changes.

**Status:** ready-for-agent

- [ ] Checkpoint revert restores the filesystem and refreshes the workspace
      index before asking OpenCode to revert history
- [ ] Provider history is rolled back by the exact number of removed turns
- [ ] Later checkpoint references are pruned only after provider rollback
      succeeds
- [ ] Successful completion is published only after tree, provider and ref work
      has finished
- [ ] If provider rollback fails, the restored tree remains, later refs remain,
      failure is reported and false completion is not published
- [ ] The adopted resume cursor remains usable after successful rollback and is
      not silently replaced after failure
- [ ] Socket tests use a real temporary repository and scripted OpenCode peer to
      assert operation order and both outcomes
