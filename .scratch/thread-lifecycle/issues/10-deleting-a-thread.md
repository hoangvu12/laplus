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

> **Confirmed while building ticket 06, and the answer is "withhold it".** The
> archived section of the settings panel filters on neither `deletedAt` nor
> `archivedAt` — `SettingsPanels.tsx`'s `archivedGroups` takes
> `snapshot.threads` whole and groups it by project. So a thread that was
> archived and then deleted renders in the panel, with an unarchive control on
> it, unless this server leaves it out of the answer. Ticket 06 built the filter
> as `crate::threads::Shelf`, which is where the extra condition goes:
> `Shelf::Archived` means archived **and not deleted**, and `Shelf::Working`
> already has to exclude deleted threads for the criterion below.

Note this ticket does **not** depend on archive. Delete has no archived condition
in its invariants — what it needs is the deletion field from ticket 01, and
nothing more.

**Blocked by:** 01 — Lifecycle fields reach the client as stored state.

**Status:** done

- [x] The command is parsed before the world is consulted; a blank identifier is
      refused.
- [x] An unknown thread is refused.
- [x] Deleting a thread that is already deleted is refused.
- [x] The command answers with the sequence it committed at.
- [x] The deletion time is recorded and the row is kept.
- [x] The transcript, the work log and the checkpoint rows all survive.
- [x] The checkpoint refs in the developer's repository are left alone.
- [x] A deleted thread stops appearing in the project list.
- [x] Commands dispatched against a deleted thread are refused.
- [x] A subscription to a deleted thread is refused, unless the client says it
      already holds the conversation — the existing resume rule is unchanged by
      this ticket.
- [x] Whether a deleted thread appears in the archived shell snapshot is decided
      against the client's reducer, and the choice is written down in the ticket's
      comments.
- [x] The change publishes on the thread's own feed and reaches the project list.
- [x] A subscriber on a second connection sees it.
- [x] Deletion survives a restart: the thread does not come back on the list.

## What it turned out to be

One change in the vocabulary (`Change::Deleted`), one parse arm, `Shell::delete`,
and — as the note predicted — no migration and no store change, because ticket 01
added the column and every change already writes the whole thread row. Two things
were not that.

**The project list had to be told a `thread-removed`, not a summary.** Every other
lifecycle change publishes the conversation's shell summary and lets the client
filter: the sidebar drops an archived conversation because it reads
`archivedAt === null` on it. That road is closed here —
`OrchestrationThreadShell` does not declare `deletedAt` at all, and a
`Schema.Struct` drops a key it does not name, so the field would not survive the
client's decode of the summary even though this server puts it there. The
`thread-removed` the shell reducer already handles is the vocabulary that exists
for this, and `project.delete` already publishes one per conversation.

That turned `Change::reaches_the_shell` — a bool — into
`Change::on_the_list(thread)`, answering `Summary`, `Removal` or nothing. The
third case is the one worth having: **deleting does not stop a session**, so an
agent still winding down behind a deleted conversation would have published a
`thread.session-set`, and a summary for that would have upserted the conversation
straight back onto the list the removal had just taken it off.

**The refusal of later commands is one guard, not nineteen.** `Shell::dispatch`
asks `Command::over_a_living_thread` of every parsed command and refuses the ones
naming a deleted conversation, before the world is consulted. Nineteen dispatch
arms would have been nineteen places for the twentieth to be forgotten — and the
one command deliberately left out is `thread.delete` itself, because "already
deleted?" is a question about the field the change is about to move and belongs
under the fold's own lock, exactly as `Shell::set_archived` argues for a second
archive.

## What was decided along the way

**The archived-snapshot question, confirmed and built.** The note's answer stood:
`SettingsPanels.tsx`'s `archivedGroups` filters on neither field, so
`Shelf::holds` now answers `false` for a deleted conversation on *either* shelf.
It is one condition in the one place both snapshots are built from, so the
project list and the archived answer cannot disagree about it.

