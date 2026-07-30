# Pulling, and worktrees a developer manages

Status: ready-for-agent

Evidence and provenance: `.scratch/contract-parity/ledger.md` item 5, and
`.scratch/vcs/upstream-research.md` for how `pingdotgg/t3code` implements the
same three methods. This spec covers item 5 only.

## Problem Statement

Three methods this server declares are refused, and the developer meets that
refusal in three different ways — one of which is worse than a refusal.

**A conversation can be pointed at the wrong folder without being told.** The
branch picker lists every ref, including a ref that is current in another
worktree, and picking one points the conversation at that worktree. From then on
the conversation is split in half: the agent runs in the project's own folder,
while the checkpoints, the diff panel and a revert all read and write the
worktree. The developer sees a diff of a tree the agent never touched, and a
revert restores a tree the agent never edited. Nothing in the UI says so. This
needs no new method to reach — a worktree made by hand at a terminal is enough.

**Deleting a conversation that lives in a worktree offers to tidy up and then
cannot.** The delete flow asks whether to remove the worktree too, and answering
yes calls `vcs.removeWorktree`, which this server refuses. The conversation goes,
the worktree stays, and the developer is shown an error after the fact.

**A conversation that is behind its tracking ref has no way to catch up.**
`vcs.pull` is declared and registered and answers a refusal to anything that
calls it. There is no button for it in the UI today, so this half is parity
rather than a broken flow — but it is the one of the three with everyday value
once something does call it, and it is the cheapest to answer.

## Solution

Answer all three methods, in the shape `crate::refs` already established, and
fix the split-folder bug that made itself visible while measuring them.

**Pull** brings the current ref up to date with its tracking ref, fast-forward
only. It refuses to guess: a detached HEAD and a ref with no tracking ref are
both told what is wrong rather than pulled anyway, and a pull that would need a
merge fails with git's own words instead of inventing a strategy. It reports
whether anything moved, so a UI can distinguish "brought you up to date" from
"you were already".

**Create worktree** and **remove worktree** let a developer manage worktrees
through laplus the way they already manage refs. The developer may name a
location or leave it to the server, and when they leave it the server puts it
under the preferences directory, alongside everything else laplus keeps.

**A conversation that lives in a worktree runs there.** One rule for where a
conversation's work happens — the worktree when it has one, the project's folder
otherwise — applied in both places instead of one, so the agent, the checkpoints
and the diff panel finally agree.

What this does **not** do is give a conversation a worktree. Threads still run
in the project's own folder unless a developer put them somewhere else by hand,
the turn bootstrap still refuses to prepare one, and ADR-0003 still stands. It
gains a note recording that it was reopened, examined and upheld, and naming
what would have to exist first.

## User Stories

1. As a developer whose branch is behind its tracking ref, I want to pull
   without leaving laplus, so that I do not have to find a terminal to do the
   most ordinary thing in git.
2. As a developer who has just pulled, I want to be told whether anything
   actually arrived, so that "nothing happened" and "you were already up to
   date" are not the same silence.
3. As a developer who pulled and got new commits, I want the working tree status
   to reflect them immediately, so that the panel does not keep telling me I am
   behind.
4. As a developer on a detached HEAD, I want the pull to tell me that is why it
   will not run, so that I am not left guessing at a generic git error.
5. As a developer on a ref with no tracking ref, I want to be told to push with
   an upstream first, so that I know the fix is mine to make and what it is.
6. As a developer whose branch has diverged from its tracking ref, I want the
   pull to stop rather than merge or rebase on my behalf, so that laplus never
   makes a history decision I did not ask for.
7. As a developer whose pull failed, I want git's own message, so that I can act
   on the real reason rather than a paraphrase.
8. As a developer whose pull failed, I want my working tree left exactly as it
   was, so that a failed pull is never something I have to clean up after.
9. As a developer who asked to pull in a folder that is not a repository, I want
   the same clear refusal the other ref methods give, so that the whole family
   behaves alike.
10. As a developer, I want to create a worktree for a ref that already exists,
    so that I can have two branches of one project checked out at once.
11. As a developer, I want to create a worktree on a new ref branched from an
    existing one, so that I can start work without disturbing what is checked
    out where I am.
