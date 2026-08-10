Status: ready-for-agent

# Pinned threads and manual pinned ordering

Evidence and provenance: `.scratch/upstream-feature-scout/research.md`, T3 Code
upstream commit `9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6`, and upstream PRs
[#5312](https://github.com/pingdotgg/t3code/pull/5312),
[#5578](https://github.com/pingdotgg/t3code/pull/5578), and
[#5581](https://github.com/pingdotgg/t3code/pull/5581).

## Problem Statement

Laplus's Sidebar V2 separates conversations into active, snoozed, and settled
work, but it cannot express a simpler and more durable decision: “keep this
conversation near the top until I decide otherwise.” Important threads move as
their activity and lifecycle state change, and a developer cannot arrange the
important subset into their own priority order.

The missing behavior crosses the whole application. There is no pin/unpin or
pin-reorder command in the shared contract, no persisted pin state in the Rust
server's thread projection, no optimistic client operation, no pinned shelf,
and no common action-menu policy through which the sidebar and chat header can
offer the same action.

Local-only pin state would solve only the visible symptom. It would disagree
between windows and clients, lose the relationship to the thread that owns it,
and fail to converge when Laplus merges threads from several environments.

## Solution

Match the observable pinned-thread behavior of the selected T3 Code snapshot.
Each thread's owning server persists when it is pinned and its optional
fractional order key. Clients merge pins from every visible project and
environment into one Pinned shelf, sort them by the shared keys, and let the
developer drag reorder-capable rows into a manual order. Pin, unpin, and reorder
flow through the normal command/event/projection stream and update the UI
optimistically while the canonical event is in flight.

Pinning is an attention state layered over the existing lifecycle, with T3's
current interaction rules: snooze temporarily removes a pinned thread from the
Pinned shelf without losing its pin or position; waking restores its exact
slot; explicitly settling a pinned thread clears the pin; pinning a settled
thread clears settlement; and a visible pinned thread is not auto-settled.

The sortable shelf belongs to Sidebar V2, matching upstream. Pin/unpin policy is
shared between sidebar rows and the chat-header title menu so the two entry
points cannot drift. The compatibility `Sidebar` does not gain a duplicate
sortable shelf.

## User Stories

1. As a developer, I want to pin a thread, so that important work stays above
   the ordinary inbox until I release it.
2. As a developer, I want pins from all visible projects and environments in
   one shelf, so that I have one priority list rather than hidden per-project
   lists.
3. As a developer, I want a newly pinned thread at the top, so that pinning has
   an immediate prioritization effect.
4. As a developer, I want to drag pinned threads into my own order, so that the
   shelf reflects my priorities instead of activity time.
5. As a developer, I want that order to survive restart and synchronize through
   the thread's server, so that another client renders the same priorities.
6. As a developer connected to several environments, I want one drag to avoid
   rewriting unrelated neighbors on other servers, so that cross-environment
   ordering remains reliable.
7. As a developer, I want a snoozed pin to disappear until wake time and then
   return to its exact slot, so that “later” temporarily overrides “keep on
   top” without destroying either decision.
8. As a developer, I want settling a pinned thread to unpin it, so that marking
   work finished removes it from my priority list.
9. As a developer, I want pinning a settled thread to make it active, so that a
   thread I reprioritize cannot remain classified as finished.
10. As a developer, I want to unpin directly from a pinned row, so that removing
    a priority takes one action.
11. As a developer, I want pin/unpin in the chat-header menu, so that I do not
    have to return to the sidebar to manage the open thread.
12. As a developer connected to an older server, I want unsupported actions
    hidden or disabled without corrupting the order supported servers share.
13. As a developer, I want failed and concurrent reorder operations to converge
    visibly on canonical server state, so that the shelf never silently claims
    an order that was not persisted.

## Implementation Decisions

- T3 Code commit `9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6` is authoritative
  for observable pinning, lifecycle interaction, ordering, failure, filtering,
  and version-skew behavior. Laplus re-expresses the server half in Rust; it
  does not introduce an upstream runtime dependency.
- A thread shell carries optional `pinnedAt` and `pinOrderKey` fields. Historical
  payloads without either field decode as unpinned and unordered.
- The contract adds pin, unpin, and pin-reorder commands and their corresponding
  events, following the existing orchestration command/event naming and
  acknowledgement conventions. Pin accepts the fresh order key needed to place
  a new pin at the head without a second round trip.
- Environment capabilities independently advertise pin/unpin and manual pin
  reordering. Pin controls are absent against servers without pin support.
  Reorder UI excludes pins whose owning server lacks reorder support, while all
  clients still apply the same read-side sort.
- The Rust thread fold enforces T3's invariants and idempotency: pinning clears
  settlement, settling clears pin state and its key, unpinning clears the key,
  and reordering rejects an unpinned thread. Snooze preserves both pin fields.
- SQLite's thread projection stores `pinned_at` and `pin_order_key` through a
  forward migration. The live shell, archived snapshot where applicable, event
  replay, and restart rehydration all expose the same state.
- The Pinned shelf is one global order over the currently visible merged thread
  set, not one order per project. Project scope and other upstream filters show
  the matching subsequence and preserve T3's canonical reorder semantics.
- A fresh or re-pinned thread receives a key before the smallest key across all
  pinned shells, including pins temporarily hidden by snooze, and therefore
  appears at the top when visible. Unpin clears the old key rather than
  reserving a former slot.
- `pinOrderKey` is a base-26 fractional index sorted lexicographically. A normal
  drag writes only the moved thread on its owning server by choosing a key
  between its displayed neighbors. If legacy keyless or corrupt neighbors make
  that impossible, the client materializes evenly spread keys for the affected
  canonical section, matching upstream.
- Keyed pins sort first by order key with a stable scoped-identity tiebreak.
  Keyless legacy pins follow in newest-created-first order. Thread ids are only
  unique within an environment, so every ordering identity includes the
  environment.
- Reorder writes are optimistic. The temporary order remains until every key
  written by that drop is observed, canonical order already matches, membership
  changes, a foreign key wins, or a write fails. Failure releases the override,
  returns to canonical order, and surfaces an error toast.
- Concurrent writes use the existing event stream's last-write-wins behavior.
  Equal keys use the deterministic scoped-identity tiebreak so clients cannot
  render stream-arrival order differently.
- Sidebar V2 renders pinned threads as full rows above active threads with the
  upstream divider, drag activation threshold, vertical restriction, pin
  indicator, and one-click unpin behavior. Search and shelf keyboard/multi-
  selection behavior follow the selected upstream snapshot.
- Pinning does not add a sortable shelf to the compatibility `Sidebar`.
  Capability and operations live below the component so later removal of that
  compatibility branch does not require moving domain behavior.
- A shared thread-action-menu policy supplies consistent rename, pin/unpin,
  lifecycle, copy, archive/delete, and capability rules to Sidebar V2 and the
  chat-header title menu. The header gains the upstream title-triggered entry
  point; reorder remains a shelf-only action.
- No local UI store owns canonical pin membership or order. Local state is
  limited to transient drag/optimistic presentation.

## Testing Decisions

- Contract tests decode new commands, events, optional shell fields, and both
  capability flags, including compatibility with historical payloads.
- Rust fold/decider tests cover fresh pin, idempotent pin/unpin, pinning a
  settled thread, settling a pin, snooze/wake preservation, reorder rejection
  for unpinned threads, and deterministic event payloads.
- Projection tests prove pin state and order survive event replay, SQLite
  migration, restart rehydration, and every shell/snapshot read seam that can
  contain a thread.
- Pure client tests port upstream's fractional-key cases, including deep
  insertion churn, invalid keys, between-neighbor writes, keyless section
  materialization, scoped-id tie-breaking, new-pin-at-top, and mixed-capability
  ordering.
- Optimistic state tests cover single and multi-write confirmation, failure,
  membership change, concurrent foreign writes, snooze/wake, and release back
  to canonical order.
- Sidebar V2 tests cover global partitioning, project/filter subsequences,
  snoozed pins, settle/pin precedence, drag affordances, one-click unpin,
  unsupported servers, keyboard/multi-selection order, and the open-thread
  visibility exceptions inherited from existing shelves.
- Shared action-menu and ChatHeader tests prove both surfaces apply identical
  capability and lifecycle rules and dispatch against the correct scoped
  environment/thread identity.
- The primary server seam is a focused real-WebSocket test: pin, reorder,
  snooze/wake, settle/unpin, reconnect, and verify projected shell/event state.
- User-visible verification builds the current web bundle, drives a running
  Laplus window, pins threads from at least two projects, reorders them, reloads,
  snoozes and wakes one, settles another, and exercises pin/unpin from the chat
  header. Dev servers and watchers are stopped afterward.
- Focused verification follows repository policy: affected contract/client/web
  tests and checks, targeted Rust tests, and the UI-driver walkthrough. The full
  workspace suite remains CI's responsibility.

## Out of Scope

- A pinned shelf or drag-and-drop implementation in the compatibility
  `Sidebar`.
- Manual ordering of unpinned active, snoozed, settled, archived, project, or
  saved-draft rows.
- Local-only pins, per-project pin orders, or a user-selectable pin sort mode.
- Cross-server transactions or a central service that owns every environment's
  order. Each assignment remains a command to the thread's owning server.
- Backfilling order keys for every existing pin during migration. Keyless pins
  remain readable and materialize when upstream-compatible ordering requires it.
- Changing the saved-draft shelf design or making Sidebar V2 the default.
- Native mobile UI. Laplus has no native mobile application; responsive web and
  the Tauri shell share the Sidebar V2 implementation.
- General upstream synchronization or unrelated T3 sidebar features.

## Further Notes

- Upstream pinning originally made pin override snooze, but the selected commit
  contains the later, authoritative behavior in which snooze temporarily wins
  and preserves the pin's exact slot. Tickets must implement the selected
  snapshot rather than the older PR summary in isolation.
- The upstream reorder design deliberately minimizes cross-environment writes:
  ordinary moves write one thread on one server. This is part of the product's
  convergence behavior, not merely an implementation optimization.
- Ticketing should prefer vertical slices: establish the contract and Rust
  persistence with a thin read path first, then pin/unpin UI and shared menus,
  then manual ordering and its optimistic/concurrency behavior. Do not build a
  disconnected drag UI against local placeholder state.
