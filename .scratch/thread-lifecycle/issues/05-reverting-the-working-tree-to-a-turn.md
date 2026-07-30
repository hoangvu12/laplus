# 05 — Reverting the working tree to a turn

**What to build:** the developer picks a turn and puts the working tree back to
how it looked before that turn ran — files the agent created, modified and
deleted alike — without reading the diff and unpicking it by hand.

This is the gap that prompted the whole audit. The revert control dispatches a
command the server refuses, so a developer can see precisely what a turn changed
and has no way to undo it.

It is much smaller than it sounds, because the hard part is already built. A turn
is a photograph of the working tree (ADR-0008), and the checkpoint machinery
already takes one at every turn boundary — tracked, staged and untracked files
together — under a ref of its own, and already serves both the per-turn diff and
the whole-conversation diff from those photographs. A revert is a restore of a
photograph this server already took.

**Answer in two stages.** The dispatch records that a revert was asked for and
answers immediately; the restore itself is deferred, because it touches a disk and
the socket's only reader must never wait on one. Completion is published as its
own event. The contract already declares both the request and the completion, and
the client already folds them.

A revert names a turn. A turn the thread has no checkpoint for is a revert with
nothing behind it, and is refused rather than attempted — the same reasoning that
keeps a checkpoint row from being published before its tree has been written.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] The command is parsed before the world is consulted; a blank identifier is
      refused.
- [x] An unknown thread is refused.
- [x] A turn the thread has no checkpoint for is refused, with a sentence naming
      the turn.
- [x] The dispatch answers with a sequence without waiting for the restore.
- [x] The restore runs off the read loop, so a large repository does not stall
      the connection or any other subscriber on it.
- [x] Files the turn modified are returned to their earlier contents.
- [x] Files the turn created are removed.
- [x] Files the turn deleted are restored.
- [x] A file the turn left untracked is handled the same way the checkpoint
      recorded it, so the restore covers the whole tree rather than only what git
      was already tracking.
- [x] Completion is published as its own event on the thread's feed, after the
      tree has actually been written and never before.
- [x] A restore that fails is reported as a failure rather than as a completion,
      so a failed revert is never mistaken for a finished one.
- [x] Reverting to the turn the tree already matches is harmless.
- [x] The thread, its transcript and its work log are untouched — a revert moves
      the working tree, not the conversation.

## What it turned out to be

Three git commands: a scratch index seeded from the photograph (`read-tree
<checkpoint>`), the project's own folder restated into it from the tree as it is
now (`add -A -- .`), then `read-tree -m -u <checkpoint>`. The four file criteria
above are not implemented anywhere — they fall out of the two-way merge, because
a file the turn created is in the index and not in the photograph, a file it
deleted is in the photograph and not in the index, and a file it left untracked
is in both only because `add -A` put it there on each side. `read-tree -m` also
writes only what differs, so a revert on a large repository does not restat every
file in it or wake the watcher for one.

`crate::checkpoints::restore`, `crate::threads::Change::{RevertRequested,
Reverted}`, and `Shell::revert_checkpoint`. No migration, no new read-model shape.

## What was decided along the way

**A revert is seeded from the photograph, and a capture from `HEAD`.** The
obvious symmetry — stage the same way for both — is wrong, and the case that
proves it is a project that is one package inside a repository. A capture seeds
from `HEAD` because the tree it writes has to describe the whole repository for a
later diff to run over. If a revert did the same, then a commit the developer
made _anywhere else in that repository_ since the checkpoint would be a
difference the merge had to resolve, and `read-tree -m` aborts rather than
picking a side: `error: Entry 'other/sib.txt' not uptodate. Cannot merge.`, and
no revert at all. Seeded from the photograph, everything above the project's own
folder starts out already agreeing, so there is nothing to resolve and nothing to
write. `a_project_inside_a_repository_reverts_itself_and_nothing_above_it` is
that case; it hangs and then fails on the read timeout if the seed is changed
back.

**Turn zero is an ordinary target and is the common one.** The panel reverts a
user message by asking for `max(0, n - 1)` (`ChatView.tsx`), so undoing the first
turn of a conversation names the baseline — a range check starting at one would
have refused exactly the revert the control is most used for. A conversation that
has recorded _no_ turn is still refused, because until one has finished nothing
has promised the baseline was written.

**Neither event reaches the project list.** The list renders a conversation's
title, its session and its latest turn, and a revert moves a working tree rather
than any of the three — the same reading that keeps a checkpoint off it.
`a_revert_does_not_reach_the_project_list` pins it by driving a rename afterwards
and showing that is the next thing the list hears.

**Whether the project folder is still there is not decided on the read loop.** It
is a disk. A folder that has been moved is reported the way a `git` that refused
is: a `revert.failed` row on the conversation, after the command was accepted.

**The failure channel is the work log**, following `checkpoint.failed` — the
contract declares no failure event for a revert, and the developer is looking at
the conversation.

## The one divergence, and it is worth a look

**The client's reducer trims the conversation on `thread.reverted` and this
server does not.** `threadReducer.ts` drops the messages, checkpoints, proposed
plans and activities after the turn reverted to, and moves `latestTurn` back. The
last criterion above says the opposite — the thread, its transcript and its work
log are untouched — so a window that watched a revert shows a shorter
conversation than the same window after a reload.

The criterion was followed, because trimming a transcript is a deletion with its
own cascade, its own store work and its own view about whether the messages are
recoverable, and none of that is "a revert moves the working tree". But the
disagreement is user-visible and this repository treats a server that folds
differently from its client as a defect elsewhere, so it wants a ticket of its
own: either the server trims to match, or the client is asked to stop.

## Not done

**The window has not been driven.** Every criterion above is asserted through the
socket, and `AGENTS.md` is right that a green suite is not evidence the
application works — the revert control, its confirmation dialog and what the
timeline looks like afterwards are unchecked, and the divergence above is exactly
the kind of thing `tools/ui-driver/` would have shown in a minute.

**"No checkpoint for that turn" is answered from the registry, not from the
disk.** The count is compared against the conversation's own highest recorded
turn, because the check is on the read loop and a `git rev-parse` there is a disk
this command must not wait on. A ref that was pruned or garbage-collected
therefore passes the check, is answered with a sequence, and fails afterwards as
a `revert.failed` — which is exactly what
`a_restore_that_fails_is_reported_as_a_failure_rather_than_a_completion` drives.
Correct as far as the developer is concerned, and worth knowing before anyone
reads the refusal as a promise the tree is there.

**A revert is not refused while a turn is running.** Only `ChatView.tsx` stops
the developer asking for one, so a client that did not could restore the tree
under a working agent, and the checkpoint taken at the end of that turn would
photograph the result. Not in this ticket's list and not in the spec's invariant
table; it belongs with the session-state guards the settle and snooze commands
bring in.
