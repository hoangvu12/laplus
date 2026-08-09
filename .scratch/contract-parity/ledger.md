# Contract parity ledger

What `packages/contracts` declares, what `laplus-server` answers, and what the
difference costs. Re-derived 2026-07-30 by reading both trees.

This is **evidence, not a ticket**. Specs derived from it get their own feature
directory; this file is what they cite. Re-derive it by rerunning the counts in
[Method](#the-method) rather than trusting the numbers below after the code moves.

**This file has already gone stale once**, and how is worth knowing. It was
written in `536ce72`, the first commit of the thread-lifecycle effort, and never
re-run while that effort closed thirteen commands and one method underneath it.
`ca8a81b` then corrected three documents by _copying this file's figures_ rather
than re-running its method, and published a stale number in the commit whose
subject was that a figure quoted without provenance is how the last one drifted.
The counts below are cheap to reproduce. Reproduce them.

## The headline

| Surface                              | Declared | Answered |
| ------------------------------------ | -------- | -------- |
| RPC methods (`Rpc.make` in `rpc.ts`) | 61       | **37**   |
| Dispatchable orchestration commands  | 20       | **20**   |
| `/api/access/cloudflare` HTTP routes | 18       | **18**   |

`projects.list`, `projects.add` and `projects.remove` appear in `WS_METHODS` but
have no `Rpc.make`, so 64 strings resolve to 61 real methods. This is not a
discovery — `orchestration.rs:4` already documents them as dead — but it is part
of why the denominator is 61. The other part is
[`orchestration.replayEvents`](#orchestrationreplayevents-should-be-deleted-not-implemented-done),
which was 61's last member and has since left the contract.

**The figure does not belong in prose.** `README.md`, `AGENTS.md` and
`server/CLAUDE.md` have each carried a parity number, and each has been wrong.
They now point here instead. This is the only file that should hold a count.

The third row runs the other way round and is new on 2026-08-03. The first two
count a contract the server is behind; this one counts a _server_ the contract
was behind — eight live routes that nothing in `packages/contracts` described.
See [Gap 4](#gap-4--http-routes-the-contract-did-not-describe-closed).

## This is not upstream drift

The instinct that laplus is behind upstream is wrong and worth killing early,
because it points at a sync that would not help.

- laplus's founding commit `2c9487a` is **2026-07-28**.
- `pingdotgg/t3code` HEAD `a8e05cb` (v0.0.31) is **2026-07-29**.

One day. Every gap below is between **our own contract and our own server**, and
our own bundled UI already calls into it. A sync would add UI calling methods
this server does not have, which is what `server/CLAUDE.md` already says and
what ticket 33 demonstrated.

There is no `upstream` git remote; `origin` is `hoangvu12/laplus`. Upstream is
read over the network at `github.com/pingdotgg/t3code`, which is what the
citations in this file mean.

## Gap 1 — orchestration commands: closed

All twenty kinds in `DispatchableClientOrchestrationCommand`
(`packages/contracts/src/orchestration.ts:749`) now have a dispatch arm. The
catch-all in `orchestration.rs` — `Command not implemented by this server:
<kind>` — is unreachable from a well-formed client.

Closed by the thread-lifecycle effort, `.scratch/thread-lifecycle/`: `536ce72`
(the two mode pickers), `38cf450` (both renames), `d2d2036` (checkpoint revert),
`ef681b0` (session stop), `0cdecf9` (the six lifecycle fields stop being
hardcoded nulls), `4690570` (archive and unarchive, and
`orchestration.getArchivedShellSnapshot` with them — the one RPC method this
effort closed), `5ffd349` (settle, unsettle), `adb8b08` (the activity resets),
`233c29c` (snooze, unsnooze), `079751c` (delete).

Twelve of that effort's thirteen tickets are `done`. The residue is ticket 13,
below.

### The one residue — ticket 13

`.scratch/thread-lifecycle/issues/13-...md`, `ready-for-agent` — the decision
below is now written into the ticket itself. Ticket 12 shut every
door that _writes_ a runtime or interaction mode, but `store.rs:1987` still reads
both columns straight off the row, so a stored value the contract does not name
would fail the client's decode of the whole thread — a conversation that cannot
be drawn rather than a wrong badge.

**Measured, not assumed:** the live database at `~/.laplus/state.sqlite` holds 2
threads, both `full-access` / `default`. There are no bad rows. The risk is real
in principle and zero in fact, so this does not jump the queue, and the migration
half of the ticket has nothing to migrate.

What it wants is the **read-side floor**: round a value the contract does not name
to `DEFAULT_RUNTIME_MODE` (`full-access`) or `DEFAULT_PROVIDER_INTERACTION_MODE`
(`default`), reusing `RUNTIME_MODES` and `INTERACTION_MODES`, three lines below
the identical `model_selection` degradation that already sits in the same
function. Low priority, behind everything in Gap 2.

## Gap 2 — RPC methods: 37 of 61

Twenty-four refused. `refusals.rs` enumerates all sixty-one and the error tag a
refusal carries, so it is the place to read what a client sees.

**Two of the twenty-seven this file first counted are gone**, in the two commits
the suggested order opens with. `server.probe` is answered, and
`capabilities.connectionProbe` with it. `orchestration.replayEvents` left the
contract, which is what moves the denominator from 61 to 60. Both sections below
are kept rather than deleted: what they argued is why the change was made, and a
ledger that erases its own reasoning the moment it is acted on is a worse
record than one that says which entries have closed.

| Cluster                    | n   | Methods                                                                                                                                                                        |
| -------------------------- | --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Preview + automation       | 12  | `preview.{open,navigate,resize,refresh,close,list,reportStatus}`, `previewAutomation.{connect,respond,focusHost}`, `subscribePreviewEvents`, `subscribeDiscoveredLocalServers` |
| Server admin + diagnostics | 7   | `server.{refreshProviders,updateProvider,updateServer,getTraceDiagnostics,getProcessDiagnostics,getProcessResourceHistory,signalProcess}`                                      |
| Worktrees + pull           | 2   | `vcs.{pull,createWorktree}` — `removeWorktree` is answered                                                                                                                     |
| Streams                    | 2   | `subscribeTerminalEvents`, `subscribeServerLifecycle`                                                                                                                          |
| Review                     | 1   | `review.getDiffPreview`                                                                                                                                                        |

**Twenty-six of the original twenty-seven had a live call site** in
`apps/web/src` or `packages/client-runtime/src`, counted by grepping
`WS_METHODS.<key>` — the key, not the wire string, which the client never writes.
`previewAutomation.connect` had four, `subscribeTerminalEvents` and
`server.probe` three each. Every other one had at least one. The single exception
was `replayEvents`, and that is what decided its fate. Past tense throughout,
because three of the twenty-seven have since closed; the finding it supports —
that this gap is UI calling a server that does not answer — is unchanged for the
twenty-four that remain.

### `orchestration.replayEvents` should be deleted, not implemented — **done**

It is the **only** one of the twenty-seven that no client calls, and upstream
removed it in `5fcdefd0` — "perf(server): trim stale context-window rows and drop
dead replay RPC (#4791)", 2026-07-28, the day laplus forked. It is dead on both
sides.

Implementing it is not a method, it is an architecture. ADR-0016 records why:
upstream's `readEvents` is `eventStore.readFromSequence` because upstream is
event-sourced — the log is the source of truth and a snapshot is a projection of
it. Here the source of truth is SQLite rows and in-memory state, and events are
announcements that nothing retains. A replay log would be a second representation
of every mutation, which is the thing ADR-0016 accepted "a cursor is answered at
its two ends" in order to avoid.

**Deleting it makes the target 60, and 60 of 60 is reachable.** Removing surface
no server will answer is established practice here — `94da6be` and `faf6ec5` did
it for nine methods. The rule that admits this one and nothing else: _a method may
leave the contract only when neither this repository's UI nor upstream's calls
it._

Two things to move first:

- **`rpc.rs` named `replayEvents` as its canary.**
  `every_method_this_server_refuses_answers_under_its_own_union` finds the refused
  set by asking dispatch, then asserts that set contains one named method — "so
  that this says something the code decided rather than roughly how much is left
  to build." That assertion needs a different subject before the method can go.
- **`tests/socket_handshake.rs`** refuses the same method end to end.

`orchestration.rs`'s "What this ticket does not do" also documents
`afterSequence` handling in detail and wants a look.

### A trap in `server.probe` — **implemented, and the trap held**

The method and the capability shipped in one commit, in that order, and the
sections below are what said they had to. `refusals.rs` now carries the same
warning from the other side, for whoever reads that row next.

`refusals.rs` recorded it, on the `server.probe` row, and still does: the
refusal tag for `server.probe` is one `session.ts` converts into
`ConnectionBlockedError` — a connection refused on permission and not retried.
It was dormant while this server did not advertise
`capabilities.connectionProbe` and the client probed with `server.getConfig`
instead (`session.ts:125`). **Advertising that capability before implementing
`server.probe` would have turned every connection into a blocked one**, which is
why the two shipped in one commit. The row now says the same thing pointing
forwards: the capability is advertised, so a regression that made this method
refuse would refuse the connection rather than draw an empty state.

The method itself is the cheapest thing on this list. Its contract is
`payload: Schema.Struct({})`, `success: Schema.Struct({})` — a ping. Upstream's
whole implementation is `ws.ts:1356`:

```ts
[WS_METHODS.serverProbe]: (_input) =>
  observeRpcEffect(WS_METHODS.serverProbe, Effect.succeed({}), { "rpc.aggregate": "server" }),
```

and `ServerEnvironment.ts:142` sets `connectionProbe: true` unconditionally. The
payoff is real: `rpc.rs`'s `get_config_is_repeatable` records that the client
re-sends `server.getConfig` as its liveness probe, so every liveness check used
to drag back the whole config payload. One line plus one flag, in one commit,
replaced it with an empty round trip.

### The automation methods have no producer

`previewAutomation.{connect,respond,focusHost}` and `subscribePreviewEvents` can
be implemented as declared, and would carry no traffic.

The **host** half already ships:
`apps/web/src/components/preview/previewAutomationRequestConsumer.ts` consumes
requests and answers them, and `client-runtime/src/state/preview.ts` calls all
three methods. What is missing is whatever _asks_ for a `click` or a `snapshot`.
Upstream's asker is the agent, reaching in over MCP — `apps/server/src/mcp/`, and
`ClaudeAdapter.ts:3523` hands the CLI a per-thread `mcpServers` entry:

```ts
mcpServers: { <name>: { url: mcpSession.endpoint,
                        headers: { Authorization: mcpSession.authorizationHeader } } }
```

laplus runs no MCP server. `agent.rs` mentions MCP only in doc comments about the
permission-prompt flag. **So the MCP server is off-contract work that four
declared methods depend on to be useful, and this ledger never counted it,
because this ledger counts methods.** It has its own effort; see
[What the clusters cost](#what-the-clusters-cost).

`previewAutomation.focusHost` is also the one preview method that reaches the
Tauri shell, so it needs a new verb on ADR-0021's named list.

### Preview is smaller than its method count suggests

**The server renders nothing.** `preview.reportStatus` is the _client_ telling the
server what the page is doing (`navStatus`, `canGoBack`); the client owns the
webview. The server keeps a list of tabs per thread, a `revision` counter and a
`serverEpoch` so a client can discard a stale answer, and broadcasts changes —
the same state-and-broadcast shape as everything already built.

So the Electron-versus-Tauri difference lands on `apps/web` and the shell, not on
this server. The one method with real per-OS work in it is
`subscribeDiscoveredLocalServers`: scan listening ports, resolve process name and
pid, and correlate with terminals this server owns — `DiscoveredLocalServer`
carries a `terminal: {threadId, terminalId}` field for exactly that. Windows and
Linux both, since CI runs both.

### Self-update is unblocked

ADR-0020 deferred `server.updateServer` as _"unblocked the moment a release exists
to update from."_ Two feeds now exist: GitHub Releases (`v0.1.0`, `v0.1.1`) and
the npm package (`laplus@0.1.1-rc.15`).

The capability is not a boolean, which ADR-0020's _"`capabilities.serverSelfUpdate`
stays false"_ gets wrong. `environment.ts:33` types it as one of three literals —
`boot-service`, `respawn`, `desktop-managed` — with absent meaning "must be
relaunched manually". Upstream resolves it in `cloud/selfUpdate.ts:92`:

```
desktop app supervising it            → "desktop-managed"
marked systemd user unit              → "boot-service"
published npm CLI on linux/darwin     → "respawn"
anything else                         → null
```

Under `desktop-managed` the server does not self-update at all and the client
never calls the method — so **`server.updateServer` is a headless method**, and
laplus's three shapes map cleanly: the Tauri window is `desktop-managed` (free,
and the updater is already built and keyed), `npx laplus` is `respawn` (the npm
feed exists and the mechanism is upstream's), and ADR-0028's systemd unit is
`boot-service` (needs a marker env var in a unit already installed on a machine,
so it goes last).

The property to copy, whichever path: upstream installs and verifies the new
version _before_ anything restarts — "a failed install leaves the running server
untouched."

One thing unsettled: the two feeds disagree on version (`v0.1.1` against
`0.1.1-rc.15`), and self-update has to answer "what version is available?"

## Gap 3 — the 16 methods our contracts dropped

Upstream declares 73 `WS_METHODS` and 6 orchestration methods; we declare 57 and 6. The 16-method difference splits cleanly, and most of it is intentional.

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

We used to declare a seventh orchestration method upstream does not,
`replayEvents`. Deleting it is what made the two orchestration maps agree, and
what makes this section's arithmetic come out at 16 rather than 17.

### Nothing in Gap 2 reintroduces any of it

Checked file by file against upstream, because three of the clusters look like
they might.

| Cluster                 | Upstream source                                                  | Reaches removed surface?                                                                                                        |
| ----------------------- | ---------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `review.getDiffPreview` | `review/ReviewService.ts`, 114 lines                             | **No.** Imports `GitVcsDriver` and `VcsDriverRegistry` only. Zero hits for `pullRequest`, `sourceControl`, `github`, `stacked`. |
| Diagnostics (4)         | `diagnostics/ProcessDiagnostics.ts`, `ProcessResourceMonitor.ts` | **No** — but see the trap below.                                                                                                |
| MCP server (7 files)    | `mcp/`                                                           | **No.** `effect/*` and its own siblings only.                                                                                   |
| Preview (2 files)       | `preview/Manager.ts`, `preview/PortScanner.ts`                   | **No.**                                                                                                                         |

**The diagnostics trap.** Upstream has two lookalike methods nine lines apart in
`ws.ts`:

```
serverGetProcessResourceHistory   → processResourceMonitor.readHistory   ← ours, declared
serverGetResourceTelemetryHistory → resourceTelemetry                    ← not ours, never taken
```

Different subsystems: `diagnostics/` (3 files) against `resourceTelemetry/`
(7 files, ~3.4k lines). Ours is the small one. Someone searching for "resource
history" can land on the wrong one and drag in a subsystem this repository
skipped on purpose.

**One adjacency, not a violation.** Upstream's self-update lives at
`cloud/selfUpdate.ts` — the _directory_ whose surface `94da6be` removed. The code
is npm, systemd and spawn; nothing cloud, relay or Clerk. But porting by directory
walks straight into the removed surface.

## What the clusters cost

Non-test TypeScript under `apps/server/src/`, as a rough sizing signal only —
laplus does not reimplement upstream line for line, and 37 of 61 methods already
fit in 54k lines of `crates/laplus-server/src` (`find . -name '*.rs' | xargs wc -l`,
so comments and unit tests included; `cargo xtask release` reports the
production-code figure separately).

| Subsystem     | LOC   | Files | Note                                                    |
| ------------- | ----- | ----- | ------------------------------------------------------- |
| `review`      | 114   | 1     | best value per line on the list                         |
| `diagnostics` | ~750  | 3     | **not** `resourceTelemetry`, which is 7 files and ~3.4k |
| `preview`     | ~830  | 2     | server-side registry only; the webview is the client's  |
| `mcp`         | 1464  | 7     | off-contract; `PreviewAutomationBroker.ts` is 587 of it |
| `process`     | ~460  | 1     |                                                         |
| `vcs`         | ~5100 | 9     | only `pull` and the two worktree methods are wanted     |

## Suggested order

Target is **61 of 61**; `replayEvents` has left the contract and the Usage report
has since added one declared-and-answered method. Ordered by when value lands;
each line is independently shippable. The first two are done and are kept here
so the order still reads as one sequence.

1. ~~`server.probe` + `capabilities.connectionProbe`, one commit.~~ **Done.**
   One line of Rust, and it shortens every liveness check.
2. ~~Delete `orchestration.replayEvents` from `packages/contracts`, after
   re-pointing the canary in `rpc.rs` and `tests/socket_handshake.rs`.~~
   **Done.** Both canaries now name `previewAutomation.respond`, chosen to be
   the last method still refused — which is item 7 below, and off-contract
   besides.
3. `review.getDiffPreview`.
4. `subscribeTerminalEvents` + `subscribeServerLifecycle` — one ticket. Both take
   an empty payload and stream one event type, and their siblings
   `subscribeTerminalMetadata`, `subscribeAuthAccess` and `subscribeServerConfig`
   are already implemented in that shape.
5. `vcs.pull`, then the two worktree methods. **In progress** as
   `.scratch/vcs/`, which took the cluster in its own order rather than this
   one: `removeWorktree` first, because it is the only one of the three with a
   live UI path — the delete-conversation flow offers to remove a worktree and
   until then could not. `createWorktree` and `pull` remain.
6. Server admin and diagnostics: `refreshProviders` + `updateProvider` together,
   the diagnostics trio, `signalProcess`, then self-update as three tickets —
   `desktop-managed`, `respawn`, `boot-service`.
7. Preview: the tab registry, then discovery, then the automation router.
8. The MCP server — its own effort, off-contract, and what gives the automation
   router traffic.

Sized to decisions rather than to methods this is roughly 13–15 tickets, not 27.

Each cluster gets its own feature directory citing this file, per the note at the
top. The MCP server gets one too: `mcp/` is a general agent-tool surface whose
only toolkit today happens to be preview, so it outlives the preview effort and
should not be owned by it.

## Gap 4 — HTTP routes the contract did not describe: closed

**This gap points the opposite way to the other three.** Gaps 1–3 are methods
the contract declares and the server does not answer. This one was eight routes
the _server already served_ that no `HttpApiEndpoint` described, so no generated
client could reach them and no schema said what they answered with — a contract
can be behind its server as easily as ahead of it, and only one of those two
directions had ever been counted here.

Found on 2026-08-03 while implementing ticket 04 of `.scratch/cloudflare-tunnel/`.
The eight:

| Route                                              | Landed by | Why it was missed                                    |
| -------------------------------------------------- | --------- | ---------------------------------------------------- |
| `GET /api/access/cloudflare/account`               | ticket 04 | the server half landed before the contract half      |
| `POST /api/access/cloudflare/account/login`        | ticket 04 | as above                                             |
| `POST /api/access/cloudflare/account/login/cancel` | ticket 04 | as above                                             |
| `POST /api/access/cloudflare/account/consent`      | ticket 04 | as above                                             |
| `POST /api/access/cloudflare/account/tunnels`      | ticket 04 | as above                                             |
| `POST /api/access/cloudflare/account/select`       | ticket 04 | as above                                             |
| `GET /api/access/cloudflare/challenge`             | ticket 01 | no client calls it — laplus answers it to itself     |
| `GET /api/access/cloudflare/challenge/ws`          | ticket 01 | as above, and a `101` that `HttpApi` cannot describe |

All eight are now declared. The two challenge routes carry no authentication
middleware, because their caller is this server's own verifier holding a
single-use diagnostic token rather than a session; the WebSocket one is declared
with a `Void` success and a comment saying so, because nothing in `HttpApi` can
express an upgrade. They are declared anyway so that a route audit finds every
path rather than only the ones a client drives.

**The untagged refusal body is closed too**, by the Cloudflare cleanup pass on
2026-08-03. Every Cloudflare route used to refuse a precondition with `409` and
a rejection with `400`, both carrying an untagged `{ "message": … }` that
decoded as no tagged `Environment*Error` — so the reason never reached the
browser and the routes' declared error sets were a partial truth. The eleven
mutating endpoints now declare `EnvironmentPublicExposurePreconditionError`
(409) and `EnvironmentPublicExposureRejectedError` (400), which carry a closed
`reason`, the server's sentence, and the mutations a partial failure completed
and left outstanding. `Refused` in `server.rs` builds them.

`EnvironmentScopeRequiredError` is deliberately not folded into that union: a
client without the scope is refused before any reason is evaluated and learns
only which scope it needs, which is ADR-0047's rule that a refusal discloses
nothing. `http_cloudflare_account.rs` asserts that the scope refusal carries no
`reason` and no `message`.

**One untagged body is left, and it is recorded rather than fixed.**
`POST /api/access/cloudflare/test` answers `504` with `{ "message":
"Verification is still running." }` when a bounded verification has not settled.
It is outside the eleven because it is neither a precondition nor a rejection —
nothing was refused, the answer is not ready — and giving it a shape would mean
a third tagged class for one route. It is the same defect as the eleven, at a
tenth of the cost to leave, so it is written down here instead of forgotten.

## Limits of this ledger

Stated so a later reader does not over-trust it:

- **Implemented means "has a dispatch arm."** A method that dispatches but only
  partly satisfies its contract still counts as answered here.
  `assets.createUrl` is a known instance — `refusals.rs`'s `partial_refusal`
  documents it answering one of the contract's three asset resources.
  There may be others; this ledger did not audit for them.
- **Answered does not mean useful.** Three methods on the list would dispatch
  correctly and do nothing a developer could see: the three automation methods
  without an MCP server. `server.updateServer` under `desktop-managed` is a
  fourth, though the contract sanctions that one. Reaching 60 of 60 without
  saying so would make the figure mean what "26 of 71" meant.
- **Only the method and command surface was read**, not the provider-event
  vocabulary. What the `claude` CLI driver emits versus what the contract's
  event types allow is a separate audit.
- **The HTTP row covers `/api/access/cloudflare` only.** The other twenty-six
  routes in `server.rs` were not walked against the contract, so a gap of the
  same shape may exist under `/api/auth` or `/oauth`. The command below is the
  one to widen when someone wants that answer.
- Counts came from static reading, not from a running server. The surface walk
  in `server/tools/ui-driver/` (`surface-walk.mjs`, `surface-actions.mjs`) is
  the dynamic check and matches on the `Method not implemented by this server`
  wording — see `REFUSAL_SENTENCE` in `refusals.rs` before changing it.

## The method

Reproduce with `gh` for the upstream half; there is no upstream checkout and no
upstream remote.

**Declared RPC methods — 61.**

```sh
grep -c 'Rpc.make' packages/contracts/src/rpc.ts
```

All sixty-one `Rpc.make` calls live in `rpc.ts`, including the orchestration ones,
which resolve against the `as const` map in
`packages/contracts/src/orchestration.ts:25`. The two maps hold 58 and 6 keys;
`projects.{list,add,remove}` have no `Rpc.make`, so 64 strings are 61 methods.

**Answered RPC methods — 37.** The arms of the `match tag` in
`server/crates/laplus-server/src/rpc.rs`, with each `pub const … &str` resolved.
Bounded by the function rather than by line numbers, which is the correction this
file's first version needed: it hardcoded `NR>=262 && NR<=470`, and `dispatch`
had already moved by the time anyone re-ran it.

```sh
awk '/^pub fn dispatch\(/,/^}/' server/crates/laplus-server/src/rpc.rs \
  | grep -oE '^        [A-Za-z_:]+ =>' | sed 's/ =>//;s/^ *//' | grep -v '^unknown$' | wc -l
```

**The refused set — 24.** `REFUSALS` in `refusals.rs` is the contract's own list
read out, so subtracting the answered set from it gives the gap without a second
list to drift:

```sh
awk '/const REFUSALS: &\[\(&str, Tag\)\] = &\[/,/^\];/' \
  server/crates/laplus-server/src/refusals.rs \
  | grep -oE '^\s+\("[^"]+"' | grep -oE '"[^"]+"' | tr -d '"' | sort > /tmp/declared.txt
# resolve the 37 arms above to their wire strings into /tmp/implemented.txt, then
comm -23 /tmp/declared.txt /tmp/implemented.txt
```

**Declared commands — 20.** The `DispatchableClientOrchestrationCommand` union at
`packages/contracts/src/orchestration.ts:749`.

**`/api/access/cloudflare` routes — 18 served, 18 declared.** Both sides read as
paths, so a route the contract spells differently shows up as one entry missing
from each list rather than silently matching.

```sh
grep -oE '\.route\("/api/access/cloudflare[^"]*"' \
  server/crates/laplus-server/src/server.rs \
  | grep -oE '"[^"]+"' | tr -d '"' | sort -u > /tmp/cf-routes.txt
grep -oE '"/api/access/cloudflare[^"]*"' \
  packages/contracts/src/environmentHttp.ts | tr -d '"' | sort -u > /tmp/cf-contract.txt
comm -23 /tmp/cf-routes.txt /tmp/cf-contract.txt   # served, undeclared
comm -13 /tmp/cf-routes.txt /tmp/cf-contract.txt   # declared, unserved
```

**Answered commands — 20.** The `match` in `orchestration.rs`, ending at the
`Command not implemented by this server` catch-all:

```sh
grep -noE '"(project|thread)\.[a-z.-]+"' server/crates/laplus-server/src/orchestration.rs \
  | sort -u -t: -k3
```

**UI usage.** Grep `apps/web/src` and `packages/client-runtime/src` for
`WS_METHODS.<key>`, not for the wire string — the client never writes the string
literal, and the key is not the wire string with the dots removed
(`server.updateServer` is `serverUpdateServer`).

**Upstream.** `gh api "search/code?q=<term>+repo:pingdotgg/t3code"` to locate, then
`gh api "repos/pingdotgg/t3code/contents/<path>" -q '.content' | base64 -d` to
read.