12. As a developer, I want to name the folder a worktree goes in, so that I can
    put it where my own habits expect it.
13. As a developer who does not care where a worktree goes, I want laplus to
    choose, so that I do not have to invent a path for something I will not look
    at directly.
14. As a developer who let laplus choose, I want worktrees kept together in one
    predictable place, so that I can find and clean them up later without
    hunting.
15. As a developer who let laplus choose, I want the folder named after the ref
    it holds, so that I can tell two worktrees apart at a glance.
16. As a developer creating a worktree for a ref whose name has slashes in it, I
    want a single flat folder rather than a nest of directories, so that the
    layout stays predictable.
17. As a developer, I want to be told the path and the ref of the worktree that
    was made, so that I can open a terminal in it without going to look.
18. As a developer who asked for a worktree where one already exists, I want to
    be refused rather than have something overwritten, so that a mistyped path
    cannot cost me work.
19. As a developer who asked to create a worktree on a ref that is already
    current somewhere, I want git's refusal passed through, so that laplus does
    not let me put one branch in two places.
20. As a developer, I want the working tree status to notice a worktree I just
    made, so that the branch picker reflects it without a manual refresh.
21. As a developer, I want to remove a worktree I no longer need, so that stale
    checkouts do not accumulate on disk.
22. As a developer removing a worktree with uncommitted changes in it, I want to
    be refused by default, so that laplus cannot quietly discard work.
23. As a developer who is sure, I want to force the removal, so that a worktree
    full of build output is not permanently undeletable.
24. As a developer removing a path that is not a worktree of this repository, I
    want a clear failure rather than a deleted folder, so that a wrong path is
    survivable.
25. As a developer who removed a worktree, I want the ref it held to survive, so
    that removing a checkout is not the same as deleting a branch.
26. As a developer deleting a conversation that lives in a worktree, I want the
    offer to remove the worktree to actually work, so that saying yes tidies up
    instead of failing after the conversation is already gone.
27. As a developer whose conversation lives in a worktree, I want the agent to
    run in that worktree, so that the folder I pointed the conversation at is
    the folder it edits.
28. As a developer whose conversation lives in a worktree, I want the diff panel
    to show the changes the agent actually made, so that reviewing a turn is
    reviewing the agent's work rather than an unrelated tree.
29. As a developer whose conversation lives in a worktree, I want a revert to
    put back the tree the agent changed, so that undoing a turn cannot write
    over a different checkout.
30. As a developer, I want the terminal a conversation opens to land in the same
    folder the agent is working in, so that what I type and what the agent does
    happen in one place.
31. As a developer with two conversations in one project, I want to know that
    laplus still runs them in the same folder unless I moved one myself, so that
    I am not misled into thinking they are isolated.
32. As a developer reading the codebase later, I want the decision that worktrees
    were reconsidered and deliberately not given to threads recorded, so that I
    do not spend an afternoon re-deciding it.
33. As a developer reading the glossary, I want a worktree to be defined in the
    same vocabulary as a ref, so that "current here, checked out there" has one
    written meaning.
34. As a developer, I want these three methods to fail the same way the ref
    methods do, so that one error shape covers everything git-shaped.

## Implementation Decisions

**Shape.** All three methods follow `crate::refs`: a `read` of the payload that
either yields a typed request or a refusal, and a `run` that takes the shared
working-tree registry from `rpc::Services` and answers a value. No new registry,
no new error type — the error union the contract declares for all three is the
one `crate::git` already builds.

**Home.** The two worktree methods and pull join `crate::refs` rather than
starting a module. They are ref-shaped operations over the same registry, they
share its error union and its `cwd`-rooted payload, and `crate::refs` is already
the file a reader goes to for "what can the developer do to a ref".

**Pull is fast-forward only.** No stash, no rebase, no merge strategy, no
dirty-tree pre-check. A pull that cannot fast-forward fails carrying git's own
stderr. This matches upstream exactly and is a deliberate refusal to make a
history decision on the developer's behalf.

**Pull reports movement by comparing the commit before and after**, not by
parsing git's output. Same commit means `skipped_up_to_date`, a different one
means `pulled`.

