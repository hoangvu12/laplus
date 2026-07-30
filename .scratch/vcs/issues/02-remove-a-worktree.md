# 02 — Remove a worktree

**What to build:** `vcs.removeWorktree`, answered rather than refused, so that a
developer deleting a conversation that lives in a worktree can say yes to the
offer to remove the worktree and have it actually happen.

Today that offer is reachable and its yes branch fails: the conversation is
deleted, the worktree stays on disk, and the developer is shown an error after
the fact. This is the one flow in the whole effort with a live UI path.

The method takes a working tree and a path, and removes the checkout at that
path. Without force, git refuses a worktree with uncommitted changes in it and
that refusal reaches the developer intact — laplus does not soften it. With
force, the removal goes ahead, which is what the delete-conversation flow asks
for. The ref the worktree held always survives: removing a checkout is not
deleting a branch. A path that is not a worktree of this repository is refused by
git in git's own words rather than pre-checked, because git's message is better
than the one a pre-check would write.

Follows the shape `crate::refs` established — a read of the payload that yields
either a typed request or a refusal, and a run that takes the shared working-tree
registry — and uses the error union `crate::git` already builds. On the way out
it disturbs the kept working tree, exactly as a switch and an init already do;
see ADR-0006 for why that marks rather than reads. Failing to disturb never
fails a removal that succeeded.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] A worktree named by path is removed, and the folder is gone from disk
- [ ] The ref the worktree held is still listed by the branch picker afterwards
- [ ] A worktree with uncommitted changes is refused when force is not asked
      for, and the developer sees git's own reason
- [ ] The same worktree is removed when force is asked for
- [ ] A path that is not a worktree of this repository is refused, and nothing
      on disk is deleted
- [ ] A working tree that is not a repository is refused the way the existing
      ref methods refuse it
- [ ] The kept working tree is marked stale after a successful removal, so the
      status panel reflects it without a manual refresh
- [ ] Tested through the socket against a real repository built with the `git`
      binary, asserting both what happened on disk and what the status panel
      says — nothing reaches into `crate::refs` or `crate::git` directly

**Verification in the running app** (the suite is not evidence the app works):

1. Make a worktree by hand and start a conversation on the ref it holds
2. Delete that conversation and answer yes to the offer to remove the worktree
3. Confirm the worktree folder is gone and no error toast appears
4. Confirm the branch it held is still in the picker
