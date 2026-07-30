# ADR-0025 — The fold is cut out of `threads`, and it is total by its function rather than by its file

Date: 2026-07-30
Status: Accepted

## Context

ADR-0002 refused the log/registry split of `crate::threads` and named the cut
that should be taken instead: **the pure fold**. It put that at roughly 730
lines — `Thread` with its two renderings, `Change`, `Threads::fold` and the event
constructors — and said the next review should start from it rather than
re-deriving the registry proposal from a first impression.

An architecture review on 2026-07-30 started there and measured it. Every figure
in ADR-0002 has moved, all in the same direction:

|                                | ADR-0002 | now   |
| ------------------------------ | -------- | ----- |
| `threads.rs` total             | 2,511    | 4,832 |
| production lines               | 1,770    | 3,198 |
| the fold, as ADR-0002 named it | ~730     | 890   |
| `Threads::fold` alone          | 156      | 306   |
| the runtime half               | ~270     | 445   |

Nine commits of the thread-lifecycle effort landed between the two measurements
and almost all of the growth is fold-side: `Lifecycle`, `Shelf`, `Busy`,
`Adoption`, four new `Change` variants, and the
`re_emitted_at` / `wakes_the_inbox` / `wants_waking` triangle. The runtime half
gained one method in that window, `stop_session`. Counted honestly — everything a
fold module would have to take, including the state types the ADR did not list —
the pure surface is **1,889 lines against the runtime's 1,040**. The ratio
ADR-0002 was arguing about has inverted and is now 1.8:1 the other way.

Two facts about the code decided most of what follows.

**The seam already exists.** `Threads::commit` stamps the clock on one line and
hands it to the fold on the next:

```rust
let occurred_at = change.re_emitted_at(thread).unwrap_or_else(now_iso);
let payload = self.fold(thread, change, sequence, &occurred_at);
```

Both the instant and the sequence number are already parameters. Extracting the
fold does not require threading time through it — that work was done when
`re_emitted_at` was written.

**`&self` was spent in one place.** Of the 890 lines ADR-0002 named, four are
impure, and three of them are the same statement: `Threads::message_sent` bumps
two `AtomicUsize` counters and writes a `laplus:` line to stderr, recording
whether the buffered assistant message matched what the deltas had already built.
The fourth is `fresh_id("event")` in `thread_event` — which turns out not to be
inside the fold at all, because `commit` builds the event envelope itself.

## Decision

**`crate::threads::fold` is a module, and `threads.rs` keeps the running agent.**

The whole pure surface crosses, not ADR-0002's narrower list: `Thread`,
`Lifecycle`, `Shelf`, `Busy`, `Adoption`, `Change` and its four predicates, both
renderings, `durable`, `settle`, `bind_assistant_message`, the inbox-state
constants, and the data types a thread is made of. `threads.rs` keeps `Threads`,
`Inner`, `Entry`, `Live`, `Signal`, the lock-and-publish path, the read-model
queries and every method that touches a child process.

**The interface is one function.**

```rust
fold(&mut Thread, &Change, sequence: i64, at: &str) -> Rendered
```

`Rendered` carries the payload and, when there was one, the reconciliation
verdict. Returning that verdict as data is what lets `fold` drop `&self` and
become a free function; `commit` bumps the counters and writes the stderr line,
on the side of the seam that was already doing both.

**The guarantee is about `fold`, not about the file.** `Activity::info`,
`Activity::tool`, `Activity::approval`, `Activity::failed` and `Adoption::now`
read the clock and mint identifiers, and they move with the types they construct.
They are not called by the fold: they are called at 21 sites in `turn.rs`,
`worklog.rs` and `orchestration.rs` to build the `Change` that is then handed to
it. The clock enters upstream of the fold, as it did before this ADR.

**`threads` re-exports what the rest of the crate uses.** No module outside
`threads.rs` changes an import, and the nine `crate::threads::` paths `CONTEXT.md`
cites stay true.

