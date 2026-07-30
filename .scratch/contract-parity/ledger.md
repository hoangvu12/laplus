# Contract parity ledger

What `packages/contracts` declares, what `laplus-server` answers, and what the
difference costs. Produced 2026-07-30 by reading both trees.

This is **evidence, not a ticket**. Specs derived from it get their own feature
directory; this file is what they cite. Re-derive it by rerunning the counts in
[Method](#the-method) rather than trusting the numbers below after the code moves.

## The headline

| Surface                              | Declared | Answered |
| ------------------------------------ | -------- | -------- |
| RPC methods (`Rpc.make` in `rpc.ts`) | 61       | **33**   |
| Dispatchable orchestration commands  | 20       | **7**    |

**The docs are wrong.** `README.md:68`, `AGENTS.md:55` and `server/CLAUDE.md`
all say "26 of the 71 methods". The real figures are 33 of 61. 71 counted the
three dead strings and, apparently, a different denominator entirely; 26 is
simply stale. Fixing those three lines is its own small ticket.

`projects.list`, `projects.add` and `projects.remove` appear in `WS_METHODS` but
have no `Rpc.make`, so 64 strings resolve to 61 real methods. This is not a
discovery — `orchestration.rs:4` already documents them as dead — but it is why
the denominator is 61.

## This is not upstream drift

The instinct that laplus is behind upstream is wrong and worth killing early,
because it points at a sync that would not help.

- laplus's founding commit `2c9487a` is **2026-07-28**.
- `pingdotgg/t3code` HEAD `a8e05cb` (v0.0.31) is **2026-07-29**.

One day. Every gap below is between **our own contract and our own server**, and
our own bundled UI already calls into it. A sync would add UI calling methods
this server does not have, which is what `server/CLAUDE.md` already says and
what ticket 33 demonstrated.

## Gap 1 — orchestration commands: 7 of 20

`orchestration.rs:1313` refuses the rest with `Command not implemented by this
server: <kind>`. **All 13 are wired to live UI controls**, so each is a
reachable dead end rather than an unused branch.

Answered: `project.create`, `project.delete`, `thread.create`,
`thread.turn.start`, `thread.turn.interrupt`, `thread.approval.respond`,
`thread.user-input.respond`.

| Missing                               | UI surface     |
| ------------------------------------- | -------------- |
| `thread.checkpoint.revert`            | revert control |
| `thread.session.stop`                 | 3 call sites   |
| `thread.runtime-mode.set`             | mode picker    |
| `thread.interaction-mode.set`         | mode picker    |
| `thread.delete`                       | sidebar        |
| `thread.archive` / `thread.unarchive` | sidebar        |
| `thread.settle` / `thread.unsettle`   | inbox          |
| `thread.snooze` / `thread.unsnooze`   | inbox          |
| `thread.meta.update`                  | rename         |
| `project.meta.update`                 | rename         |

### The read model cannot express the lifecycle either

Adding dispatch arms is not sufficient for archive/settle/snooze/delete:

- `threads.rs:449-452` and `threads.rs:489-491` hardcode `archivedAt`,
  `settledOverride`, `settledAt` and `deletedAt` to `null`, in both the thread
  value and the shell summary.
- The `threads` table (`store.rs:73-101`) has **no columns** for any of them.
- `proposedPlans` is hardcoded `[]` and `hasPendingPlan` stays `false` — a
  separate, already-documented gap (`threads.rs:465-474`).

So that cluster needs a migration on a `STRICT` table in a shipped database
before any command can land. That is hard to reverse and is the one ADR-shaped
decision in this ledger.

### Two are much cheaper than the rest

- **`runtime-mode.set` / `interaction-mode.set`** — `runtime_mode` and
  `interaction_mode` columns already exist (`store.rs:88-89`) and are already
  emitted. They are write-once at thread creation today. Dispatch-arm work only,
  no migration.
- **`checkpoint.revert`** — the infrastructure is already built.
  `checkpoints.rs` captures the whole working tree at every turn boundary under
  `refs/laplus/checkpoints/<thread>/turn/N` and exposes `reference`, `capture`,
  `present`, `changed` and `patch`. A revert is a restore from a ref this server
  already writes.

  Upstream's shape, for reference: `decider.ts:900` turns the command into a
  `thread.checkpoint-revert-requested` event,
  `orchestration/Layers/CheckpointReactor.ts` does the git work, and
  `thread.revert.complete` closes it out (`decider.ts:1096`). The contract
  already declares all three.

## Gap 2 — RPC methods: 33 of 61

28 missing. **27 of the 28 are called by the shipped `client-runtime`** — they
are live, not speculative. `refusals.rs` already enumerates every method and the
error tag a refusal carries, so it is the place to read what a client sees.

| Cluster                    | n   | Methods                                                                                                                                                                        |
| -------------------------- | --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Preview + automation       | 12  | `preview.{open,navigate,resize,refresh,close,list,reportStatus}`, `previewAutomation.{connect,respond,focusHost}`, `subscribePreviewEvents`, `subscribeDiscoveredLocalServers` |
| Server admin + diagnostics | 8   | `server.{probe,refreshProviders,updateProvider,updateServer,getTraceDiagnostics,getProcessDiagnostics,getProcessResourceHistory,signalProcess}`                                |
| Worktrees + pull           | 3   | `vcs.{pull,createWorktree,removeWorktree}`                                                                                                                                     |
| Streams                    | 2   | `subscribeTerminalEvents`, `subscribeServerLifecycle`                                                                                                                          |
| Review                     | 1   | `review.getDiffPreview`                                                                                                                                                        |
| Orchestration              | 2   | `getArchivedShellSnapshot`, `replayEvents`                                                                                                                                     |

### `orchestration.replayEvents` should be deleted, not implemented

It is the **only** one of the 28 that no client calls, and upstream removed it
in `5fcdefd0` — "perf(server): trim stale context-window rows and drop dead
replay RPC (#4791)", 2026-07-28, the day laplus forked. It is dead on both
sides. Drop it from `packages/contracts`.

Note that `orchestration.rs:44-56` documents `afterSequence` handling in detail
and `rpc.rs:797-801` names `replayEvents` as the method its enumeration test
asserts is refused. Both want a look when it goes.

### A trap in `server.probe`

`refusals.rs:132-138` records it: the refusal tag for `server.probe` is one
`session.ts` converts into `ConnectionBlockedError` — a connection refused on
permission and not retried. It is dormant only because this server does not
advertise `capabilities.connectionProbe`, so the client probes with
`server.getConfig` instead. **Advertising that capability before implementing
`server.probe` turns every connection into a blocked one.**

## Gap 3 — the 16 methods our contracts dropped

Upstream declares 73 `WS_METHODS` and 6 orchestration methods; we declare 57 and 7. The 16-method difference splits cleanly, and most of it is intentional.

**Removed on purpose (9) — not gaps:**

| Commit                                                                               | Removed                                                                                                                                                                    |
| ------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `94da6be` "Remove the cloud, relay and Clerk surface the server will never answer"   | `cloud.getRelayClientStatus`, `cloud.installRelayClient`                                                                                                                   |
| `faf6ec5` "Remove the source-control hosting surface, which has no server behind it" | `sourceControl.{lookupRepository,cloneRepository,publishRepository}`, `server.discoverSourceControl`, `git.{runStackedAction,resolvePullRequest,preparePullRequestThread}` |

**Genuine post-fork drift (7) — safe to ignore:**
`server.{getBackgroundPolicy,reportClientActivity,reportHostPowerState,getResourceTelemetryHistory,retryResourceTelemetry}`,
`subscribeBackgroundPolicy`, `subscribeResourceTelemetry`.

All seven arrived in one upstream PR — `49c0d96e`, "Reduce idle work and disk
churn with native resource diagnostics (#2679)", 2026-07-29, the day after the
fork. **Nothing in upstream's `apps/web` or `packages/client-runtime` calls
them** (desktop and mobile only), so they cost our UI nothing.

The one extra orchestration method we declare and upstream does not is
`replayEvents`, covered above.

## What upstream spends on the clusters we lack

Non-test TypeScript LOC under `apps/server/src/`, as a rough sizing signal only
— laplus does not reimplement upstream line for line, and 33 of 61 methods
already fit in ~43.6k lines of Rust.

| Subsystem           | LOC   | Files |
| ------------------- | ----- | ----- |
| `review`            | 114   | 1     |
| `background`        | 450   | 2     |
| `process`           | 463   | 1     |
| `checkpointing`     | 610   | 5     |
| `diagnostics`       | 747   | 3     |
| `preview`           | 827   | 2     |
| `resourceTelemetry` | 3385  | 7     |
| `vcs`               | 5125  | 9     |
| `orchestration`     | 11580 | 27    |

`review` being ~114 lines against how central diff review is to the product is
the best value-per-line in the table.

## Suggested order

Cheapest-with-real-payoff first, and each line is independently shippable.

1. `thread.runtime-mode.set` + `thread.interaction-mode.set` — columns exist.
2. `thread.checkpoint.revert` — refs already captured.
3. `thread.session.stop` — no schema change.
4. Migration, then archive / unarchive / settle / unsettle / snooze / unsnooze /
   delete, plus `orchestration.getArchivedShellSnapshot`, and stop hardcoding
   the nulls in `threads.rs`.
5. `review.getDiffPreview`.
6. Delete `orchestration.replayEvents` from contracts.
7. Correct "26 of 71" in the three docs.
8. Preview subsystem — largest and most self-contained; defer.

Items 1–4 are one coherent feature and are being specced first. 5–8 are
independent.

## Limits of this ledger

Stated so a later reader does not over-trust it:

- **Implemented means "has a dispatch arm."** A method that dispatches but only
  partly satisfies its contract still counts as answered here.
  `assets.createUrl` is a known instance — `refusals.rs:237` documents it
  answering one of the contract's three asset resources via `partial_refusal`.
  There may be others; this ledger did not audit for them.
- **Only the method and command surface was read**, not the provider-event
  vocabulary. What the `claude` CLI driver emits versus what the contract's
  event types allow is a separate audit.
- Counts came from static reading, not from a running server. The surface walk
  in `server/tools/ui-driver/` (`surface-walk.mjs`, `surface-actions.mjs`) is
  the dynamic check and matches on the `Method not implemented by this server`
  wording — see `refusals.rs:100-108` before changing that sentence.

## The method

Reproduce with the trees side by side:

- Declared RPC methods — resolve `Rpc.make(WS_METHODS.x)` and
  `Rpc.make(ORCHESTRATION_WS_METHODS.x)` against the two `as const` maps in
  `packages/contracts/src/rpc.ts:129` and
  `packages/contracts/src/orchestration.ts:25`.
- Answered RPC methods — the `match tag` arms in
  `server/crates/laplus-server/src/rpc.rs:263-448`, with the `pub const … &str`
  method constants resolved per module.
- Declared commands — the `DispatchableClientOrchestrationCommand` union at
  `packages/contracts/src/orchestration.ts:749`.
- Answered commands — the `match` in `orchestration.rs`, ending at the
  catch-all on line 1313.
- UI usage — grep `apps/web/src` and `packages/client-runtime/src` for
  `WS_METHODS.<key>`, not for the wire string; the client never writes the
  string literal.