**The tracking ref's name has to start being kept.** The porcelain parser in
`crate::git` currently records only _that_ a branch has an upstream, discarding
the name git hands it. The pull result declares that name, so the parser keeps
it. This is a change to an existing parse, not a new read, and everything else
that already knows about tracking refs is unaffected.

**Pull re-reads the status after pulling** to build its answer, so the tracking
ref it reports is the one that is true afterwards rather than the one that was
true before.

**All three disturb the kept working tree.** Pull, create and remove each change
what git would say about a folder, and each is this server doing the changing —
so each marks the kept working tree stale on the way out, exactly as a switch
and an init already do. See ADR-0006 for why this marks rather than reads. A
create additionally disturbs the new worktree's own folder if one is kept for it.
A failure to disturb never fails the operation that succeeded.

**The default worktree location is under the preferences directory:**
`<preferences>/worktrees/<repository folder name>/<ref name with slashes
flattened to dashes>`. This is upstream's layout with laplus's directory
substituted, and it keeps worktrees beside the database, the logs and the
registry rather than scattered next to checkouts. `<preferences>` is already on
`ServerConfig` and already a temp directory under test.

**The three ref inputs on create are two legal shapes, not eight.** A ref alone
checks that ref out into a new worktree. A ref plus a new ref name creates the
new ref at the given one and checks that out. A base ref name is metadata only —
it records a merge-base hint in git config and is ignored entirely without a new
ref name. Nothing else is a valid combination and the read says so.

**Remove passes force through and does not soften it.** Without force, git
refuses a dirty worktree and that refusal reaches the developer. Removing a
worktree never touches the ref it held.

**Neither worktree method validates paths beyond what git does.** Git already
refuses to remove a path that is not a worktree of the repository, and its
message says so better than a pre-check would.

**One rule for where a conversation's work happens.** The expression that
resolves a conversation's folder — its worktree when it has one, the project's
folder otherwise — is stated once and used by both the turn and the review path,
rather than being written in the review path and described in a comment on the
turn path. The comment that currently claims the two agree becomes true.

**Vocabulary.** `CONTEXT.md` gains a **Worktree** entry under `Refs`, defined
against the **Current** entry that already distinguishes a branch that is current
here from one merely checked out elsewhere, and a **Managed worktree** note for
the ones laplus made and where they live. Implementation detail stays in this
spec; the glossary gets the words only.

**ADR-0003 is upheld, not superseded.** It gains a status note recording that
worktrees were reconsidered on the evidence in `.scratch/vcs/`, that these three
methods do not give a thread a worktree, and that the missing prerequisite is
project setup scripts — without them a prepared worktree would start with no
untracked files and nothing that builds. The turn bootstrap keeps refusing.
ADR-0031's treatment of ADR-0020 is the shape to copy.

## Testing Decisions

**A good test here asserts on the developer's outcome, not the call.** For every
operation that changes something, two things are checked, because either alone
passes while the feature is broken: **what happened on disk**, which is whether
the developer's files actually moved, and **what the status panel now says**,
which is whether they can tell. This is `socket_branches.rs`'s doctrine
verbatim and it is the prior art for all of the below.

**The seam is the socket, against real repositories.** A repository is built
with the `git` binary and then acted on by sending the requests the UI sends.
Nothing reaches into `crate::refs` or `crate::git` directly. This is one seam,
already in use, and it is deliberately stricter than upstream — upstream tests
worktree behaviour against the driver and tests the socket against a stubbed
driver, so no single upstream test covers both halves at once.

**Modules tested:** `crate::refs` for all three methods, `crate::git` for the
tracking-ref name, and the turn path for the folder rule.

**Pull needs a remote, and it is a local one.** No test in this tree has set one
up before. A bare repository in a second temporary directory serves as the
origin; a clone of it pushes a divergent commit so there is something to pull.
No network, so this stays as hermetic as everything else in the suite. The
helper belongs in the workspace harness beside `init_repository` and `commit`,
because it is fixture-building rather than a new boundary. Upstream builds its
remote fixtures exactly this way.

