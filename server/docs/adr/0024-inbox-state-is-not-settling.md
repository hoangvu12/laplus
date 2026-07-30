# ADR-0024 — Inbox state is not settling, and the prose moves rather than the fields

Date: 2026-07-30
Status: Accepted

## Context

This crate has meant one thing by **settling** since ADR-0001. `crate::settling`
reads an `OrchestrationSessionStatus` as an `OrchestrationLatestTurn["state"]` —
how a _turn_ went — and that ADR is why it is a module rather than five `match`es:
upstream writes the rule down twice and laplus is the third copy, so its
correctness is agreement rather than opinion. `CONTEXT.md` has carried the entry
since, and `Threads::fold` calls the function `settle`.

The thread-lifecycle spec then brought a second meaning through the door. Its
commands are `thread.settle` and `thread.unsettle`, its fields are
`settledOverride` and `settledAt`, its events are `thread.settled` and
`thread.unsettled` — and every one of them is about whether a _thread_ belongs in
the developer's inbox. Nothing about a turn. The two concepts share an English
word, a Rust module namespace and a set of identifiers, and they are unrelated:
a turn settles when the agent stops, and a thread settles when the developer
decides they are finished with it.

The collision is not avoidable by choosing better names, because half the names
are not ours. `packages/contracts` is upstream's, it is schema-only, and the
field names are what a client decodes; ADR-0018 is why there are no more syncs
but not a licence to rewrite the vocabulary the shipped UI speaks. Renaming
`settledOverride` would mean forking the contract and the client runtime that
reads it.

## Decision

**The turn meaning keeps the word; the thread meaning gets a different one.**

- **Settling** stays what `crate::settling` owns and what `CONTEXT.md` has always
  said it is, with a cross-reference warning added at the entry.
- **Inbox state** is the glossary name for the thread-level concept — the six
  contract fields as `crate::threads::Lifecycle`, and the commands that move
  them. Every doc comment, refusal sentence and test name for this work says
  "inbox" or "settle a conversation", never "settling".
- **The contract's field names do not move.** `settledOverride`, `settledAt`,
  `thread.settled`, `thread.unsettled` and the two command types are spelled
  exactly as the client sends and decodes them.
- **The Rust identifiers around them disambiguate.** `Lifecycle`, `Shelf`,
  `Busy`, `Adoption`, `Shell::settle` — a thread noun or a command name, and
  never a second `settling`.

Seniority decides which meaning yields: the turn one is older here, is owned by a
module, is mirrored from two upstream copies, and is read on every session change
this server publishes.

## Considered options

- **Rename the contract fields.** Fork `packages/contracts` and
  `@t3tools/client-runtime` to say `inboxState`. Rejected: it is a fork of the
  vocabulary the UI is written in, against ADR-0012's reading that the client is
  ordinary work we maintain rather than something to diverge from for taste.
- **Rename `crate::settling`.** Call the turn rule `turn_outcome` and let the new
  commands have the word. Rejected: three documents, an ADR and every session
  path in the crate use it, and the rename buys the newer meaning a word it does
  not need — the concept it names in the interface is "inbox", not "settled".
- **Let both use the word and rely on context.** Rejected because the two meet:
  `Threads::fold` calls `settle(thread.latest_turn, session)` a few lines from
  the arm that writes `settled_override`. One of those two is about the turn and
  one is about the thread, and a reader with only the word to go on has no way to
  tell which.
- **Prefix the new one, `thread_settled`.** Better than nothing and still a
  homonym: it reads as "the thread's copy of the turn rule", which is precisely
  the wrong thing.

## Consequences

- **A grep for `settl` finds two subjects.** Unavoidable, because the contract
  supplies half the hits. The glossary entry is the disambiguator and both
  entries cross-reference each other.
- **The refusal sentences say "inbox" out loud.** `Conversation 'x' is archived,
so it is already out of the inbox` names the concept rather than the field, and
  the interface renders those sentences verbatim.
- **A future reader adding to `crate::settling` gets a warning at the door.** Its
  module doc, `Lifecycle`'s doc and `CONTEXT.md` each say which of the two they
  are, in the place a reader arrives.
- **This costs nothing to reverse if the contract ever renames the fields.** The
  decision is about prose and Rust identifiers, and the day `settledOverride`
  becomes `inboxState` upstream, the glossary entry loses its warning and nothing
  else changes.
