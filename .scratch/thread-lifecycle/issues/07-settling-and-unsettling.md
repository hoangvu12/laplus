# 07 — Settling and unsettling

**What to build:** the developer can settle a finished conversation so it leaves
the inbox and stops competing for attention with work that needs them, and can
unsettle one to pin it back when they decide it is not finished after all.

**A naming warning, first.** `CONTEXT.md` already defines **settling** as reading
a session status as a turn state — a property of a _turn_, owned by its own
module, with the rule mirrored in three places in this repository. This ticket
uses the same English word for something different: whether a _thread_ belongs in
the developer's inbox. The existing meaning has seniority. Add the thread-level
concept to the glossary as **inbox state**, with a cross-reference warning at
**settling**, and keep the contract's field names as they are — renaming contract
vocabulary is not on the table, so it is the prose and the Rust identifiers that
have to disambiguate. This collision is the one candidate for a new ADR in the
whole effort.

**The server does not classify.** Which threads _count_ as settled is already
computed by the bundled client runtime, which ships unmodified. This ticket
stores the override, enforces the invariants, and emits the events. It does not
decide what the inbox shows.

**The invariants are not optional.** The client has its own copy of them, and
that copy exists explicitly so the interface can refuse before a round trip — the
server's are the authoritative ones. Anything the client refuses to _classify_ as
settled must also be refused as a settle _target_, which is why the list below
matches the client's rule exactly rather than approximately.

The unanswered-request guard is not new logic. The work log module already derives
unanswered approvals and unanswered questions, and is already the source of the
two pending flags on the shell summary. Deriving it rather than counting is what
makes it survive a restart.

**Repeats re-emit rather than refuse.** Settling something already settled lands
on the same state, and the re-emission carries the _existing_ updated-at rather
than the current time, so a double-click neither rewinds the thread nor moves it
in a list ordered by when things changed.

The two directions are not symmetrical. A **user** unsettle pins the thread
active. The neutral reset — returning an overridden thread to no override at all
— is server-decided and is ticket 08; the contract lets a client send only the
user reason, so the neutral one cannot be forged.

**Blocked by:** 01 — Lifecycle fields reach the client as stored state.
06 — Archiving and unarchiving (settle refuses an archived thread, and at the
socket seam the archive command is the only way to make one).

**Status:** ready-for-agent

- [ ] Both commands are parsed before the world is consulted; blank identifiers
      are refused.
- [ ] An unknown thread is refused.
- [ ] Settling an archived thread is refused.
- [ ] Settling a thread whose session is starting or running is refused.
- [ ] Settling a thread with an unanswered approval is refused.
- [ ] Settling a thread with an unanswered question is refused.
- [ ] Settling a thread whose turn has been requested but not yet adopted by a
      session is refused, within the same adoption grace the client uses.
- [ ] Unsettling an archived thread is refused.
- [ ] Every refusal carries a sentence naming both what went wrong and the thread
      it went wrong about, because the dispatch error carries nothing else
      machine-readable and the interface renders the sentence verbatim.
- [ ] A settle records both the override and the time it settled at.
- [ ] A user unsettle pins the thread active rather than clearing the override to
      neutral.
- [ ] Settling twice is harmless and does not change the thread's updated-at.
- [ ] Unsettling twice is harmless and does not change the thread's updated-at.
- [ ] Both changes publish on the thread's own feed and reach the project list.
- [ ] A subscriber on a second connection sees both.
- [ ] Inbox state survives a restart, and a fresh subscriber agrees with a
      subscriber that watched it happen.
