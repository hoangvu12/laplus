# ADR-0001 — Settling is mirrored from upstream; the encoder stays per-provider

Date: 2026-07-27
Status: Accepted

## Context

Two vocabularies describe a turn's lifecycle, both fixed by the contract:
**session status** (seven literals) and **turn state** (four). Two rules connect
them, and they point in opposite directions:

- **Encode** — what status to publish, given what the driver knows. lightcode's
  is `turn::Ending::session_status`, which knows that the `claude` CLI reports a
  turn the developer stopped as a failed one.
- **Decode / settle** — how a still-running turn settles when a status arrives.

Before this decision, the decode half was spread over five hand-written matches
across `turn.rs` and `threads.rs`, all trafficking in `&'static str`, and one of
them disagreed with upstream: `stopped` left a running turn running.

It was tempting to treat the two rules as one codec — encode and decode as
inverse functions behind a single interface, with a round-trip property test.
Reading upstream showed that to be wrong.

## Decision

**The decoder is a mirror, and lives in one module.** `crate::settling` owns
`SessionStatus`, `TurnState` and `settles_turn_as`. Its authority is upstream,
which writes the same rule down twice, character for character:

- `apps/server/src/orchestration/Layers/ProjectionPipeline.ts:78`
- `packages/client-runtime/src/state/threadReducer.ts:539`

Upstream needs both because its server and its client fold the same events into
the same read model. lightcode reuses that client unmodified, so ours is the
third copy and the only one under this repository's control. Its correctness is
therefore not a matter of opinion — it is agreement — and the test asserts
exactly that, with the table transcribed rather than derived.

**The encoder stays with the driver, per-provider.** Upstream keeps two of them
(`ProviderRuntimeIngestion`, `ProviderCommandReactor`) and shares neither with
its client, because each knows about one provider's runtime. `Ending` is
lightcode's, and it knows about the `claude` CLI. A second driver would bring
its own encoder; it would not bring another copy of the decoder.

**The two are not inverses.** `Ending::Completed` encodes to `ready`, which
settles a turn as `completed`. A round-trip property test would be asserting
something untrue.

**Both vocabularies are typed.** `Session.status` and `LatestTurn.state` are
enums, not `&'static str`. That is what stops a sixth site writing
`"interrupted"` by hand.

## Consequences

- `stopped` now settles a running turn as `interrupted`, matching upstream.
  Currently unreachable — `turn.rs` reports an unfinished turn as `error` and
  keeps `stopped` for when none was running — so no behaviour a developer sees
  changes today. Ticket 15 owns whether that encoder choice should change.
- A status added to the contract is a compile error here, not a silent
  fall-through.
- Do not propose fusing `Ending` and `settling` into one codec. That was
  considered and rejected on the evidence above; this ADR exists so a future
  architecture review does not re-suggest it.
- The three-way duplication with upstream remains, by necessity. What changed is
  that it is now one copy instead of five, and it is pinned.
