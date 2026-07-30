# 10 — Deleting a thread

**What to build:** the developer can delete a thread, so a conversation started by
mistake stops taking up space in their list — and a deletion they regret has not
destroyed anything.

**Deleting is soft.** The deletion time is stamped and the row is kept, along with
its transcript, its work log and its checkpoints. Three reasons, and none of them
is squeamishness:

- The checkpoint refs a turn wrote are real git objects in the developer's own
  repository. A hard delete would orphan them.
- The thread table cascades. Removing the row takes the transcript and the work
  log with it, irreversibly, in one statement.
- The contract carries a deletion time on the thread. That field is only
  meaningful if the thread survives to carry it.

A deleted thread leaves the project list and refuses further commands. A stale
client must not go on driving a conversation the developer removed.

**One behaviour to confirm rather than assume.** Whether a deleted thread must
also be withheld from the archived shell snapshot is a client-visible detail that
was not settled during the audit. Check it against the client's own reducer before
choosing, rather than picking whichever seems tidier.

Note this ticket does **not** depend on archive. Delete has no archived condition
in its invariants — what it needs is the deletion field from ticket 01, and
nothing more.

**Blocked by:** 01 — Lifecycle fields reach the client as stored state.

**Status:** ready-for-agent

- [ ] The command is parsed before the world is consulted; a blank identifier is
      refused.
- [ ] An unknown thread is refused.
- [ ] Deleting a thread that is already deleted is refused.
- [ ] The command answers with the sequence it committed at.
- [ ] The deletion time is recorded and the row is kept.
- [ ] The transcript, the work log and the checkpoint rows all survive.
- [ ] The checkpoint refs in the developer's repository are left alone.
- [ ] A deleted thread stops appearing in the project list.
- [ ] Commands dispatched against a deleted thread are refused.
- [ ] A subscription to a deleted thread is refused, unless the client says it
      already holds the conversation — the existing resume rule is unchanged by
      this ticket.
- [ ] Whether a deleted thread appears in the archived shell snapshot is decided
      against the client's reducer, and the choice is written down in the ticket's
      comments.
- [ ] The change publishes on the thread's own feed and reaches the project list.
- [ ] A subscriber on a second connection sees it.
- [ ] Deletion survives a restart: the thread does not come back on the list.
