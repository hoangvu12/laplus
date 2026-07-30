# 06 — Archiving and unarchiving, and reaching archived threads

**What to build:** the developer can archive a thread and watch it leave the
project list, unarchive it and get it back intact, and browse what they have
archived without it cluttering the list they work from.

This is the first slice that lets the inbox actually be cleared, and the first
that makes ticket 01's fields mean something.

Archiving is not deleting. The thread, its transcript, its work log and its
checkpoints all stay exactly as they were; what changes is that it stops
appearing in the list of work the developer is doing.

Because the project list excludes archived threads, reaching them needs the
archived shell snapshot the contract declares and this server does not answer.
Build it with **the same snapshot builder** the live subscription and the HTTP
snapshot endpoint already share, filtered to archived threads. A second builder
would let the world the client draws depend on which transport answered first,
which is the trap the shared builder exists to avoid.

**Blocked by:** 01 — Lifecycle fields reach the client as stored state.

**Status:** done

- [x] Both commands are parsed before the world is consulted; blank identifiers
      are refused.
- [x] An unknown thread is refused.
- [x] Archiving a thread that is already archived is refused, with a sentence
      naming it.
- [x] Unarchiving a thread that is not archived is refused.
- [x] Each command answers with the sequence it committed at.
- [x] An archived thread stops appearing in the project list.
- [x] An unarchived thread reappears in the project list with its transcript,
      work log and checkpoints intact.
- [x] The archived shell snapshot answers with the archived threads, and is built
      by the same builder the live subscription and the HTTP snapshot use.
- [x] Both changes publish on the thread's own feed and reach the project list.
- [x] A subscriber on a second connection sees both.
- [x] Archive state survives a restart, and a fresh subscriber agrees with what a
      subscriber that watched it happen holds.

## What it turned out to be

`crate::threads::Shelf` — two variants, one predicate and the change that puts a
conversation on it — plus the two changes, the two parse arms,
`Shell::set_archived`, and `orchestration.getArchivedShellSnapshot` routed to
`Shell::archived_shell_snapshot`. No migration: ticket 01 added the column, and
this is the command that writes it. The store needed nothing at all, because
every change already writes the whole thread row.

`Shelf` is carried rather than a boolean, and that is what keeps the filter and
the refusal one rule: `Shelf::holds` decides both which snapshot a conversation
is in and whether a move would move anything. The two snapshots are one builder
called twice — `the_two_snapshots_are_one_snapshot_filtered_two_ways` asserts that
observably rather than by reading the code, by comparing a conversation's summary
field by field either side of the move.

`Threads::latest_change` takes the shelf too, so a snapshot's `updatedAt`
describes the snapshot it is on. Without it an empty archive would report a
change to a list that has never had anything on it, which
`an_empty_archive_describes_itself_from_the_registry` pins.

## What was decided along the way

**A repeat is refused, and it needed a new seam to be refused honestly.** Every
other command on a thread goes through `Threads::apply`, which cannot refuse: it
folds and publishes. "Already archived" is a question about the very field the
change is about to move, so asking it before `apply` and answering inside it
would let two windows both be told they archived one conversation. `apply` is
now a call to `Threads::apply_unless`, which asks a guard under the same lock the
fold runs under and before a sequence is taken. `Shell::set_mode` and
`Shell::update_thread_meta` are unchanged and still answer a repeat, which is
right for them: they write a value the developer chose, and this is a move
between two lists.

**The archived snapshot carries every project, not the archived ones.** The
settings panel groups archived threads by project and looks each one up in this
same answer (`SettingsPanels.tsx`), so a project list filtered alongside the
threads would silently drop the threads whose project had nothing else archived.

**The project list still hears about an archive.** `thread-upserted` carrying the
stamp, rather than `thread-removed`: the client's shell reducer upserts by id and
every view filters on `archivedAt === null` (`Sidebar.tsx`, `CommandPalette.tsx`,
`SidebarV2.tsx`), so the summary with the stamp on it is what makes the sidebar
drop it. A `thread-removed` would also be the event a _deleted_ project's threads
publish, which is a different thing.

**Nothing is told to stop.** Archiving a conversation with a live agent leaves
the agent alone — the same posture snooze will need. The only fields either
command moves are `archivedAt` and `updatedAt`.

**A registry that cannot be read refuses under the method's own union.** The
panel renders "Failed to load archived threads" from a refusal
(`archivedThreads.ts`) and would tear down the socket on a defect, so the failure
is an `OrchestrationGetSnapshotError`.

## Not done

**The window has not been driven**, as with tickets 03 and 05 and at the same
instruction. Every criterion above is asserted through the socket in
`tests/socket_archiving.rs`; the context menu, the settings panel's archived
section and its unarchive control are unchecked.

**A deleted thread is not withheld from the archived answer.** Nothing stamps
`deletedAt` yet, so there is no deleted thread to withhold — but the spec's "one
behaviour to confirm rather than assume" _was_ confirmed while building this, and
the answer is written onto ticket 10 rather than left to be re-derived:
`SettingsPanels.tsx` filters the archived list on neither field, so a deleted
thread would render there with an unarchive control on it. `Shelf` is where the
condition goes when ticket 10 lands.
