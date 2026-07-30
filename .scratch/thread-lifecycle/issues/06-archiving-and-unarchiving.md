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

**Status:** ready-for-agent

- [ ] Both commands are parsed before the world is consulted; blank identifiers
      are refused.
- [ ] An unknown thread is refused.
- [ ] Archiving a thread that is already archived is refused, with a sentence
      naming it.
- [ ] Unarchiving a thread that is not archived is refused.
- [ ] Each command answers with the sequence it committed at.
- [ ] An archived thread stops appearing in the project list.
- [ ] An unarchived thread reappears in the project list with its transcript,
      work log and checkpoints intact.
- [ ] The archived shell snapshot answers with the archived threads, and is built
      by the same builder the live subscription and the HTTP snapshot use.
- [ ] Both changes publish on the thread's own feed and reach the project list.
- [ ] A subscriber on a second connection sees both.
- [ ] Archive state survives a restart, and a fresh subscriber agrees with what a
      subscriber that watched it happen holds.
