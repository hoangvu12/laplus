# Upstream research — what a `codex` driver would cost

Written 2026-07-30, before any spec. The question this answers: **how does
`pingdotgg/t3code` drive Codex, and what would laplus have to build to drive it
too?**

Read over the network, not from a checkout — there is still no `upstream` remote
(`server/CLAUDE.md`). Upstream is `pingdotgg/t3code` at `df78cda8bf9c`
(2026-07-30); the Codex protocol is `openai/codex` at release `rust-v0.146.0`
(2026-07-29). Sizes and line numbers below are those two trees'; re-derive with
the commands in [Method](#method) before trusting them.

**Nothing here was run.** `codex` is not installed on the machine this was
written on, so every claim about the wire is read off upstream's source and
OpenAI's schema, never off a live process. The one thing that would change the
estimate is a spike, and it is the last section.

## Summary

1. **The protocol is bigger than `claude`'s and better specified.** Codex speaks
   JSON-RPC 2.0 over stdio (`codex app-server`), bidirectional, with ~50
   methods and notifications in the surface upstream touches. OpenAI publishes
   JSON Schema for all of it — 240 files under
   `codex-rs/app-server-protocol/schema/json/v2`. Where the `claude` wire had to
   be discovered by a spike, this one can be read.
2. **The hard half is not Codex.** laplus is single-provider by construction:
   one constant is both driver kind and instance id, and the live session
   payload publishes it. The contract, the settings schema and the bundled UI
   already speak `codex`. What is missing is server plumbing, and Codex is the
   thing that forces it open.
3. **Upstream is a specification, not a source of code.** It is Effect TS to the
   bone. The reusable artifact is OpenAI's schema, not upstream's driver — and
   the Rust crates that would have handed us the wire types are published but
   seven months stale. See [The Rust angle](#the-rust-angle).

## What upstream ships

Non-test implementation, ~170 KB across nine files:

| File (`apps/server/src/`)                | Size  | Lines | What it is                               |
| ---------------------------------------- | ----- | ----- | ---------------------------------------- |
| `provider/Layers/CodexAdapter.ts`        | 54 KB | 1729  | app-server events → contract events      |
| `provider/Layers/CodexSessionRuntime.ts` | 49 KB | 1423  | the process, threads, turns, approvals   |
| `provider/Layers/CodexProvider.ts`       | 19 KB | 615   | snapshot, status, `model/list`           |
| `textGeneration/CodexTextGeneration.ts`  | 14 KB | 411   | commit/PR/branch titles via `codex exec` |
| `provider/Drivers/CodexHomeLayout.ts`    | 13 KB | 422   | `CODEX_HOME` and the shadow home         |
| `provider/CodexDeveloperInstructions.ts` | 10 KB | —     | the system prompt                        |
| `provider/Drivers/CodexDriver.ts`        | 9 KB  | 213   | the registry entry                       |
| `provider/Layers/codexLaunchArgs.ts`     | 2 KB  | 48    | argv for `app-server` and for `exec`     |
| `codexModelOptions.ts`                   | .5 KB | 14    | service-tier option                      |

Plus ~83 KB of tests beside them, and a client package of its own:
`packages/effect-codex-app-server` — ~45 KB hand-written (`protocol.ts` 423
lines, `client.ts` 269, `errors.ts`, `_internal/`) around **1.7 MB of generated
schema**, code-genned by `scripts/generate.ts` from the JSON Schema in
`openai/codex`. That generator pins an upstream ref (`generate.ts:20`) and
fetches `schema/json`, `schema/json/v1` and `schema/json/v2` through the GitHub
API (`:23`, `:532-534`).

For scale, upstream's **Claude** driver is ~180 KB non-test
(`ClaudeAdapter.ts` alone is 128 KB). Codex is not a small addition there
either; it is a peer.

## The protocol, against the one laplus already drives

|           | `claude` — what laplus drives today                   | `codex app-server`                                                        |
| --------- | ----------------------------------------------------- | ------------------------------------------------------------------------- |
| Wire      | NDJSON `stream-json`, one child per session           | JSON-RPC 2.0 over stdio, bidirectional                                    |
| Surface   | ~10 event types, `protocol.rs`                        | ~50 methods and notifications, 240 v2 schema files                        |
| Session   | the child **is** the conversation                     | `thread/start` / `thread/resume` against rollout files under `CODEX_HOME` |
| Approvals | `control_request` on stdout, answered on stdin        | server→client JSON-RPC requests                                           |
| Interrupt | `control_request` on stdin                            | `turn/interrupt`                                                          |
| Models    | a table compiled in (`provider.rs:BUILT_IN_MODELS`)   | `model/list` at runtime (`CodexProvider.ts:293`) — _easier_               |
| Account   | not read at all (`agent.rs` header, "Authentication") | `account/read` (`CodexProvider.ts:382`), with rate limits pushed          |

The methods upstream actually names, grepped out of its four Codex files:
`thread/{start,resume,read,rollback,archive,unarchive,closed,compacted}`,
`turn/{start,started,completed,aborted,interrupt,plan/updated,diff/updated}`,
`item/{started,completed}` with per-kind deltas
(`agentMessage/delta`, `reasoning/textDelta`, `commandExecution/outputDelta`,
`fileChange/outputDelta`), the three approval requests
(`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`,
`item/fileRead/requestApproval` — `CodexAdapter.ts:297-306`),
`item/tool/requestUserInput`, `model/list`, `account/read` and
`thread/tokenUsage/updated`. A v1 does not need all of it, and how much it does
need is exactly what the spike would settle.

### The lifetime model is the sharpest difference

`turn.rs`'s header states laplus's shape: _"One long-lived driver, not one per
turn"_ — a session is a task owning one `claude`, and the child stays because
`--input-format stream-json` means it reads turns until stdin closes.

Codex's shape is one app-server process per **provider instance**, multiplexing
threads inside it: `CodexSessionRuntime.ts:475-491` opens a conversation with
`thread/start`, or `thread/resume` falling back to a fresh start when the
rollout has gone (`:479-491`, with the recoverable-error snippets listed at the
top of the file). Threads outlive processes; rollouts on disk are the
continuity.

laplus can dodge this in a v1 by spawning one app-server per session and using
one thread inside it. Wasteful, and it throws away Codex's own continuity story,
but it preserves the shape `turn.rs` already has and is the smaller first cut.
Choosing it is a decision that wants writing down, not one to make by accident.

## The Rust angle

`codex-app-server-protocol` and `codex-protocol` **are published on crates.io**,
Apache-2.0. That would hand us the wire types for free — except both sit at
**0.63.0, published 2025-12-11 and never updated**, while the CLI shipped
`rust-v0.146.0` on 2026-07-29. Seven months of releases behind the wire a
developer's `codex` actually speaks. Not usable as a dependency.

Three ways to get types, in the order this repository's grain suggests:

1. **Hand-write serde structs for the subset used.** What `protocol.rs` did for
   `claude`, for the same reason its header gives: the blast radius of a format
   change is one pure file, and a golden-file test says it shifted before any
   server code notices. Costs a decision per field about what to model and what
   to let `Unknown` absorb.
2. **Generate from `schema/json/v2`.** Same source of truth upstream's generator
   uses, so the same 240 files, mechanically. Buys coverage, costs a build step
   and a vendored blob nobody reads.
3. **Git-depend on `openai/codex`.** Current by construction, but it drags a
   large Cargo workspace into a build that is currently ours.

`Unknown` is load-bearing either way — `protocol.rs:26-29` argues it for the
`claude` envelope and the argument transfers unchanged.

## What laplus already has, and what it does not

**The contract is ready.** `packages/contracts/src/providerInstance.ts` makes
`ProviderDriverKind` a deliberately open slug so a fork can add a driver
(`:18-31`), `settings.ts:185-237` already declares `CodexSettings` — binary
path, `CODEX_HOME`, shadow home, launch args — and `:431` mounts it at
`settings.providers.codex`. `model.ts:130` names the driver kind and carries
Codex defaults and slug aliases. The UI is upstream's, so the model picker, the
provider settings form and the add-instance wizard already render Codex.

**ADR-0001 predicted this driver.** _"A second driver would bring its own
encoder; it would not bring another copy of the decoder."_ `crate::settling` is
already shared and already mirrored from upstream; what a Codex driver adds is
its own `Ending`-equivalent, not a second settling table.

**What is single-provider by construction:**

| Where                       | What it says                                                              |
| --------------------------- | ------------------------------------------------------------------------- |
| `provider.rs:77`            | `INSTANCE_ID = "claudeAgent"` — routing key _and_ driver slug, one const  |
| `provider.rs:837-838`       | the snapshot sets `instance_id` and `driver` from it                      |
| `threads/fold.rs:1071-1072` | the live session publishes it as `providerName`/`providerInstanceId`      |
| `settings.rs:241`, `:326`   | only `providers.claudeAgent` decodes; anything else is refused (ADR-0009) |
| `config.rs:535`             | the default model selection names it                                      |
| `catalogue.rs`              | commands come from a `claude` handshake; skills from `.claude/` on disk   |

None of that is wrong today — `provider.rs:68-77` says plainly that the slug is
open in the contract and closed in practice because the UI keys tables off it.
It is simply the surface a second driver has to move.

**The test story is the hidden cost.** `tests/harness/agent.rs` fakes `claude`
with a shell script that echoes a version string, because the suite has to run
offline, for free, on a machine that never had the agent installed. A fake
app-server has to speak JSON-RPC and answer requests, so it is a fixture binary
rather than three lines of `sh` — and `fixtures/claude-cli/` (43 files) has no
Codex counterpart, nor does `tools/wire-capture/`, which records the _socket_
wire rather than an agent's.

## What each scope would cost

**Multi-provider plumbing, on its own.** A provider registry in place of the one
constant, an instance id persisted per thread and published from the session
rather than hardcoded, `providers.codex` accepted by `settings.rs`, and turn
dispatch routed by instance. Small-to-medium and mostly mechanical — on the
order of hundreds of lines across ~10 files — but it is the part that churns
existing tests, and it buys nothing visible until a second driver exists.

**A Codex driver v1** — one instance, one app-server per session, text turns,
streaming deltas, command and file-change items, the three approvals, interrupt,
`model/list`, resume: the `claude` equivalent of that work is 1662 lines in
`protocol.rs`, 513 in `agent.rs`, 2465 in `turn.rs`, 413 in `catalogue.rs` and
908 in `provider.rs` before their test modules — 5,961 lines of non-test Rust,
with roughly as much test beside it. Codex's is not smaller: the surface is
wider, though the discovery is already done. Weeks of one person's work, not a
weekend, and the `claude` path needed `spike-claude-protocol/` before any of it
landed.

**What v1 can leave out.** Multi-account (`CodexHomeLayout.ts` exists because
Codex supports several logins; one instance needs none of it), text generation
(`codex exec` for commit messages — laplus has no such surface), realtime, apps,
and the archive/rollback half of the thread API.

## The spike this cannot replace

A Codex analogue of `spike-claude-protocol/`, roughly a day:
`npm i -g @openai/codex`, drive `codex app-server` by hand over stdio, and
capture `initialize` → `thread/start` → `turn/start` → deltas → an approval →
`turn/completed`. It answers the two questions this document reasons about
rather than knows:

1. Is one app-server per session acceptable in practice, or does resume-by-
   rollout make the per-instance shape mandatory?
2. How much of the 240-file schema does a v1 actually have to decode?

Everything above is read off source and schema. Until that spike runs, treat the
lifetime decision as open.

## Method

```sh
# upstream tree, sizes and paths
curl -s "https://api.github.com/repos/pingdotgg/t3code/git/trees/HEAD?recursive=1" \
  | python3 -c "import json,sys;[print(e['size'],e['path']) for e in json.load(sys.stdin)['tree'] if 'codex' in e['path'].lower() and e['type']=='blob']"

# the Codex schema, and how many files it is
curl -s "https://api.github.com/repos/openai/codex/contents/codex-rs/app-server-protocol/schema/json/v2" \
  | python3 -c "import json,sys;print(len(json.load(sys.stdin)))"

# the published crates, and how stale
curl -s -A ua "https://crates.io/api/v1/crates/codex-app-server-protocol" \
  | python3 -c "import json,sys;c=json.load(sys.stdin)['crate'];print(c['max_version'],c['updated_at'])"
curl -s "https://api.github.com/repos/openai/codex/releases/latest" \
  | python3 -c "import json,sys;d=json.load(sys.stdin);print(d['tag_name'],d['published_at'])"

# laplus's own single-provider surface
rg -n 'claudeAgent|INSTANCE_ID' server/crates/laplus-server/src --glob '*.rs'
```

## Sources

Upstream paths are relative to `pingdotgg/t3code@df78cda8bf9c`, schema paths to
`openai/codex@rust-v0.146.0`.

| What                          | Where                                                                                     |
| ----------------------------- | ----------------------------------------------------------------------------------------- |
| The driver registration       | `apps/server/src/provider/Drivers/CodexDriver.ts:62`, `:108-213`                          |
| `app-server` argv             | `apps/server/src/provider/Layers/codexLaunchArgs.ts:13-16`                                |
| Opening a conversation        | `apps/server/src/provider/Layers/CodexSessionRuntime.ts:475-491`                          |
| Approval request kinds        | `apps/server/src/provider/Layers/CodexAdapter.ts:297-306`, `:540`                         |
| `model/list`, `account/read`  | `apps/server/src/provider/Layers/CodexProvider.ts:293`, `:382`                            |
| Multi-account home layout     | `apps/server/src/provider/Drivers/CodexHomeLayout.ts`                                     |
| The generated client          | `packages/effect-codex-app-server/`, generator at `scripts/generate.ts:20-23`, `:532-534` |
| User-facing account guide     | `docs/user/providers-codex.md`                                                            |
| Protocol schema (v2)          | `codex-rs/app-server-protocol/schema/json/v2/` — 240 files                                |
| Published Rust types          | crates.io `codex-app-server-protocol` 0.63.0, `codex-protocol` 0.63.0                     |
| The contract's open slug      | this repo, `packages/contracts/src/providerInstance.ts:18-31`                             |
| Codex settings, already there | this repo, `packages/contracts/src/settings.ts:185-237`, `:431`                           |
| Codex model defaults          | this repo, `packages/contracts/src/model.ts:130`, `:169-176`                              |
| The one constant              | this repo, `server/crates/laplus-server/src/provider.rs:68-77`                            |
| The hardcoded session         | this repo, `server/crates/laplus-server/src/threads/fold.rs:1071`                         |
| The session's lifetime rule   | this repo, `server/crates/laplus-server/src/turn.rs` header                               |
| The per-provider encoder      | this repo, `server/docs/adr/0001-…md`                                                     |
| The fake agent                | this repo, `server/crates/laplus-server/tests/harness/agent.rs`                           |
