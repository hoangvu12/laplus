# ADR-0002 — `threads` is not split into a conversation log and a running-agent registry

Date: 2026-07-27
Status: Accepted

## Context

`crate::threads` is the largest module in the server: 2,511 lines, of which 1,770
are code and the rest its own tests. An architecture review, prompted by ticket
16 (concurrent sessions), proposed cutting it in two — a **conversation log**
holding transcripts and the read model, and a **running-agent registry** holding
`Live`, the prompt and signal channels and the driver handles. The argument had
three legs: the file is too big, the running-agent half is about half of it, and
that half has no tests.

The proposal was grilled and each leg was measured. All three are wrong, and the
measurements are worth writing down because a future review will otherwise
re-derive the same proposal from the same first impression.

**The running-agent half is not half.** Counted honestly — `Live`, `Prompt`,
`Signal`, `Answered`, the `winding_down` and `live_agents` fields, `session`,
`detach`, `forget` and `shutdown` — it is roughly 270 lines of 1,770. The
impression that it is half comes from it being the *conceptually* separate half,
not the large one.

**It is not untested.** "Zero tests" was true only of inline `#[cfg(test)]` units.
Through the primary seam it is one of the most heavily driven paths in the
project: nine cases in `socket_interrupt.rs`, nine in `socket_permissions.rs`,
eleven in `socket_turn.rs`, eight in `socket_continuity.rs`, and now seven in
`socket_concurrency.rs`. The spec puts the bulk of testing at the socket boundary
on purpose (Testing Decisions, "Primary seam"), so counting unit tests to decide
what is covered measures the wrong thing here.

**It would not have served ticket 16.** The split was proposed to make "server
state is per-session rather than global" true. It already is: there is no
`static mut`, no `OnceLock` singleton, no `set_var` and no ambient `current_dir`
on the agent path, and every piece of state hangs off an `Arc` owned by one
server instance and keyed per thread — an `Entry` gives each conversation its own
state, its own event feed and its own `Live`. Moving those fields into a second
struct would not have changed what a client can observe. `socket_concurrency.rs`
demonstrates it as built.

## Decision

**`threads` stays whole.** The proposed log/registry cut is rejected.

**If the file ever does need splitting, the cut is the pure fold, not the
registry.** Upstream's own primary seam is there: `projector.ts` is a fold with
no I/O, which is what makes it testable in isolation and reusable by both the
server and the client. lightcode has roughly 730 equivalent lines sitting
un-separated inside `threads.rs` — `Thread` with `to_detail_value` and
`to_shell_value`, `Change`, `Threads::fold`, and the event constructors. Those
take an event and a state and return a new state and a rendered payload; nothing
in them touches a channel, a child process or a clock.

That is the cut worth taking, and naming it here is the point of this ADR: the
next review should start from it rather than from the registry.

## Consequences

- Do not re-propose the log/registry split without new evidence. The numbers
  above are the ones to argue with.
- The three things two conversations genuinely share stay shared, because each is
  shared for a reason a per-session copy would break: `Sequences` (one total
  order across the feed, which the client relies on to drop rather than reorder),
  the `shell` broadcast (the project list is one list), and the observability
  gauges (they count the server, not a session). That they aggregate rather than
  bleed is asserted in
  `socket_concurrency.rs::what_the_two_conversations_share_is_shared_on_purpose`.
- The size of `threads.rs` remains a real cost, and this ADR does not claim
  otherwise. What it claims is that the proposed cut would have paid it down in
  the wrong place.