**A deletion is refused a fresh read on both doors, not just the socket.** The
criterion named the subscription; the HTTP route
`GET /api/orchestration/threads/{threadId}` was not mentioned and is refused too,
because of how the two are used *together*. The client fetches that snapshot,
folds it, and then subscribes **with a cursor** precisely because it now holds the
conversation (`client-runtime/src/state/threads.ts`). So a route that answered
would have seeded a pane with the deleted conversation and then resumed past the
`thread.deleted` that would have told it — a window with no way left to learn.
Refusing one door and not the other would have been the criterion met and the
reason for it missed.

**The resume rule is untouched, and it is the only way to read what was kept.** A
client that says it already holds the conversation still opens, and is handed a
snapshot stamped `deletedAt` rather than a refusal — which is what
`socket_deleting.rs` reads the surviving transcript, work log and checkpoints
back through. On the socket the two refusals carry different sentences ("was
deleted" against "was not found"), because a draft that is about to exist and a
conversation that has been removed are the same 250ms retry loop to the client and
only the sentence tells them apart in a log. **The HTTP route gives one answer for
both**, and that is the contract's doing rather than an oversight: its refusal
carries no message at all, and its `reason` is a literal type one member wide
(`thread_not_found`), so there is nowhere for a second sentence to go.

**The checkpoint refs are asserted from the developer's repository**, with
`git for-each-ref refs/laplus` before and after, rather than from the
conversation's checkpoint rows. The rows are the registry's own memory; the refs
are the git objects a hard delete would have orphaned, and they are the thing the
softness was chosen for.

**Ticket 01's suite curated all six fields, and one of them now means
something.** `socket_lifecycle_fields.rs` wrote a `deletedAt` into the row and
then asked both feeds about it — a conversation that is now unreachable on both
by design. Its curated lifecycle leaves that field `null`, with the key still
asserted present on both renderings; the column's round trip moved to
`socket_deleting.rs::a_deletion_survives_a_restart`, which carries it across a
restart the way the command does.

**`thread.delete` was the last command in the contract's dispatchable union this
server did not answer.** The unit test that used it as the example of an
unimplemented command now uses `thread.session.set` — a command the contract
declares for the server rather than for a client.

## Not done

**The window has not been driven**, at the requester's standing instruction and as
with tickets 03, 05, 06, 07 and 09. Every criterion above is asserted through the
socket in `tests/socket_deleting.rs` or at the two unit seams; the sidebar's
delete control, its confirmation dialog and the batch delete are unchecked. The
one adjacent thing that *has* been driven is `thread.session.stop`, in ticket 04,
through this very flow — the client stops the session and then sends this command.

**Nothing stops the agent.** A session running behind a deleted conversation goes
on until it ends by itself, writing to a transcript nobody is watching. That is
where upstream leaves it too: `useThreadActions.ts` sends `thread.session.stop`
first, so the sequencing is the client's, and a delete that interrupted a turn
would be a deletion that also stopped work.

**There is no undelete.** The contract has no command for one, so the surviving
row is a recovery path for whoever opens the database rather than a state a client
can reach. A deleted conversation cannot be archived, settled or renamed back into
view either — every one of those is refused.

**The two diff methods still answer for a deleted conversation.**
`orchestration.getFullThreadDiff` and `getTurnDiff` are reads of git refs that the
softness deliberately keeps, and they are keyed by thread id, so a client holding
one could still ask. Left alone rather than closed: the diff panel opens from a
thread pane, and a thread pane cannot be opened for a deleted conversation, so
this is unreachable from the interface. Recorded here because it is the one door
of the four that was not shut, and shutting it is a line in each method if a
reason to turns up.

**The activity resets are not asserted at the socket.** That a deleted
conversation is not woken by its own winding-down agent is
`crate::threads`'s unit test, for ticket 07's reason: through this server's own
dispatch a turn request resets the override first, so the later two triggers can
never find anything left to reset from out there.
