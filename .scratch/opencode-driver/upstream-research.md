# Upstream research — how T3 Code drives OpenCode

Written 2026-08-01 against `pingdotgg/t3code` commit
[`0ad91b6e`](https://github.com/pingdotgg/t3code/tree/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62).
This is implementation research, not a spec. No OpenCode process was run.

## Identity and conclusion

“T3” is ambiguous, but the relevant project is **T3 Code**, the official
`pingdotgg/t3code` repository: its README describes it as a GUI for Codex,
Claude, and OpenCode and tells users to install/authenticate the OpenCode CLI.
The repository itself contains the production `OpenCodeDriver` and
`OpenCodeAdapter`, so it is stronger evidence than the earlier third-party fork
linked from the feature-request issue. Sources:
[README](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/README.md),
[driver](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Drivers/OpenCodeDriver.ts),
[adapter](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts).

The central finding is simple: **T3 does not drive OpenCode through terminal
output or an NDJSON subprocess protocol. It starts (or connects to) an OpenCode
HTTP server and uses `@opencode-ai/sdk/v2` for commands and the subscribed event
stream.** In Laplus, this belongs as a third implementation of the existing
Rust `session::Driver` seam, alongside Claude and Codex. Most UI and contract
vocabulary is already present; the missing work is Rust settings/probing,
HTTP/SSE transport, event normalization, and durable upstream-session binding.

## Architecture and seam

T3's provider architecture separates three concerns:

1. A `ProviderDriver` describes the driver kind, config schema, snapshot/probe,
   runtime adapter, and optional text-generation service.
2. Configured provider instances are built in child scopes and keyed by
   `ProviderInstanceId`.
3. Orchestration routes by thread through `ProviderService`; it does not know
   which provider is behind the thread. Provider runtime events are ingested
   back into the shared command/event model.

That organization is documented by T3 and embodied in the OpenCode driver's
small composition layer. Sources:
[provider architecture](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/docs/internals/providers.md),
[`ProviderDriver.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/ProviderDriver.ts),
[`OpenCodeDriver.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Drivers/OpenCodeDriver.ts).

Laplus has already crossed the corresponding structural bridge for Codex:
[`session.rs`](../../server/crates/laplus-server/src/session.rs) owns a generic
`Driver` trait and the shared conversation loop, while
[`turn.rs`](../../server/crates/laplus-server/src/turn.rs) and
[`codex.rs`](../../server/crates/laplus-server/src/codex.rs) implement the two
transports. OpenCode should fit this seam; it should not introduce a parallel
orchestration path.

## Process and protocol lifecycle

T3 supports two connection modes:

- With `serverUrl`, it connects to an externally managed server and does not
  own that server's lifetime. An optional password becomes HTTP Basic auth with
  username `opencode`.
- Without `serverUrl`, it picks a free loopback port and spawns
  `opencode serve --hostname=127.0.0.1 --port=<port>`. It waits up to 30 seconds
  for stdout beginning `opencode server listening ... on <URL>`. The child is
  detached on non-Windows platforms; closing its Effect scope sends SIGTERM,
  waits one second, then SIGKILLs the process group. T3 sets
  `OPENCODE_CONFIG_CONTENT={}` for its owned server.

It then creates an SDK v2 client with `{ baseUrl, directory, throwOnError: true
}` and starts one `event.subscribe` stream per active T3 thread. Each T3 thread
has its own session context and, when locally managed, its own `opencode serve`
child. Scope closure aborts the subscription fetch, interrupts event/exit
fibers, and reaps the child. Sources:
[`opencodeRuntime.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts),
[`OpenCodeAdapter.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts),
[`apps/server/package.json`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/package.json).

This lifecycle matches Laplus's chosen “one agent process per conversation”
shape closely. The transport differs: Laplus must make `Driver::next`
cancel-safe over an HTTP event stream instead of a child's stdout. The safest
shape is a dedicated pump task that decodes SSE/SDK events into a Tokio channel;
`next` only receives one already-normalized event and performs no await after
dequeueing.

## Starting, resuming, and sending a turn

On session start T3:

1. connects/spawns the server and constructs a directory-bound client;
2. optionally registers T3's per-thread MCP endpoint with `mcp.add` (owned
   servers only);
