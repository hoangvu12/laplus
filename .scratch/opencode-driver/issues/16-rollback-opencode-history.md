# 16 — Roll back OpenCode history with checkpoints

**What to build:** Reverting an OpenCode checkpoint restores the working tree
and then moves provider history back by the removed turn count. Completion is
published only after both sides succeed; the defined partial-failure state
remains visible and recoverable.

**Blocked by:** 15 — Resume safely across restarts and CWD changes.

**Status:** ready-for-human

- [x] Checkpoint revert restores the filesystem and refreshes the workspace
      index before asking OpenCode to revert history
- [x] Provider history is rolled back by the exact number of removed turns
- [x] Later checkpoint references are pruned only after provider rollback
      succeeds
- [x] Successful completion is published only after tree, provider and ref work
      has finished
- [x] If provider rollback fails, the restored tree remains, later refs remain,
      failure is reported and false completion is not published
- [x] The adopted resume cursor remains usable after successful rollback and is
      not silently replaced after failure
- [x] Socket tests use a real temporary repository and scripted OpenCode peer to
      assert operation order and both outcomes

## Where landed

- `server/crates/laplus-server/src/orchestration.rs` coordinates restore,
  workspace refresh, OpenCode rollback, ref pruning and final publication.
- `server/crates/laplus-server/src/opencode.rs` translates removed turn counts
  into OpenCode's retained assistant-message boundary without replacing the
  durable cursor.
- `server/crates/laplus-server/src/checkpoints.rs` removes only checkpoint refs
  later than the retained turn, after provider success.
- `server/crates/laplus-server/tests/socket_opencode_turn.rs` covers successful
  and failed rollback across a Laplus restart with a real Git repository and a
  scripted external OpenCode peer.
