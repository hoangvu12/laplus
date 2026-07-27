# 21 — Branches: list, switch, create, init

**What to build:** A developer manages branches without dropping to a shell. They
see what branches exist, switch between them to keep work separated, create a new
one to start work, and initialise a repository in a project that has none so that
agent changes become reviewable.

**Blocked by:** 19 (Working tree status with live refresh).

**Status:** done

- [x] Branches are listed with the current one indicated
- [x] Switching branches updates the working tree and the displayed status
- [x] A new branch can be created from the current position
- [x] A repository can be initialised in a project that has none, after which
      status works
- [x] A switch blocked by uncommitted changes is refused with an explanation, not
      silently or destructively
- [x] Creating a branch whose name already exists fails with a clear message
- [x] An invalid branch name is rejected before it reaches the git binary
- [x] Tests cover list, switch, create, init and the blocked-switch case through
      the socket boundary

## Comments

### What was built

A new module, `crates/lightcode-server/src/refs.rs`, with four methods:

- `vcs.listRefs` — the branch picker, with the query and the paging it sends.
- `vcs.switchRef` — choosing from it.
- `vcs.createRef` — starting work that has no branch yet, optionally switching
  to it in the same call (which is what the picker's "create branch" sends).
- `vcs.init` — the project that is not a repository at all.

All four are `Deferred`, because all four run git. None streams: the panel that
has to keep up is `subscribeVcsStatus`, and it already does.

`crate::git` grew one method, `Repositories::disturb` — a call saying it changed
a working tree, marking it stale through the same door a file change comes
through. The watcher would notice `.git/HEAD` moving on its own; saying so
directly is what makes "switch, then read the panel" a sequence rather than a
race, and it is why the switch test can assert on the panel without a sleep.

### The decision worth reading

**ADR-0007** — a branch name is checked here, a switch is not. Two refusals the
ticket asks for that look like the same kind of thing and are not: one is a rule
about a string that never changes, and the other is a fact about a working tree
that only git knows. The consequence in the code is that `crate::git::refusal`
now keeps *whole* stderr rather than its first line, because a blocked switch's
last line is the one worth reading — bounded at 20 lines and 2,000 characters.

Worth stating plainly because it is what "not destructively" means here: there
is no `--force`, no `--discard-changes` and no `-B` anywhere in `crate::refs`.
Losing uncommitted work through this server would take git deciding it was not
at risk.

### What the capture said

`fixtures/socket-wire/01-browser-session.ndjson` holds a real `vcs.listRefs`,
and `the_branch_listing_conforms_to_the_capture` compares against it with **no
declared divergences** — the only conformance test in the suite that can say
that. The capture is also what fixes two defaults that the contract's field
names only hint at: the captured repository had `origin/main` and `origin/HEAD`
and listed neither, so

- a **symbolic** remote ref (`origin/HEAD`) is never a row, because it is a
  pointer at another row; and
- a remote ref whose branch has a local counterpart is **folded away** unless
  `includeMatchingRemoteRefs` asks for it.

### Four things worth knowing

- **Switching to a remote ref makes the local branch that tracks it.** The
  picker can list `origin/feature`, and a working tree cannot be *on* one, so a
  switch to it runs `switch --create feature --track origin/feature` rather than
  detaching `HEAD`. Without this the remote half of the listing would be
  decorative.
- **Where a remote's name ends is asked of the repository, never of the
  string.** A remote may be called `origin/mirror` and a branch may be called
  `feature/x`, so only the list of remotes can say which part of
  `origin/mirror/main` is which. `Bearings::split_remote` is the one answer, and
  the listing and the switch both use it — they have to agree, because a switch
  is asked for by a name the listing produced.
- **A repository with no commit yet still lists a branch.** `HEAD` names one
  that has no ref behind it — which is every project the moment after
  `vcs.init` — and `crate::git` already reports that branch in the status, so
  the picker synthesises the same row rather than letting the two disagree. The
  same state is the one place "from the current position" has no answer:
  `createRef` without `switchRef` is refused there with the reason, because
  there is no commit for a second branch to point at.
- **`vcs.init` declares a different error union from the other three.** Its
  errors are `VcsError`, which has no `GitCommandError` in it, so an unusable
  folder is a `VcsRepositoryDetectionError`, a git that would not run is a
  `VcsProcessSpawnError`, and `kind: "jj"` is a `VcsUnsupportedOperationError`
  rather than a git repository nobody asked for. It answers with `null`, because
  it declares no success value.

### What the review caught

Two real bugs, both fixed with a socket test that fails without the fix:

- **`refKind: "remote"` answered with the branches nobody had checked out.**
  The fold — dropping `origin/main` when `main` is there — is about a *pair* of
  rows, and it was being applied to a listing that only has one side of every
  pair. On an ordinary clone that made "what is on the remote" return nothing at
  all. (`asking_for_the_remote_side_is_not_answered_by_the_fold`)
- **A switch split a remote-tracking name at its first slash.** With a remote
  called `origin/mirror`, switching to the `origin/mirror/main` the listing
  itself produced would have left the developer on a local branch called
  `mirror/main`. (`a_remote_with_a_slash_in_its_name_is_split_where_the_remote_ends`)

And one gap: **the whole remote half had no socket coverage**, only unit tests
over hand-built values that never came out of git. Both bugs above lived
exactly there. `cloned()` in the test file makes a real clone, which is the only
way to get `refs/remotes/*` and the symbolic `origin/HEAD` that must never be a
row.

Plus a name-length rule that was one rule where it should have been two: 255 is
the filesystem's limit on one *part* of a name, so applying it to the whole name
would have let a listing return `a/b/c/…` that a switch to it then refused as
invalid. Per part now, with a generous whole-name backstop that is this module's
own rather than git's.

### Left short of what a reader might assume

- **The listing is capped at 10,000 refs** (`MAX_REFS`), and unlike the status's
  file cap this one bounds the *answer* as well as the work: `totalCount` counts
  what was considered. A repository with more refs than that has a bot pushing
  into it, and the branch there is found with the query rather than by
  scrolling. A cut is logged.
- **`refKind` and `includeMatchingRemoteRefs` are implemented but unexercised by
  the capture**, which sends neither. They are the contract's, and refusing to
  implement them would have made `refKind: "remote"` a lie rather than a gap.
- **`Project::repository_identity` is still `null`.** Ticket 19's notes named
  this ticket as its filler, on the reading that `vcs.init` is where a
  repository's identity is established. Having built it: it is not. Nothing in
  ticket 21's acceptance criteria asks for the field, `VcsRepositoryIdentity`
  wants a `freshness` with an observation clock behind it, and the field belongs
  to the *project registry* rather than to any of these four calls. It stays
  where it is, and the note is here so the next reader does not go looking for
  it in `refs.rs`.
