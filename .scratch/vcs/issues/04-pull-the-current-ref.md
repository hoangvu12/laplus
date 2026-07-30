# 04 — Pull the current ref

**What to build:** `vcs.pull`, answered rather than refused, so that a client
which asks to bring the current ref up to date with its tracking ref gets that
rather than a refusal.

The pull is **fast-forward only**. No stash, no rebase, no merge strategy, no
dirty-tree pre-check. A branch that has diverged is stopped rather than merged
on the developer's behalf, and the failure carries git's own stderr instead of a
paraphrase. This is a deliberate refusal to make a history decision for the
developer, and it is what upstream does.

It refuses to guess in two more places, each by name: a detached HEAD is told
that is why it will not run, and a ref with no tracking ref is told to push with
an upstream first — so the developer knows the fix is theirs and what it is.

The result says whether anything actually arrived. That is decided by comparing
the commit before the pull with the commit after, not by reading git's output —
same commit means nothing moved, a different one means it did. It also reports
the tracking ref that was followed, read from the status **after** the pull, so
it is the one that is true afterwards.

Two things this needs that do not exist yet:

- **The tracking ref's name is not kept.** The porcelain parse in `crate::git`
  currently records only _that_ a branch has an upstream, discarding the name git
  hands it. The result declares that name, so the parse starts keeping it. This
  is a change to an existing parse, not a new read, and nothing else that already
  knows about tracking refs is affected.
- **The suite has never set up a remote.** A bare repository in a second
  temporary directory serves as the origin, and a clone of it pushes a divergent
  commit so there is something to pull. No network, so this stays as hermetic as
  the rest of the suite. The helper belongs in the workspace harness beside the
  existing repository and commit helpers, because it is fixture-building rather
  than a new boundary — upstream builds its remote fixtures the same way.

Note that **no UI calls this today.** This half is parity: the contract declares
it, the client runtime registers it, and anything that calls it currently gets a
refusal. It is the cheapest of the three to answer and the one with everyday
value once something does call it. No pull button is in scope.

Follows the shape `crate::refs` established, uses the error union `crate::git`
already builds, and disturbs the kept working tree on the way out. Failing to
disturb never fails a pull that succeeded.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] A branch behind its tracking ref fast-forwards, reports that it pulled,
      and the new commits are on disk
- [ ] A branch already level with its tracking ref reports that it skipped as
      up to date, and its commit is unchanged
- [ ] A branch that has diverged is refused, carries git's own message, and its
      working tree is exactly as it was afterwards
- [ ] A detached HEAD is refused with a message naming that as the reason
- [ ] A ref with no tracking ref is refused with a message saying to push with
      an upstream first
- [ ] A working tree that is not a repository is refused the way the existing
      ref methods refuse it
- [ ] The tracking ref named in the result matches what the status panel reports
      after the pull
- [ ] The porcelain parse keeps the tracking ref's name, asserted in
      `crate::git`'s own module tests as an addition to the existing
      tracking-branch case rather than a new seam
- [ ] Everything else that reads tracking state — the ahead and behind counts,
      and the difference between no upstream and an upstream we are level with —
      still behaves as it did
- [ ] The kept working tree is marked stale after a successful pull, so the
      panel stops saying the branch is behind
- [ ] Tested through the socket against real repositories built with the `git`
      binary, asserting both what happened on disk and what the status panel
      says — nothing reaches into `crate::refs` or `crate::git` directly