3. reads a versioned cursor `{ schemaVersion: 1, sessionId }`;
4. calls `session.get` to re-adopt that OpenCode session;
5. starts fresh only on a structured 404/`NotFoundError`; transport, auth, and
   other failures propagate rather than silently erasing context;
6. if the resumed session belongs to a different working directory, calls
   `session.fork` into the requested directory to preserve history;
7. re-applies permissions with `session.update`, or creates a new session with
   `session.create`.

The OpenCode session ID is the entire durable continuation handle. T3 stores it
in the provider session's resume cursor and returns it again after each turn so
the orchestration store refreshes the binding. Source:
[`OpenCodeAdapter.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts).

Sending uses `session.promptAsync` with:

- `sessionID`;
- a model parsed from the required `provider/model` slug;
- optional `agent` and `variant` selections;
- text parts and attachment file parts represented as `file:` URLs.

If a prompt arrives while a turn is active, T3 treats it as steering: it queues
the prompt into the busy OpenCode session and reuses the active T3 turn ID.
Interrupt and stop both call `session.abort`; stop additionally closes the
session scope. Source:
[`OpenCodeAdapter.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts).

For Laplus, the durable cursor can use the same persisted per-thread provider
handle already used by Codex, but it must be tagged/versioned rather than stored
as an unqualified string. Resume must preserve T3's distinction between a
confirmed missing session and an unavailable server. CWD mismatch should be a
deliberate v1 decision: support `session.fork`, or refuse with an explicit
message; silently making an empty session is the bad outcome.

## Event mapping

