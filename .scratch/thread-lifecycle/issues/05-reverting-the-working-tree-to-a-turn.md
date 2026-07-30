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

**Status:** ready-for-agent

- [ ] The command is parsed before the world is consulted; a blank identifier is
      refused.
- [ ] An unknown thread is refused.
- [ ] A turn the thread has no checkpoint for is refused, with a sentence naming
      the turn.
- [ ] The dispatch answers with a sequence without waiting for the restore.
- [ ] The restore runs off the read loop, so a large repository does not stall
      the connection or any other subscriber on it.
- [ ] Files the turn modified are returned to their earlier contents.
- [ ] Files the turn created are removed.
- [ ] Files the turn deleted are restored.
- [ ] A file the turn left untracked is handled the same way the checkpoint
      recorded it, so the restore covers the whole tree rather than only what git
      was already tracking.
- [ ] Completion is published as its own event on the thread's feed, after the
      tree has actually been written and never before.
- [ ] A restore that fails is reported as a failure rather than as a completion,
      so a failed revert is never mistaken for a finished one.
- [ ] Reverting to the turn the tree already matches is harmless.
- [ ] The thread, its transcript and its work log are untouched — a revert moves
      the working tree, not the conversation.