**Pull cases:** a fast-forward that arrives and says `pulled`; an already-current
branch that says `skipped_up_to_date` and leaves the commit alone; a detached
HEAD refused by name; a branch with no tracking ref refused by name; a diverged
branch refused with git's own message and its working tree unchanged afterwards;
a folder that is not a repository refused as the ref methods refuse it; and the
tracking ref reported in the result matching what the status panel reports.

**Worktree cases:** creating on an existing ref and finding that ref current in
the new folder; creating a new ref from an existing one and finding both the
folder and the new ref; letting the server choose the location and finding it
under the preferences directory with slashes flattened; a ref name with a slash
producing one folder rather than nested ones; creating where something already
exists refused; creating on a ref already current elsewhere refused; removing and
finding the folder gone and the ref still listed; removing a dirty worktree
refused without force and succeeding with it; removing a path that is not a
worktree refused; and the branch picker reflecting a created and a removed
worktree without a manual refresh.

**The folder rule is tested where it is visible**, not by inspecting which
variable was passed: a conversation given a worktree path runs its agent there,
and the checkpoint taken for its turn records that same tree. The absence of a
test like this is why the two halves drifted apart, so it is the test that keeps
them together.

**The parser change is tested where parsers are tested here** — in `crate::git`'s
own module tests, as an assertion added to the existing tracking-branch case
rather than a new seam.

**Two suite failures are pre-existing on Linux** and fail on a clean tree:
`watcher::tests::a_file_written_outside_the_server_is_reported_relative_to_its_workspace`
and `a_call_that_names_no_size_does_not_resize_the_terminal`. Both are PTY and
filesystem environment sensitivity; CI's authoritative platform is Windows. Do
not chase them and do not let them mask a real regression.

## Out of Scope

- **Giving a thread its own worktree.** The turn bootstrap keeps refusing
  `prepareWorktree`, the composer's worktree mode keeps failing, and ADR-0003
  keeps standing. This is the decision the spec examined and declined.
- **Project setup scripts.** Upstream runs a script marked to run on worktree
  creation into each new worktree, which is what makes a fresh worktree usable.
  laplus answers an empty script list unconditionally and this spec does not
  change that. It is the named prerequisite for the item above, and it is a file
  format, a loader and a runner — its own effort.
- **Fetching, and resolving a remote tracking commit.** Upstream's bootstrap
  needs both for its start-from-origin option. Nothing in these three methods
  does. `newWorktreesStartFromOrigin` stays a stored setting nothing acts on.
- **Removing the branch a worktree held.** Remove takes a path and removes a
  checkout.
- **Pruning stale worktree administrative files.** No `git worktree prune`, in
  either method; upstream does not either.
- **Cleaning up worktrees laplus made.** Nothing garbage-collects
  `<preferences>/worktrees`. A developer removes what they created, which is
  what the remove method is for.
- **A pull button.** No UI calls `vcs.pull` today and this spec does not add
  one. The method is answered so that a client which calls it is answered.
- **Merge, rebase, stash and push.** None are declared by these three methods.
- **The stacked-action and pull-request surface** upstream carries alongside
  these methods. Deliberately absent from this fork; see the ledger.

## Further Notes

**Only one of the three has a caller today.** `vcs.removeWorktree` is called by
the delete-conversation flow. `vcs.createWorktree` and `vcs.pull` are registered
in the client runtime and called by nothing in the UI. The ledger's call-site
count for this cluster came from grepping the method-name table, which catches
that registration rather than a caller — the ledger warns about this and for this
cluster the warning bites. Sizing this work by "three live flows" would be wrong.

**The split-folder bug is the most damaging thing here and the least expected.**
It was found while measuring whether the delete flow was reachable at all. It
predates all three methods, needs none of them to reproduce, and is the reason
the folder rule is in scope rather than deferred with the rest of the worktree
work. If this spec has to be cut down, that fix is the part to keep.

**Parity.** This takes the ledger from 35 of 60 to 38 of 60. Re-derive rather
than quoting that figure; the ledger says so about itself and has been stale
once.

**The suite is not evidence the app works.** `server/tools/ui-driver/` against a
running laplus is the other half, and it is outstanding for
`capabilities.connectionProbe` already. The delete-conversation flow is the one
piece of this work with a real UI path, so it is the one worth driving.