T3 subscribes globally through the directory-bound client, then rejects every
event whose embedded `sessionID` is not the context's OpenCode session. It maps
the remaining events as follows. Source for all rows:
[`OpenCodeAdapter.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts).

| OpenCode event                              | T3 normalized event / effect                                           |
| ------------------------------------------- | ---------------------------------------------------------------------- |
| `session.updated`                           | `thread.metadata.updated` when title is present                        |
| `message.updated`                           | cache message role; replay cached parts if assistant role arrives late |
| `message.removed`                           | remove cached role                                                     |
| `message.part.delta`                        | `content.delta`, assistant or reasoning, deduplicated                  |
| `message.part.updated` text/reasoning       | merge cumulative text and emit only unseen suffix                      |
| `message.part.updated` tool                 | `item.started`, `item.updated`, or `item.completed`                    |
| `permission.asked` / `.replied`             | `request.opened` / `request.resolved`                                  |
| `question.asked` / `.replied` / `.rejected` | user-input requested/resolved                                          |
| `session.status` busy                       | session running                                                        |
| `session.status` retry                      | runtime warning                                                        |
| `session.status` idle                       | finish active turn as completed                                        |
| `session.error`                             | fail active turn, mark session error, emit runtime error               |

Two details are load-bearing:

- OpenCode may deliver role and part updates in either order, so T3 keeps
  `messageRoleById` and `partById` caches.
- It receives both true deltas and cumulative part updates. T3 tracks emitted
  text per part, takes a common prefix, and emits only the unseen suffix; it
  also refuses to shorten already-emitted text when an older cumulative update
  arrives.

Tool parts are classified heuristically by tool name into command execution,
file change, web search, MCP call, image view, collaboration-agent call, or a
generic dynamic tool. The raw tool state is retained in event data. Laplus's
existing folded tool vocabulary can accept command/file items immediately;
generic/MCP/web/question coverage should be checked against
[`providerRuntime.ts`](../../packages/contracts/src/providerRuntime.ts) before
the spike freezes a v1 subset.

## Permissions and questions

T3 maps Laplus-like runtime modes into an OpenCode `PermissionRuleset`:

- `full-access`: allow `*` on `*`;
- every other mode: default ask, explicitly ask for bash, edit, web fetch/search,
  code search, external directories, and doom-loop detection, while allowing
  the `question` capability so it can flow through the separate question UI.

An OpenCode permission request is cached by ID. `bash`, `read`, and `edit` map
to command/file-read/file-change approvals; unknown kinds remain visible as
unknown rather than being dropped. Replies map `accept -> once`,
`acceptForSession -> always`, and decline/cancel to `reject`, sent with
`permission.reply`. Question arrays are given stable derived IDs and answered
with `question.reply`. Sources:
[`opencodeRuntime.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts),
[`OpenCodeAdapter.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts),
[T3 permission-mode docs](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/docs/user/permission-modes.md).

Laplus already has `Driver::answer(ApprovalRequest, Reply)`, approval decisions,
and question UI. The OpenCode driver therefore needs an internal pending-request
table that retains whether an ID belongs to permission or question and the
original OpenCode question order. Do not infer this from request ID prefixes.

## Provider discovery and models

T3 supports `enabled`, `binaryPath`, `serverUrl`, `serverPassword`, and hidden
`customModels`. For a local server it checks `opencode --version` (minimum
accepted version is `1.14.19`); for an external server it skips the CLI version
probe. Inventory comes from `provider.list` plus `app.agents`; only connected
providers' models are exposed, with slugs `${provider.id}/${model.id}`. Model
variants and visible primary/all agents become model-selection options. T3 also
has a CLI inventory fallback based on `opencode models` and
`opencode agent list`. Sources:
[`OpenCodeProvider.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeProvider.ts),
[`opencodeRuntime.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts),
[`OpenCodeSettings`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/packages/contracts/src/settings.ts).

Laplus already inherited OpenCode's contract/UI metadata: the driver slug,
default-model tables, settings schema, icon, and settings form exist in
[`model.ts`](../../packages/contracts/src/model.ts),
[`settings.ts`](../../packages/contracts/src/settings.ts), and
[`providerDriverMeta.ts`](../../apps/web/src/components/settings/providerDriverMeta.ts).
The Rust server currently accepts and publishes only Claude/Codex settings and
registrations, so those OpenCode controls cannot become functional until Rust
config decoding and the provider snapshot land.

## Recommended implementation slices for Laplus

These are ordered tracer bullets, not a commitment to every T3 feature.

1. **Capture the real wire first.** Install/authenticate a pinned OpenCode CLI
   in a throwaway environment; capture health, session create, prompt, text and
   reasoning, command/edit tools, permission, question, interrupt, resume,
   missing resume, and cwd mismatch. Keep fixtures beside
   `server/fixtures/codex-app-server`. This resolves SSE framing and exact SDK
   JSON shapes before Rust types harden around them.
2. **Make OpenCode a registered/configurable provider.** Add Rust
   `OpenCodeSettings` (`enabled`, binary path, optional server URL/password,
   custom models), settings patch support, `DriverKind::OpenCode`, resolution,
   probe, and provider snapshots. Start with CLI inventory if implementing the
   HTTP client is not yet available at probe time; expose only connected models
   in `provider/model` form.
3. **Build a narrow HTTP/SSE protocol module.** Hand-model only captured v2 API
   requests/responses/events with serde, preserve unknown event types, and pin
   them with golden fixtures. Keep HTTP/auth/SSE framing out of `session.rs`.
4. **Land the text-turn tracer bullet.** Implement `session::Driver` with one
   owned `opencode serve` per conversation, `session.create/get`, an event pump
   channel, `promptAsync`, assistant/reasoning deltas, idle completion,
   interrupt/abort, and scoped cleanup. External `serverUrl` can follow if it
   complicates ownership/auth too much for this slice.
5. **Add tools and permissions.** Cache role/parts and emitted text; normalize
   tool lifecycle; translate runtime modes to OpenCode rules; retain pending
   permission requests and map once/always/reject. Unknown tools/events must be
   counted and non-fatal.
6. **Add questions and attachments.** Map OpenCode multi-question requests to
   the existing user-input path and local attachments to file URLs.
7. **Make continuation durable.** Persist a versioned OpenCode session cursor,
   re-adopt only on success, fresh-start only on structured 404, and fork on cwd
   change. Verify server restart and missing-session behavior end to end.
8. **Drive the UI.** Select OpenCode, run text/tool/permission/question flows,
   interrupt, restart Laplus, and continue. The repository requires window-level
   verification for user-visible changes.

## Decisions to settle before a spec

- Whether v1 includes an external server URL/password or only owned local
  servers. The former adds authentication, reachability semantics, and an
  explicitly unowned lifetime.
- Whether to depend on an OpenCode Rust client, generate from its OpenAPI
  description, or hand-model the captured subset. Laplus's Codex precedent
  favors a small handwritten protocol with fixtures.
- Whether steering an active turn is a supported Laplus behavior. T3 queues it
  into the same OpenCode turn; Laplus's shared loop may currently serialize
  follow-ups differently.
- Whether MCP registration is part of v1. T3 injects its own per-thread MCP
  endpoint only into owned servers; it is not necessary for the basic coding
  loop.

## Transport decision research — 2026-08-01

### Decision

For parity with the pinned T3 implementation, Laplus should build a **small,
handwritten Rust HTTP/SSE client over `reqwest`, with handwritten serde wire
types and captured golden fixtures**. Treat OpenCode's generated OpenAPI 3.1
document as the authoritative reference and upgrade oracle, but do not check a
whole generated Rust SDK into Laplus and do not depend on a third-party
OpenCode crate.

This is a deliberately narrow client, not an untyped collection of ad-hoc
requests. Its public surface should name the operations the driver actually
needs; its transport layer should centralize base URL, directory query/header
encoding, Basic auth, status/error decoding, JSON requests, and SSE framing;
and its event envelope should preserve unknown event payloads. The OpenCode
driver then owns the stateful translation from upstream events to Laplus
events.

### What parity actually requires

At T3 commit `0ad91b6e`, the server declares `@opencode-ai/sdk ^1.3.15`, while
its lockfile resolves that range to `1.15.13`; the adapter imports its `/v2`
client. T3 uses OpenCode's official generated JavaScript SDK, not handwritten
HTTP. The adapter does not use anything resembling
the SDK's full API. It needs health/inventory (`global.health`,
`provider.list`, `app.agents`), session create/get/update/fork/promptAsync/
abort/revert, MCP add, permission reply, question reply/reject, and one
directory-scoped event subscription. The event pump filters by session and
maps session/message/part/status/error, permission, and question events. It
also explicitly aborts the subscription fetch when its session scope closes.
Sources: [T3 server manifest](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/package.json),
[T3 lockfile](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/pnpm-lock.yaml),
[T3 OpenCode adapter](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts),
[T3 OpenCode runtime](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts).

The resolved official SDK exposes those operations as ordinary REST routes — for
example session create/get/update/fork/abort/prompt-async/revert — and SSE for
event subscription. It is therefore feasible to model the parity subset
without reproducing SDK runtime behavior. Source:
[OpenCode 1.15.13 generated v2 client](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/sdk/js/src/v2/gen/sdk.gen.ts).

### Why OpenAPI generation is not the implementation boundary

An authoritative OpenAPI description does exist. The OpenCode server builds an
OpenAPI 3.1.1 document from the same Hono route metadata that serves the API;
`GET /doc` exposes it, and `Server.openapi()` generates it programmatically.
The official JavaScript SDK build runs `opencode generate`, writes
`openapi.json`, passes it to `@hey-api/openapi-ts`, generates the v2 client,
then deletes the temporary spec. Sources:
[OpenCode 1.15.13 server](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/opencode/src/server/server.ts),
[SDK generation script](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/sdk/js/script/build.ts),
[SDK package manifest](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/sdk/js/package.json).

The official build script is also evidence against blind independent
generation: after `@hey-api/openapi-ts` runs, OpenCode applies manual patches
for duplicate session event variants, numeric history parameters, and an SSE
generator typing problem. A Rust generator would need its own reviewed fixes;
“generated from `/doc`” is not by itself parity with the SDK T3 actually uses.
Source: [OpenCode 1.15.13 SDK generation script](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/sdk/js/script/build.ts).

That makes the spec excellent conformance evidence, but full-client generation
is a poor fit for this driver:

- The generated v2 TypeScript client is roughly four thousand lines before its
  generated types and fetch runtime; its API covers projects, PTYs, files,
  auth, providers, TUI control, worktrees, experimental resources, global
  administration, and many other routes Laplus will not call. The parity
  subset is only a small fraction of it. Source:
  [generated v2 client](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/sdk/js/src/v2/gen/sdk.gen.ts).
- OpenCode's source schemas are authoritative, but the generated artifact is
  tied to an OpenCode release. Generating at Laplus build time from a running
  user's `/doc` would make builds non-reproducible; checking in a full generated
  client would turn unrelated upstream schema movement into a large review
  surface. Pinning a spec while handwriting only used operations gives the same
  compatibility boundary with much less code.
- Generation does not solve the difficult behavior. T3's role/part
  reordering, cumulative-versus-delta deduplication, pending permission and
  question correlation, session filtering, idle completion, and resume/fork
  rules live in its adapter rather than in the generated SDK. Laplus must test
  those itself regardless of how request structs are produced. Source:
  [T3 OpenCode adapter](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts).

A useful guardrail is to retain the pinned OpenAPI JSON as test input (or fetch
it only in an explicit maintainer update command), and add a conformance test
that the handwritten method/path/body assumptions still exist. It should not
be a networked build script.

### Why not a Rust OpenCode SDK

OpenCode's official integration guidance describes its generated TypeScript
client for network use; its newer v2 documentation says non-Effect consumers
should run OpenCode as a server and use the TypeScript client, and warns that
the v2 API/client are still beta. It does not identify an official Rust client.
Sources: [official OpenCode client guide](https://opencode.ai/v2/docs/build/client),
[official SDK guide](https://opencode.ai/v2/docs/build/sdk),
[v1-to-v2 migration guide](https://opencode.ai/v2/docs/migrate-v1).

Several community Rust crates now exist, but none is a safe dependency for
Laplus's pinned T3 parity boundary:

- `opencode-sdk` 0.1.7 advertises broad HTTP/SSE support, but its own package
  documentation says its managed-server support is Unix-only and Windows
  compilation fails. Laplus ships on Windows, so adopting the crate's full
  lifecycle path is disqualifying; using only part of a young 0.1 crate would
  still couple Laplus to a separately modeled schema without removing its own
  event adapter. Source:
  [`opencode-sdk` package documentation](https://docs.rs/crate/opencode-sdk/0.1.7).
- `opencode-client-sdk` exposes an `opencode` Rust crate and claims alignment
  with the official JavaScript SDK, while `opencode-sdk-rs` exposes another
  independently maintained client. Neither is published or endorsed by the
  OpenCode repository, and their current versions target a moving current API,
  not specifically the `1.15.13` SDK behavior resolved by the pinned T3 commit.
  Sources: [`opencode-client-sdk`](https://docs.rs/crate/opencode-client-sdk/latest),
  [`opencode-sdk-rs`](https://docs.rs/crate/opencode-sdk-rs/latest).
- A generated-looking `opencode_sdk_rust` crate also exists, but generation by
  itself does not establish that its generator input, release cadence, auth,
  or SSE cancellation semantics match the pinned server. Source:
  [`opencode_sdk_rust`](https://docs.rs/crate/opencode_sdk_rust/latest).

These crates are useful comparative implementations and may supply fixture
ideas, but adopting one transfers compatibility control to an additional
project. The official OpenAPI document is a better upstream authority.

### HTTP, auth, SSE cancellation, and compatibility details

- OpenCode applies HTTP Basic auth to server routes only when
  `OPENCODE_SERVER_PASSWORD` is set; the default username is `opencode` unless
  overridden. A handwritten client can express this directly with reqwest's
  Basic auth support. Source:
  [OpenCode 1.15.13 server middleware](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/opencode/src/server/server.ts).
- The official v2 client disables its ordinary request timeout and carries the
  working directory in `x-opencode-directory`; for GET/HEAD it rewrites that
  value into a `directory` query parameter. Laplus should cover these exact
  rules with request-capture tests instead of assuming a generic generated
  Rust client matches them. Source:
  [OpenCode 1.15.13 v2 client wrapper](https://github.com/anomalyco/opencode/blob/v1.15.13/packages/sdk/js/src/v2/client.ts).
- T3 gives the event subscription its own abort controller and aborts it during
  scoped shutdown. In Rust, the equivalent should be an owned pump task reading
  the reqwest byte stream into a bounded Tokio channel, plus an explicit
  cancellation token/task abort and awaited cleanup. Dropping a response body
  alone is insufficient as the lifecycle contract; cancellation must be tested
  against a fixture server that holds the SSE connection open. Source:
  [T3 OpenCode adapter lifecycle](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts).
- Decode SSE framing separately from JSON event decoding. Preserve unknown
  event `type` plus raw `properties`, ignore it operationally, and record/count
  it. This allows a newer compatible OpenCode server to add events without
  killing a turn, while malformed frames and changed shapes of required events
  remain visible failures.

### Fit with Laplus

Laplus currently has Tokio, serde, and serde_json but no HTTP client in
`laplus-server`; adding reqwest (with one intentionally selected TLS backend)
is unavoidable whether HTTP calls come from handwritten code, generated code,
or a community SDK. Sources: [workspace dependencies](../../server/Cargo.toml),
[server dependencies](../../server/crates/laplus-server/Cargo.toml).

The existing Codex driver is the useful architectural precedent: it owns a
small provider-specific protocol implementation, pumps asynchronous protocol
input into a Tokio channel, exposes normalized events through `session::Driver`,
and tests the wire against committed fixtures. OpenCode should copy that
ownership boundary, substituting HTTP requests and SSE framing for JSON-RPC
over stdio. Sources: [`codex.rs`](../../server/crates/laplus-server/src/codex.rs),
[`session.rs`](../../server/crates/laplus-server/src/session.rs),
[`codex-app-server` fixtures](../../server/fixtures/codex-app-server/README.md).

### Rejected alternatives and revisit trigger

| Option                                                       | Decision       | Reason                                                                                                                                                               |
| ------------------------------------------------------------ | -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Handwritten reqwest + serde + fixture-backed SSE             | **Choose**     | Small parity surface, Windows-compatible, explicit cancellation/auth, unknown-event tolerance, and closest to Laplus's existing protocol ownership.                  |
| Generate a complete Rust client from `/doc`                  | Reject for now | Authoritative input, but excessive unrelated footprint and churn; adapter behavior still must be handwritten.                                                        |
| Depend on a community Rust SDK                               | Reject for now | No official Rust SDK, multiple competing young crates, version skew from T3's resolved 1.15.13 SDK, and at least one explicit Windows limitation.                    |
| Embed or sidecar JavaScript solely to reuse the official SDK | Reject         | Adds a Node/Bun runtime and IPC layer to a Rust server while preserving all adapter complexity; T3 can use the SDK cheaply because its server is already TypeScript. |
| Parse `opencode run --format json`                           | Reject         | Does not match T3's server behavior or its session, permission, question, MCP, external-server, resume/fork, and steering surface.                                   |

Revisit generation or an SDK only if OpenCode publishes and supports an
official Rust client with Windows support **and** a version compatible with the
chosen OpenCode minimum, or if Laplus expands from this driver subset into a
general OpenCode API client. Until then, upgrade by pinning a new CLI/spec,
diffing the used endpoints and event fixtures, and changing the narrow client
deliberately.

## Live protocol prototype — OpenCode 1.18.10

Run on 2026-08-01 against the installed
`/home/ubuntu/.opencode/bin/opencode` in disposable directories under `/tmp`,
with `OPENCODE_CONFIG_CONTENT={}` and an unsecured loopback-only server. No
Laplus product code was involved. The server reported version `1.18.10`; its
served `/doc` OpenAPI document was 478,613 bytes.

The prototype validates the T3-derived transport shape:

- `GET /global/health` returned `{"healthy":true,"version":"1.18.10"}`.
- `GET /event?directory=...` returned ordinary SSE `data:` records separated
  by a blank line, beginning with `server.connected` and periodic heartbeats.
- `POST /session?directory=...` returned a `ses_...` session and emitted
  `session.created`.
- `POST /session/{id}/prompt_async?directory=...` returned `204` immediately.
  A free OpenCode model then emitted user/assistant `message.updated`, part
  events, busy/idle status, title updates, and the requested final text.
- `POST /session/{id}/abort?directory=...` returned JSON `true`; the assistant
  `message.updated` then carried a structured `MessageAbortedError`, followed
  by both idle signals, and the status catalogue was empty immediately
  afterwards.
- A missing session returned HTTP 404 with the structured body
  `{"name":"NotFoundError","data":{"message":"Session not found: ..."}}`,
  validating the fail-closed resume discriminator.

The live version also revealed two compatibility requirements not visible from
implementing the pinned T3 adapter literally:

1. Streaming text and reasoning arrive through `message.part.delta` records
   whose properties are `sessionID`, `messageID`, `partID`, `field`, and
   `delta`; a final cumulative `message.part.updated` follows. OpenCode also
   emits `session.idle` after `session.status: idle`. The narrow decoder should
   handle the delta event directly, use the final update as reconciliation,
   accept either idle signal idempotently, and keep unrelated new events
   observable but non-fatal. Otherwise Laplus would show no incremental output
   on current OpenCode even though the pinned T3 behavior appears compatible.
2. On 1.18.10, calling `session.fork` through a client bound to a different
   `directory` preserved history but returned the fork in the _original_
   directory. The current server's
   `POST /experimental/control-plane/move-session` moved that fork to the
   requested directory and preserved its token/history metadata. Therefore the
   resume algorithm must verify the fork's returned canonical directory. For
   current servers it needs fork-then-move; it must never assume the SDK query
   parameter achieved the move merely because the fork request succeeded.

The 1.18.10 OpenAPI document additionally declares both the original
`question.*` and newer `question.v2.*` event families. T3 parity requires the
original family; the v2 family should initially follow the unknown-event rule
unless its reply semantics are implemented and fixture-backed deliberately.
The prototype did not force permission or question interactions, because the
transport and schemas were observable without manufacturing a user-facing
request.

## Primary-source index

- T3 Code repository at researched commit:
  <https://github.com/pingdotgg/t3code/tree/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62>
- T3 provider architecture:
  <https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/docs/internals/providers.md>
- T3 OpenCode driver:
  <https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Drivers/OpenCodeDriver.ts>
- T3 OpenCode runtime/process wrapper:
  <https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts>
- T3 OpenCode adapter:
  <https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts>
- T3 OpenCode probe/model inventory:
  <https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeProvider.ts>
- Official OpenCode server/SDK documentation:
  <https://opencode.ai/docs/server/>, <https://opencode.ai/docs/sdk/>