**Tests follow their code.** The ~35 inline tests that drive the fold move with
it; the ~6 driving `subscribe`, `create` and publication stay with the registry.
They stay `#[cfg(test)]` blocks in the same files as the code, because
`xtask::loc` classifies a line as a test only inside such a region — a test-only
module file would be counted as production and measured against the spec's 20,000
signal.

## Considered options

- **Take ADR-0002's list literally — 890 lines.** Rejected: it strands
  `Lifecycle`, `Shelf`, `Busy` and `Adoption` on the runtime side. All four are
  total, all four are what tickets 09 and 10 write, and the effect would be to
  split one ticket's work across the seam.
- **Cut only `Change` and `fold` — 630 lines.** The tightest seam, and barely
  deeper than the function it wraps. Rejected for the same reason: the renderers
  and the state they render would sit on the other side of a line drawn through
  the middle of one idea.
- **Pass the counters into the fold** rather than returning the verdict.
  Smaller diff, and `fold` keeps returning `Value`. Rejected: the module would
  then know about atomics and stderr, and "the fold is total" would be true in
  the doc comment and false in the code.
- **Leave `message_sent` behind.** It is called only from `fold` and is 83 of 86
  lines pure. Rejected: it makes the fold impure by call instead of by body,
  which is worse — the same fact, harder to see.
- **Inject `at` and an id into `Activity`'s constructors** so the module is total
  end to end. Rejected as the wrong price: 21 call sites across three modules
  change, `worklog.rs` and `turn.rs` gain clock reads they do not have today, and
  an activity's `createdAt` silently changes meaning from the moment it was
  constructed to the moment it was committed. Nothing is more testable
  afterwards — these paths are driven through the socket.
- **Name the module something without a collision** — `projection`, `read_model`.
  Rejected: ADR-0002 said the cut is _the fold_, and a differently-named module
  makes that forward reference stop landing, which is the failure that ADR was
  written to prevent. `projection` is also a poor neighbour for `projects`, where
  `Project` is a first-class type.
- **Import from `crate::threads::fold::` at every call site.** Honest, and the
  import line would say where the code lives. Rejected for this commit: rewriting
  six modules' imports in the same change as a 3,300-line move makes it harder to
  read as the move it is. Nothing stops a later commit doing it.

## Consequences

- **This does not make the file small.** One 4,832-line file becomes roughly
  3,280 and 1,300. What it buys is one subject per module, and it is the same
  trade ADR-0002 declined to pretend away: the size of this code is a real cost
  and cutting it here pays it down in the right place rather than in no place.
- **"Pure" carries an asterisk, and it is written down.** A reader who opens
  `fold.rs` finds five functions that read a clock. The module doc names them and
  says why they are not a violation: they construct the fold's inputs, they are
  called from outside, and `fold` itself never calls one. Anything added to that
  list is a decision, not a drift.
- **`fold` is a free function and can be tested as one.** No `Threads`, no lock,
  no channel: a `Thread`, a `Change`, a number and a timestamp in, a `Rendered`
  out. The reconciliation verdict is now assertable directly rather than by
  reading `Threads::reconciliation` back afterwards.
- **The word "fold" now has two meanings in `CONTEXT.md`.** The refs one — a
  remote ref dropped because a local branch shadows it — is prose, ours, and
  pinned by nothing in the contract. This one is an identifier and an ADR's name
  for a decision. The glossary gives seniority to this one and both entries
  cross-reference, exactly as `Question` and `Settling` are handled. See
  ADR-0024 for the same move made for the same reason.
- **Tickets 09 and 10 land in the new module.** The cut goes first because it
  changes no behaviour, no wire message and no test assertion; those tickets then
  write into a module shaped for them instead of into a file about to be taken
  apart.
- **Do not re-propose the registry split.** ADR-0002's argument is unaffected by
  this one, and its numbers have only got stronger.
