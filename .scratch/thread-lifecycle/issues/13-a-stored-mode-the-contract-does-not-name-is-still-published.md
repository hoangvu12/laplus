# 13 — A stored mode the contract does not name is still published

**What to build:** a decision about the *read* side of the mode vocabulary, and
then whatever it implies.

Ticket 12 closed the doors. `named_by_the_contract` now guards every command that
writes a runtime or interaction mode — the two pickers' own commands,
`thread.turn.start`'s per-turn override, and `thread.create` — so nothing this
server accepts from here on can put an unnameable mode on a thread. `CONTEXT.md`
records that promise in both glossaries.

It says nothing about what the store may already hold, and ticket 12 left that
deliberately: "Closing that is a read-side question (refuse, or round on the way
out, or a migration) and it is a different decision from this one."

## What happens

`crate::store`'s thread reader takes both columns straight off the row:

```rust
runtime_mode: row.get(4)?,
interaction_mode: row.get(5)?,
```

So a row holding `bypassPermissions` is read back, put on the thread, and
published in every `thread.runtime-mode-set`, every `thread.turn-start-requested`
payload, every thread snapshot and every project-list summary. The contract types
the field as a closed `RuntimeMode` union, so the client's decode of the whole
thread payload fails on a literal it does not know — the cost is not a wrong
badge, it is a conversation the UI cannot draw at all. That is ticket 12's own
framing of the cost, and it is unchanged by the doors being shut.

**The rows are possible rather than hypothetical**, which is the whole reason
this is a ticket. The doors were open from ticket 10 until ticket 12, and the
composer sends the per-turn override on *every* send — so the unguarded door was
the one almost every mode change went through, for the whole of that window.

## The three answers, and what each costs

- **Refuse on read.** The row does not become a thread. Honest, and the worst of
  the three: a developer loses a conversation because a field the UI barely shows
  holds a string this build does not like.
- **Round on the way out.** Substitute a contract-named mode when the stored one
  is not one. **There is a precedent three lines above the reader**, and it went
  this way for exactly this class of problem — a `model_selection` that will not
  parse is read as `null`, with the comment "a worse answer than the stored one
  and a much better one than no conversation". The question this leaves is *which*
  mode, and it is not free: rounding an unnameable mode to `approval-required` is
  safe and may not be what the row meant, while rounding to `full-access` matches
  what a hand-edited `bypassPermissions` was reaching for and hands out latitude
  nobody asked this server for.
- **Migrate.** One `UPDATE` at startup, and then the reader is left alone. Fixes
  the rows that exist and nothing that arrives later — which, with ticket 12's
  doors shut, may be exactly the right scope. It is also the only one of the three
  that a developer can see the effect of.

They are not exclusive: a migration plus a rounding reader is the belt-and-braces
answer, and is what "the doors are shut but the store is not trusted" would look
like.

## What this is not

- **Not a second guard on the write path.** Ticket 12 did that and it is done.
- **Not about `model_selection`**, which already degrades and whose reasoning is
  the precedent cited above rather than the subject.

**Blocked by:** None. Wants the call on which of the three, and — if rounding —
which mode an unnameable one becomes.

**Status:** needs-triage
