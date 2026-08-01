# ADR-0027 — A line of the agent's is decided and then applied, and the decision is what a test can hold

Date: 2026-07-30
Status: Accepted

## Context

ADR-0025 cut the pure fold out of `threads.rs` yesterday and said the guarantee
is about `fold` rather than about the file. The same review that produced it
measured `crate::turn` and found the same shape one level down, with the numbers
worse.

`turn::publish` was 368 lines — a nine-arm match over `protocol::Folded` that
turns one line of the agent's NDJSON into changes to a conversation, and 26% of
the file's implementation. Every pure function beside it was already covered:
12 inline tests reached 17 of the file's 27 of them. `publish` had **none**, and
not for want of trying. It applied its own results, so the only way to reach an
arm was a live `Threads` with a real `claude` child behind it, and the assertion
at the end was on whatever the socket had published by then.

It was impure for one reason, and the reason was uniform: **twelve call sites on
`Threads`**, ten of them `apply`. The other two are what decided the shape of
this ADR rather than of the last one:

- the provider resume cursor produced in `Folded::Initialized`. It queues a
  durable write and publishes nothing. **No `Change` describes it and none
  could** — the contract has no event for continuation.
- `active_turn`, read _between_ two applies in `Folded::Completed` and gating the
  second on the answer. A developer who stopped the agent can send the next turn
  while this one is winding down, so the ending must go up only while the session
  is still describing the turn that ended.

Two smaller impurities are not on that list and are not touched here: two
`fresh_message_id` calls and one `now_iso`. The clock read turned out to be
removable for free; the ids did not, and that is written down below.

`tests/protocol_golden.rs` already checks the _first_ half of the join
`CONTEXT.md` names — `protocol` → `Folded`, against 19 captured NDJSON sessions.
The second half was unreachable.

## Decision

**`decide` answers with what the line turned out to mean; `spend` applies it.**

```rust
fn decide(folding: &mut SessionState, driving: &mut Driving, line: &str) -> Decided
fn spend(threads: &Threads, start: &Start, decided: Decided)
```

`decide` takes no `Threads`, no `Start`, no lock, no clock and no child process.
`spend` is 33 lines, every one of them a call on `Threads`.

**`Decided` has three fields rather than being a `Vec<Change>`,** because two of
the twelve things `publish` did are not changes:

```rust
struct Decided {
    changes: Vec<Change>,
    provider_resume_cursor: Option<ResumeCursor>,
    settles: Option<Settles>,
}

struct Settles {
    turn_id: Option<String>,
    status: SessionStatus,
    last_error: Option<String>,
}
```

`Settles` carries two of a `Session`'s five fields and not the other three, and
that split is the seam rather than a convenience: **how the turn went** is what
the line decided, while **which session and when** are facts the driver has held
since it started the child. `runtime_mode` and `updated_at` are filled in by
`spend`, which is what takes the last clock read out of `decide`.

`Settles::turn_id` is a precondition rather than an answer. `spend` asks
`active_turn` only when there is something gated on it, so the lock is taken on
the lines that end a turn instead of on every line the agent writes.

**Nothing about the wire changes.** Same events, same order, same numbering. The
one ordering hazard the change creates is that the context-window row is decided
_ahead of_ the match and three arms return early; it was applied before the early
`return` when the function published as it went, and losing it became possible
for the first time here. There is a test named after it.

**The two `fresh_message_id` calls stay, and `decide` is not called pure.** Its
doc comment says what it is: a function that advances the reducer, mutates the
turn in flight, and calls five clock-reading `Activity` constructors — but
returns its results instead of applying them. That is a smaller claim than
ADR-0025's and it is the true one.

**Eighteen inline tests**, one or more per arm, in `#[cfg(test)] mod tests` in
`turn.rs`. They assert on which changes a line produces and in what order, from
minimal NDJSON lines of the shapes `crate::protocol`'s own tests use.

## Considered options

- **Golden files, as `protocol_golden.rs` does it** — a `*.changes.json` sibling
  per capture. This is what "the same way" would literally mean, and it is what
  the candidate proposed. Rejected: every activity in these arms is built by
  `Activity::info` / `failed` / `tool` / `approval`, which ADR-0025 named
  nineteen hours ago as the five constructors that read a clock and mint ids and
  kept impure on purpose. A golden file needs a value two runs agree on, so it
  needs those injected — reversing that ADR inside the commit that cites it as
  precedent — or it needs `id` and `createdAt` stripped before comparing, which
  is a golden file that silently stops covering the two fields a golden file
  would otherwise be for.
- **Return `Vec<Change>`.** The tightest interface, and wrong twice: it has
  nowhere to put the provider resume cursor, and it cannot express a change whose
  publication is conditional on a read of the world. Either omission puts
  `&Threads` back in the signature, and a `decide` that still holds a `Threads`
  is a rename rather than a cut.
- **Return the whole `Session` in `Settles`.** Smaller diff and one less shape.
  Rejected: `runtime_mode` would have to be handed to `decide`, and `updated_at`
  keeps `now_iso` inside it — so `decide` would take a parameter it uses on one
  line of one arm in order to keep a clock read it does not need.
- **Move `unmeasured` and `finished` onto `Decided`.** They are already what that
  type is: decided on one line, spent on the next. Rejected: both are read by the
  loop rather than by `spend`, and neither is a change to a conversation.
  It would widen `Decided` from "what happened to the thread" to "what the driver
  does next" and buy no assertion — they are `&mut Driving` fields, and the tests
  read them off the `Driving` they handed in.
- **Hand a message id into `decide` alongside the timestamp.** It would make the
  function deterministic and the golden file above possible. Rejected as ADR-0025
  rejected its own version: the id is minted _conditionally_, in two arms, one of
  which only sometimes spends it, so the caller would mint one per line and throw
  most of them away — and nothing is more testable afterwards that these
  eighteen tests do not already reach.
- **Split `decide` into nine functions, one per arm.** Rejected: the arms share
  `driving`, `folding` and the accumulator, and eight of them are already one
  statement plus the paragraph explaining it. The match is not the complexity.
- **Keep the name `publish`.** Rejected: it no longer publishes. `spend` is the
  word ADR-0025 already uses for the other half of this pair — `commit` spends
  the `Rendered` — so the two seams read alike, which is the point of taking the
  same cut twice.

## Consequences

- **26% of the file goes from nothing covered to eighteen tests.** `turn.rs` had
  12 tests over 17 pure functions and 0 assertions over 1,153 impure lines; it now
  has 30, and the second half of the join `CONTEXT.md` declares is checkable
  without a socket, a database or a `claude` on the path.
- **The file got longer, not shorter** — 2,374 lines to 3,003, of which the
  new tests are ~450. That is the trade ADR-0025 named and it lands the same way:
  what is bought is a subject per function, paid for in lines.
- **`decide` still reads a clock, twice removed and once directly.** Five
  `Activity` constructors and `fresh_message_id`. The doc comment says so rather
  than claiming otherwise, which is ADR-0025's asterisk widened by one line.
  Anything added to that list is a decision, not a drift.
- **The context-window row is the one thing a careless edit can now lose.** It is
  decided before the match and three arms `return decided` early. `the_early_returns_still_carry_the_context_window_row_out` is what notices.
- **`Driving` is unchanged.** `unmeasured` and `finished` are still the one-line
  handoffs from the fold to the loop they were, and their doc comments now say
  why they did not move.
- **Candidate 5 of the same review can be re-measured.** Splitting `drive`'s
  408-line loop was ranked last and marked speculative pending this landing;
  `drive` is now four lines shorter and one of its statements is a pair.
