# 03 — Create a worktree

**What to build:** `vcs.createWorktree`, answered rather than refused, so that a
developer can have two branches of one project checked out at once without
leaving laplus — either checking out a ref that already exists, or starting a new
ref branched from an existing one.

The developer may name the folder, or leave it to the server. When they leave it,
the server puts it under the preferences directory —
`<preferences>/worktrees/<repository folder name>/<ref name>` — with slashes in
the ref name flattened to dashes, so `feature/thing` is one folder rather than a
nest. That keeps worktrees together in one predictable place, beside the
database, the logs and the registry, rather than scattered next to checkouts. It
is upstream's layout with laplus's directory substituted.

The contract offers three ref inputs and they make **two** legal shapes, not
eight:

| Given                     | Means                                                   |
| ------------------------- | ------------------------------------------------------- |
| a ref alone               | check that ref out into a new worktree                  |
| a ref plus a new ref name | create the new ref at the given one, check it out there |
| plus a base ref name      | records a merge-base hint in git config; nothing else   |

The base ref name is metadata only and is ignored entirely without a new ref
name. Anything outside those shapes is refused by the read.

Follows the shape `crate::refs` established, uses the error union `crate::git`
already builds, and disturbs the kept working tree on the way out — and the new
worktree's own folder too, if one is kept for it. Failing to disturb never fails
a creation that succeeded. Git already refuses to create a worktree where
something exists, or on a ref that is current somewhere else; those refusals
reach the developer in git's own words rather than being pre-checked.

**Blocked by:** None — can start immediately.

**Status:** ready-for-agent

- [ ] A worktree created on an existing ref has that ref current in the new
      folder
- [ ] A worktree created with a new ref name has the new ref, branched from the
      given one, current in the new folder
- [ ] A base ref name given alongside a new ref name records the merge-base hint
      in git config, and the recorded value has any remote prefix stripped
- [ ] A base ref name given without a new ref name changes nothing
- [ ] The path and the ref of the worktree that was made are both reported back
- [ ] A named location is used as given
- [ ] An unnamed location lands under the preferences directory, in the
      documented layout
- [ ] A ref name containing a slash produces one flat folder, not nested
      directories
- [ ] Creating where something already exists is refused, and nothing is
      overwritten
- [ ] Creating on a ref already current in another worktree is refused in git's
      own words
- [ ] A working tree that is not a repository is refused the way the existing
      ref methods refuse it
- [ ] The branch picker reflects the new worktree without a manual refresh
- [ ] Tested through the socket against a real repository built with the `git`
      binary, asserting both what happened on disk and what the status panel
      says — nothing reaches into `crate::refs` or `crate::git` directly
- [ ] Worktrees created under test land in the test's own preferences directory,
      never in a real `~/.laplus/`
