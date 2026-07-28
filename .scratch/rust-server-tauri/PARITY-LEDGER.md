# Parity ledger — laplus against t3code

**Date:** 2026-07-28 · **Against:** `main` @ `04ef5b148`

What this is: one place to see what the Rust server and the shell do, what they
do not, and which of the "do not"s are _bugs_, which are _unbuilt_, and which are
_never going to happen_. Derived by walking three sources against each other:

- the contract — `packages/contracts/src/rpc.ts` (`WS_METHODS`) and
  `orchestration.ts` (`ClientOrchestrationCommand`), which is the whole
  vocabulary the UI can speak;
- the UI — `apps/web` and `packages/client-runtime`, which is what it _does_
  speak, and where;
- the server — `server/crates/laplus-server/src/`, which is what answers.

The reference TypeScript server is not in the working tree (sparse-checkout) but
is in the object store: `git show HEAD:apps/server/src/...` reads it.

Ticket state at time of writing: **34 of 38 done**, 1 `ready-for-human` (24), 3
`needs-triage` (35, 37, 38). **The tickets are not the source here** — sections 0
to 3 are the contract surface, and **section 7 is the reference server read
directly**, which is where the gaps the tickets do not know about are.

Method count, for scale: the contract declares **71 WS methods**; laplus
implements **26**.

---

## This ledger is now tracked. Read the tickets, not this file, to pick work

Triaged 2026-07-28 into **tickets 39–70** under `issues/`. The findings below are
the evidence and the reasoning; the tickets are the work, and they carry the
acceptance criteria. Where the two disagree, the ticket is newer.

| Finding       | Ticket                                                                    | State            |
| ------------- | ------------------------------------------------------------------------- | ---------------- |
| S2            | 39 — a refusal the client cannot decode                                   | **done**         |
| R1            | 40 — the context meter is empty because six fields are undeclared         | ready-for-agent  |
| M1 (§0)       | 41 — changing the model mid-thread stops the next message                 | ready-for-agent  |
| M4 + S1       | 42 — archive fails on every row of the sidebar                            | ready-for-agent  |
| R5, R5b       | 43 — hooks, thinking counts and status lines folded to nothing            | ready-for-agent  |
| R19           | 44 — the drift counter cannot see what the CLI already does               | ready-for-agent  |
| S4            | 45 — three provider rows spin for ever                                    | ready-for-agent  |
| M18           | 46 — two subscriptions this server does not answer                        | ready-for-agent  |
| M2            | 47 — plan mode changes a label and nothing else                           | ready-for-agent  |
| M8 + S5 (R10) | 48 — a project cannot be renamed, its actions cannot be saved             | ready-for-agent  |
| M3            | 49 — a conversation cannot be deleted                                     | ready-for-agent  |
| M6            | 50 — a session cannot be stopped, only a turn interrupted                 | ready-for-agent  |
| R7            | 51 — the model picker offers no options the model supports                | ready-for-agent  |
| R9            | 52 — three query options the UI can set and this server drops             | ready-for-agent  |
| R6            | 53 — thinking is most of what a turn streams, and none of it is live      | ready-for-agent  |
| R2            | 54 — the todo list never appears                                          | ready-for-agent  |
| R3            | 55 — there is no proposed plan                                            | ready-for-agent  |
| R4            | 56 — a subagent is one collapsed row                                      | ready-for-agent  |
| R12           | 57 — the application has no runtime log                                   | ready-for-agent  |
| R11           | 58 — a fetch interval is configured and nothing ever fetches              | ready-for-agent  |
| R13           | 59 — file search is fragment matching where the picker wants fuzzy        | ready-for-agent  |
| M13           | 60 — the pull button has no server behind it                              | ready-for-agent  |
| M14           | 61 — the review surface has no backing                                    | ready-for-agent  |
| M5            | 62 — a checkpoint can be restored, but the agent cannot be rewound        | **needs-triage** |
| M7 + S6       | 63 — a pasted screenshot never reaches the agent                          | **needs-triage** |
| M12           | 64 — starting a conversation in a fresh worktree is refused               | ready-for-agent  |
| M15           | 65 — the diagnostics page is a settings page for an excluded feature      | **needs-triage** |
| M19           | 66 — no way to learn a newer `claude` or laplus exists                    | **needs-triage** |
| R8            | 67 — one control request is answered, and the agent gets no tools of ours | **needs-triage** |
| R14           | 68 — terminals do not survive a restart                                   | **needs-triage** |
| R15           | 69 — a retried command is refused as already existing                     | **needs-triage** |
| §5 workflows  | 70 — nine upstream workflows are firing, some trying to publish           | **needs-triage** |

**Deliberately not ticketed.** Section 3 (the spec's own exclusions), section 4
(the two kept differences), **M16, M17, M20, M21** and **R16** — all already
recorded here as decisions so they are not re-filed — and **R17**, which is the
_reason_ for 69, 42's snapshot question and `replayEvents` rather than a gap of
its own. ADR-0016 takes it.

**Existing tickets re-triaged in the same pass.** **38** moved
`needs-triage → ready-for-agent`: R18 disproves the cost objection it was parked
on, and its acceptance criteria are now written. **35** and **37** stay
`needs-triage` — both gained evidence, neither gained a decision — and 37 is now
recorded as **blocked by 57**, because the option it calls best is a log line and
there is no runtime log.

---

## 0. The one to fix first

**Changing the model or the permission mode inside an existing conversation
stops the next message from sending.**

Not a cosmetic gap. The path:

1. `ChatView.tsx:3337` `persistThreadSettingsForNextTurn` fires
   `thread.meta.update` / `thread.runtime-mode.set` / `thread.interaction-mode.set`
   — but **only when the value differs from what the server holds.**
2. The server refuses all three by name (`orchestration.rs:1219`,
   `Command not implemented by this server: …`).
3. `ChatView.tsx:4740` records that as `failure`, and `:4751`
   guards the turn start on `failure === null`. So `thread.turn.start` is never
   dispatched.
4. `:4808` puts the prompt back in the composer and calls `setThreadError`.

So: send a message, change the model, send another → the second one bounces with
an error and the text reappears in the box. Leave the pickers alone and
everything works, which is why the suite and every session so far missed it.

Same shape, second path: `ChatView.tsx:4717` auto-titles from the first message
via `thread.meta.update` when `isFirstMessage && isServerThread`. Every new
conversation normally starts as a **draft** (title arrives in
`bootstrap.createThread`), so this is likely unreachable today — but it is the
same guard and the same refusal, and it is one code path away.

**The cheapest honest fix** is to implement the three commands as writes to the
thread row that already holds those fields (`threads.rs:1161` already applies
`interaction_mode` from an `Option`, so the setter is nearly there). Refusing
them is only safe for commands the UI never sends.

**And the fix is smaller than it looks**, which the decider audit settled:
upstream's `thread.turn.start` reads the mode off the _thread_
(`decider.ts:777`), so on that side these commands are the only route by which a
mode reaches a turn. laplus reads it off the _command_, so the mode already works
— the three handlers need only write the row and publish their events so the
client's pre-flight stops failing. No ordering problem, no new state. See
`2026-07-28-decider-audit.md` §1.

---

## 1. Works today

Verified as implemented and covered by the suite. Ticket numbers in brackets.

| Area           | What works                                                                                                                                                  |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Transport      | WebSocket + Effect-RPC JSON framing, permissive loopback handshake, `server.getConfig`, config change feed [01–04]                                          |
| Projects       | add / remove / list via `project.create` + `project.delete`, persisted in SQLite [05]                                                                       |
| Files          | `filesystem.browse`, file tree, lazy expand, ignore-file semantics, read / search / write, external editor, live watcher [06–08, 25]                        |
| Agent          | binary resolution with a diagnostic naming what was looked for and where; model table gated on CLI version [09]                                             |
| Turns          | one turn streamed token-by-token, deltas reconciled against the buffered message, multi-turn continuity via `--resume`, transcript survives restart [10–11] |
| Tools          | tool calls and results in the work log, kinds mapped to upstream's labels (incl. MCP tool calls and subagents) [12]                                         |
| Permissions    | `can_use_tool` prompts, accept / acceptForSession / decline / cancel, clean return to the agent [13]                                                        |
| Questions      | `AskUserQuestion` renders as a question, not an Allow/Deny prompt [`f10e062a7`]                                                                             |
| Interrupt      | mid-turn stop, conversation stays usable [14]                                                                                                               |
| Resilience     | agent error reported in-conversation, child death survived, drift counters for unknown events, compaction, rate-limit standing [15]                         |
| Concurrency    | several conversations at once across projects [16]                                                                                                          |
| Terminal       | open / write / resize / attach / reattach / clear / restart / close, scrollback, VT questions replayed [17–18]                                              |
| Git            | working-tree status with coalesced live refresh, turn diffs and thread diffs from checkpoints, branch list / switch / create, `git init` [19–21]            |
| Settings       | `settings.json` + `keybindings.json`, per-field patches, rule→resolved compilation, merge-by-command, live to the UI without restart [22]                   |
| Shell          | frameless Tauri window, UI draws its own window controls, topbars drag, `isDesktopShell` gate [23, 27]                                                      |
| Composer menus | `/` commands from the `initialize` handshake, `$` skills from the filesystem [`a89885662`]                                                                  |
| Release        | NSIS installer, 5.05 MB download / 24.16 MB footprint / inside the 20–30 MB target, measured by the build [24]                                              |
| CI             | `cargo test --no-fail-fast` on `windows-latest` for `server/**` [36]                                                                                        |

---

## 2. Missing, and worth building

Ordered roughly by how much a developer using the app would feel it.

| #   | Gap                                                                                                       | Wire surface                                                                                        | What the developer sees                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| --- | --------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| M1  | **Model / mode change mid-thread**                                                                        | `thread.meta.update`, `thread.runtime-mode.set`, `thread.interaction-mode.set`                      | Section 0. The next message does not send.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| M2  | **Plan mode is a no-op**                                                                                  | —                                                                                                   | `interactionMode: "plan"` is stored and echoed but never reaches the CLI. Upstream calls `query.setPermissionMode("plan")` (`ClaudeAdapter.ts:3757`). laplus only maps `runtimeMode` → `--permission-mode` (`agent.rs:160`). The picker changes a label; the agent behaves identically.                                                                                                                                                                                                                                                                                                                                                                                                                   |
| M3  | **Delete a conversation**                                                                                 | `thread.delete`                                                                                     | Nothing happens; error toast. Reachable from `ChatView.tsx:1149` and the archived-threads panel. Upstream's cleanup is exactly two steps in order — stop the provider session, then close the thread's terminals with `deleteHistory` — each logged and skipped on failure rather than aborting the other. It does **not** delete checkpoint refs, so those leak on both sides and are not part of this.                                                                                                                                                                                                                                                                                                  |
| M4  | **Archive / unarchive** — ⚠ **most reachable broken control in the app**                                  | `thread.archive`, `thread.unarchive`, `orchestration.getArchivedShellSnapshot`                      | Both commands refused _and_ the snapshot method missing. Measured, not inferred: **every** sidebar row carries an `Archive` button, and pressing one puts "Failed to archive thread" on screen — one press from the default view. `/settings/archived` says "Could not load archived threads". See surface-walk §S1.                                                                                                                                                                                                                                                                                                                                                                                      |
| M5  | **Restore a checkpoint** — ⚠ **not ready work; needs triage**                                             | `thread.checkpoint.revert`                                                                          | Five of upstream's six revert steps are buildable today. The sixth — rolling the _agent's own memory_ back — needs `resumeSessionAt`, which is an **Agent SDK option the `claude` binary does not expose** (verified against `--help`, 2.1.220). laplus also discards the message `uuid` it would need. Three options, none free: truncate the CLI's own session JSONL, revert the filesystem only, or start a fresh session. See `2026-07-28-decider-audit.md` §3.                                                                                                                                                                                                                                       |
| M6  | **Stop a session**                                                                                        | `thread.session.stop`                                                                               | Interrupting a turn works; ending the agent process from the UI does not.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| M7  | **Attachments / pasted images**                                                                           | `assets.createUrl`                                                                                  | Accepted, then dropped on the way to the agent (`orchestration.rs:1009`). The developer pastes a screenshot and the agent never sees it. Silent, which is the worst version.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| M8  | **Rename / recolour a project**                                                                           | `project.meta.update`                                                                               | Refused.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| M9  | **Ticket 38 — a project's own slash commands**                                                            | —                                                                                                   | `.claude/commands/` is not scanned; only the CLI's built-ins reach the `/` menu. **The ticket's cost premise is now disproved** — see R18: the commands arrive free on the `init` of any real turn, and `InitEvent` already parses the field.                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| M10 | **Ticket 37 — the permission mode in force**                                                              | —                                                                                                   | The picker shows what was _asked for_; the CLI reports what is _in force_, and since `1053a3862` that report is no longer displayed anywhere.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |
| M11 | **Ticket 35 — the draft pane's request storm**                                                            | —                                                                                                   | Four requests a second for a thread that cannot exist. **Larger than filed and on a different transport:** measured 16 × `GET /api/orchestration/threads/<id>` → 404 in ~5s, **on boot, on the default route**, over ticket 31's own HTTP snapshot path — not only the socket subscription the ticket describes, and not only in a draft pane. It is also the whole of the console's 404 noise, which appears on every route. See surface-walk §S3.                                                                                                                                                                                                                                                       |
| M12 | **Worktree threads**                                                                                      | `vcs.createWorktree`, `vcs.removeWorktree`, `bootstrap.prepareWorktree`                             | Starting a conversation in a fresh worktree is refused (`orchestration.rs:1929`). The branch toolbar offers it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| M13 | **Pull**                                                                                                  | `vcs.pull`                                                                                          | Button, no server.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| M14 | **Diff preview**                                                                                          | `review.getDiffPreview`                                                                             | The review surface has no backing.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| M15 | **Diagnostics panel**                                                                                     | `server.getTraceDiagnostics`, `getProcessDiagnostics`, `getProcessResourceHistory`, `signalProcess` | `settings.diagnostics` is empty / errors — and see S2, which is the more fixable half.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| S2  | **The refusal does not decode, and the decoder's complaint is what the user sees** — ⚠ **new, and cheap** | four methods below                                                                                  | laplus answers `ServerMethodNotImplementedError`, a tag **not in those methods' declared error unions**, so the client fails to decode the _error_ and prints the schema mismatch on the page: `Expected { readonly "_tag": "EnvironmentAuthorizationError", ... }, got {"_tag":"ServerMethodNotImplementedError"…}`. Same rule `config.rs:266` already observes for `ConfigIssue` kinds, unobserved by `crate::rpc`. Answering with a tag each method declares turns four raw decoder errors into four honest empty states — worth doing whether or not the methods are ever built. Affects `server.getProcessDiagnostics`, `getProcessResourceHistory`, `getTraceDiagnostics`, `discoverSourceControl`. |
| S4  | **Three provider rows spin for ever**                                                                     | —                                                                                                   | `/settings/providers` lists Codex, Claude, Grok and OpenCode; the three laplus does not ship sit permanently on "Checking provider status — Waiting for the server to report installation and authentication details." Upstream has `provider/unavailableProviderSnapshot.ts` for exactly this — a shadow snapshot that satisfies the wire shape while signalling unavailability. Claude-only is a settled decision; three rows implying the others are _loading_ is not that decision showing through.                                                                                                                                                                                                   |
| M16 | **Thread settlement and snooze**                                                                          | `thread.settle` / `unsettle` / `snooze` / `unsnooze`                                                | Correctly _hidden_, not broken: `capabilities.threadSettlement` and `threadSnooze` are absent, and the client treats absent as unsupported. A real upstream feature, cleanly declined.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| M17 | **Repository identity**                                                                                   | `capabilities.repositoryIdentity: false`                                                            | Threads are not grouped across checkouts of the same repository.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| M18 | **Two subscriptions the client opens and this server does not answer**                                    | `subscribeAuthAccess`, `subscribeServerLifecycle`                                                   | Console errors on connect; no auth-state or server-lifecycle surfacing. Cheap to stub honestly.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| M19 | **Provider version advisory / self-update**                                                               | `server.updateProvider`, `server.updateServer`, `capabilities.serverSelfUpdate`                     | No "a newer `claude` is available", and no app self-update at all — `tauri.conf.json` configures no updater plugin. Upstream ships both. Full-artifact updates were accepted in the spec; none are wired.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| M20 | **Busy-terminal indicator**                                                                               | `hasRunningSubprocess`                                                                              | Always `false`, tab is always titled after the terminal rather than the program in it. Deliberate (a poll per terminal per interval for a caption) — recorded so it is not re-filed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| M21 | **`subscribeTerminalEvents`**                                                                             | —                                                                                                   | In the contract, unimplemented, and **nothing in the UI calls it.** A surface with no caller; listed only so the diff is complete.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |

---

## 3. Out of scope — will not be built in v1

These are the spec's own exclusions. Each is a decision with a reason, not a
backlog item; re-opening one is a scope change.

| Area                                                            | Why                                                                                                                                                                                                                                                    |
| --------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| **Other agents** — Codex, Cursor, Grok, OpenCode                | Separate protocols, separate adapters. Upstream has 111 files under `provider/`; laplus drives one CLI. The provider surface admits a second driver later.                                                                                             |
| **macOS and Linux**                                             | One platform removes the WebKitGTK size problem and the cross-engine QA matrix.                                                                                                                                                                        |
| **WSL**                                                         | The duplicate Linux `node_modules` tree is most of upstream's Windows-vs-macOS size gap — excluding it is a large part of why the artifact is 5 MB.                                                                                                    |
| **Accounts and auth**                                           | Permissive loopback handshake only. No login, no identity store, no multi-user, no pairing flow (`auth.bootstrapMethods` is honestly empty).                                                                                                           |
| **Cloud, relay, remote environments**                           | `cloud.*`, `relay*`, hosted pairing, Tailscale, SSH, containers. `settings.connections` has no backing.                                                                                                                                                |
| **Browser preview and preview automation**                      | The whole `preview.*` + `previewAutomation.*` block and local dev-server discovery. **The hardest one to reverse:** upstream drives an Electron `BrowserView` over CDP. WebView2 exposes no equivalent, so this is not a port — it is a new subsystem. |
| **Source-control hosting**                                      | `sourceControl.*`, `server.discoverSourceControl`, `git.runStackedAction`, `git.resolvePullRequest`, `git.preparePullRequestThread`. No GitHub/GitLab/PRs/stacked diffs/clone/publish. `settings.source-control` has no backing. Local `git` only.     |
| **Mobile and the Electron desktop app**                         | Only `apps/web` is reused.                                                                                                                                                                                                                             |
| **MCP management UI**                                           | Upstream has 13 files under `mcp/`. laplus renders MCP tool _calls_ correctly (`worklog.rs:242`) — the CLI reads its own MCP config, so MCP servers still work; there is just no UI for configuring them.                                              |
| **Auto thread titling by an LLM**                               | Upstream's `textGeneration/` (17 files). laplus titles from the first message's seed.                                                                                                                                                                  |
| **Asset URL service, external diagnostics, process management** | Named out in the spec. Note M7 and M15 overlap this — attachments need `assets.createUrl`, which is the part of it that has a real cost.                                                                                                               |
| **Delta updates**                                               | The Tauri updater has no differential download. Acceptable _because_ the artifact is small.                                                                                                                                                            |
| **Feature parity itself**                                       | Explicitly a non-goal. The 20K-LOC production-code figure is the scope-creep alarm; the server is ~34K total lines, most of it prose and tests.                                                                                                        |

---

## 4. Two things deliberately kept different

Carried from `HANDOFF-2026-07-28-using-the-app.md` so they are not re-filed as
regressions:

- **Streaming stays.** Upstream drives Claude without partial messages, so
  replies land whole. laplus passes `--include-partial-messages`. Raised twice as
  a divergence and kept twice: it is the only thing filling the first two seconds
  of a turn.
- **The composer draws no spinner.** There is a "Working for 3s" text row and no
  animated affordance. That is upstream's design, unchanged here.

---

## 5. Open tickets

| #   | Status            | What is left                                                                                                                                                                                                           |
| --- | ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 24  | `ready-for-human` | Two boxes need a machine this agent cannot provision: a clean-Windows install, and a launch where the WebView2 runtime is absent so the bootstrapper has to fetch it. The size question is answered and inside target. |
| 35  | `needs-triage`    | The draft pane's four-requests-a-second. Mechanism unchanged, character changed — the decision the ticket asks for is still unmade.                                                                                    |
| 37  | `needs-triage`    | The cost of `1053a3862`, filed against itself.                                                                                                                                                                         |
| 38  | `needs-triage`    | `catalogue`'s known gap — see M9.                                                                                                                                                                                      |

Also live and not ticketed: **the nine upstream workflows are now firing**
(ticket 36's finding). `release.yml` runs on a three-hourly schedule and
`deploy-relay.yml` on every push to `main`. Most fail for want of secrets, which
is noise — but some of it is attempts to publish. Disabling them is a decision
about this fork and has deliberately not been taken.

---

## 6. Suggested order

1. **M1** — a send that bounces is the worst bug in the list, and the fix is
   three command handlers over fields the thread row already has.
2. **M2** — plan mode currently lies to the developer. One call, following
   `ClaudeAdapter.ts:3757`.
3. **M3 + M4 + M8** — the conversation and project lifecycle: delete, archive,
   rename. Ordinary CRUD against `store.rs`, plus one snapshot method.
4. **M5** — checkpoint revert. The infrastructure exists; this is the rewind.
5. **M7** — attachments. Needs `assets.createUrl` and a store, so it is the
   first item here with real design in it.
6. **M18** — two honest stubs, to quieten the connect path.
7. Then the triage three (35, 37, 38) and M9's process-cost question.

Everything below that is section 3, and section 3 is a scope conversation rather
than a queue. Then section 7, which is a longer list than this one.

---

## 7. Read against the reference server

Sections 0–6 compare _surfaces_: which methods and commands exist on each side.
That finds a missing method; it cannot find a method that answers with less than
upstream's. This section is `apps/server/src/` read against
`server/crates/laplus-server/src/`, and everything in it is invisible to the
contract diff because in each case laplus **does** implement the method.

Two things first. The Rust tree documents its own divergences unusually well —
`provider.rs`, `terminal.rs` and `orchestration.rs` each open with a "what is
deliberately not ported" block, and several items below are found there rather
than discovered. Where that is so, it is marked **declared**. And one correction
to section 1's reading: `orchestration.replayEvents` is **not** implemented
(`rpc.rs:713` asserts it answers `ServerMethodNotImplementedError`), which makes
it the twelfth contract method the client calls and this server refuses.

### The turn: what the agent says and what the UI is told

| #   | Finding                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | Evidence                                                |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------- |
| R1  | **The context-window meter never fills — and the data is in a line laplus already parses.** `ContextWindowMeter.tsx` reads the last activity of kind `context-window.updated` (`contextWindow.ts:56`); laplus emits none. **Measured against the real CLI** (`2026-07-28-cli-stream-audit/`): every `result` line carries `modelUsage.<model>.contextWindow` (200000) plus full `usage` token counts, and every `message_delta` carries live mid-turn usage including `output_tokens_details.thinking_tokens`. `ResultEvent` declares eight fields and serde silently drops the rest. Upstream reads exactly `modelUsage` (`maxClaudeContextWindowFromModelUsage`, `ClaudeAdapter.ts:325` → `makeClaudeTokenUsageSnapshot`, `:408`). **So this is six undeclared struct fields, not a missing subsystem.** Also unread on the same line: `permission_denials`, `terminal_reason`, `api_error_status`, `ttft_ms`, `usage.iterations`. | `protocol.rs:513`, audit README                         |
| R2  | **The todo list never appears.** The plan rows come from activities of kind `turn.plan.updated` (`session-logic.ts:515`). Upstream builds them from the Todo tool (`isTodoTool`, `extractPlanStepsFromTodoInput`, `ClaudeAdapter.ts:664–694`) and merges in subagent task state (`planStepsFromClaudeTasks`). laplus emits no `turn.plan.updated`, so `TodoWrite` renders as a generic tool row and `PlanSidebar.tsx` has nothing to draw.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | `session-logic.ts:515`                                  |
| R3  | **Plan mode is absent, not merely unwired.** Beyond M2 (the mode never reaches the CLI): upstream also captures `ExitPlanMode`'s plan text (`extractExitPlanModePlan`, `exitPlanCaptureKey`, `ClaudeAdapter.ts:1092–1113`) into `thread.proposed-plan.upsert`, which fills `OrchestrationThread.proposedPlans` and lets a later turn cite `sourceProposedPlan`. laplus has no proposed-plan path at all — the contract field decodes to `[]` forever. So the whole plan → review → implement loop is missing, of which "the picker does nothing" is the smallest part.                                                                                                                                                                                                                                                                                                                                                               | `orchestration.ts:372`, `ChatView` `proposed-plan` rows |
| R4  | **Subagent progress is not tracked.** laplus reuses the `task.progress` kind for **thinking** rows (`turn.rs:33`). Upstream's `task.started`/`progress`/`completed` are the subagent lifecycle, carrying per-task token usage (`applyClaudeTaskToolResult`, `normalizeClaudeTaskProgressTokenUsage`). A `Task` call here is one collapsed tool row labelled "Subagent task" with no view inside it.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | `turn.rs:33`, `worklog.rs:275`                          |
| R5  | **Hooks fire on an ordinary turn and are invisible — and the drift counter does not catch it.** Not hypothetical: a bare "reply ok" against the real CLI emitted `SessionStart:startup` with `hook_name`, `hook_event`, `exit_code`, `outcome`, `stdout` and `stderr`. Both subtypes fall to `SystemEvent::Other` → `Folded::Nothing`, which **does not increment `unknown_events`**. `protocol.rs:33`'s own comment names them. Same shape as the bug the compact-boundary test records at `protocol.rs:1512` — "which is silence" — one subtype over. A hook that fails prints its reason into a line nothing reads.                                                                                                                                                                                                                                                                                                               | `protocol.rs:276`, `:785`                               |
| R5b | **Two more system subtypes with no arm and no second source:** `thinking_tokens` (a live thinking-token count — **25 lines across two short turns**) and `status` (`"requesting"`). Both silent, both uncounted.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | audit README                                            |
| R6  | **Thinking is most of what a turn streams, and none of it is live.** `Delta` handles `text_delta` only — **declared** at `protocol.rs:804`. Measured: **23 thinking deltas against 7 text deltas.** Streaming was kept against upstream on the explicit ground that it "is the only thing filling the first two seconds of a turn" (`HANDOFF`); on a reasoning model those seconds _are_ thinking, so the window the flag exists to fill is the window it does not fill. Nothing is lost — the block arrives whole in the buffered message — but it lands in one jump.                                                                                                                                                                                                                                                                                                                                                               | `protocol.rs:797`, audit README                         |
| R7  | **Model options are not offered.** `ProviderModel.capabilities` is hardcoded `None`, so `TraitsPicker.tsx` shows no reasoning-effort, fast-mode or thinking control. Upstream maps them onto `effort` (incl. `ultracode` → `xhigh`), `settings.fastMode` and `settings.alwaysThinkingEnabled` (`ClaudeAdapter.ts:3509–3520`). `config.rs:245` says this holds "until the ticket that _sends_ a turn can honour them" — **that ticket has shipped, so the note is stale and this is now just missing.**                                                                                                                                                                                                                                                                                                                                                                                                                               | `config.rs:245`                                         |
| R8  | **Only one control request is registered, and no MCP server is injected.** laplus passes `--permission-prompt-tool stdio` and nothing else, so `mcp_message`, `elicitation`, `request_user_dialog` and `oauth_token_refresh` go unanswered — **declared** at `protocol.rs:70`. Separately, upstream injects **its own HTTP MCP server** into every session (`mcpServers: { "t3-code": … }`, `ClaudeAdapter.ts:3552`), which is how the agent reaches t3code's own tools. laplus injects none.                                                                                                                                                                                                                                                                                                                                                                                                                                        | `protocol.rs:70`                                        |
| R9  | **Three query options are not sent:** `settingSources` (which of the CLI's setting files it reads), `additionalDirectories`, and `extraArgs` from the provider's `launchArgs` — the last **declared** in `provider.rs`. `launchArgs` is a settings field the UI can write and this server drops.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | `agent.rs:232`                                          |

### Around the turn

| #   | Finding                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Evidence                                          |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| R10 | ~~**`t3.json` is never read.**~~ **Half wrong — corrected by the surface walk (S5).** The menu really does open with `Setup Worktree`, because **the client reads `t3.json` itself** (`useT3ProjectFileScripts.ts` → `projects.readFile`, which laplus implements). What fails is _keeping_ one: the saved set is `OrchestrationProjectShell.scripts`, persisted through `project.meta.update`, which carries `scripts` and is refused (M8). So the server's hardcoded `[]` is **unpopulatable**, and importing an action is a dead end one step later than this claimed. Still true: nothing runs `runOnWorktreeCreate`, which needs worktrees (M12) anyway. | `projects.rs:56`, surface-walk §S5                |
| R11 | **Nothing ever runs `git fetch`.** `automaticGitFetchInterval` is parsed, stored and published (`settings.rs`), and no code path fetches — `git.rs` mentions fetch only in a comment about which files churn. Ahead/behind against the tracking ref therefore only moves when something outside laplus fetches. Consistent with `CONTEXT.md`'s "neither costs a network", but the setting claims otherwise.                                                                                                                                                                                                                                                   | `settings.rs`, `git.rs:425`                       |
| R12 | **The application has no runtime log.** No `tracing` or `log` dependency; 63 stderr writes. The shell captures **only** `startup.log` (`laplus-shell/src/main.rs:198`), and a released Windows build has no console, so everything written after the window opens goes nowhere. `logsDirectoryPath` is advertised to the UI and, past boot, nothing writes to it. Meanwhile `localTracingEnabled`, `otlpTracesEnabled` and `otlpMetricsEnabled` are hard `false` (`config.rs:399`) while their URL settings are read and stored. When a developer reports a bug, there is nothing to ask them for.                                                            | `config.rs:399`, `laplus-shell/src/main.rs:198`   |
| R13 | **File search is fragment matching, not fuzzy.** Upstream runs `@ff-labs/fff-node`'s FileFinder over a 25,000-entry index with a 15s scan budget and a 15min idle TTL. laplus scans and matches path fragments (`filesystem.rs:1724`) — **declared** at `:448`. Same results for a literal query; different, and worse, for the `@`-mention picker where fuzzy ranking is the point.                                                                                                                                                                                                                                                                          | `WorkspaceSearchIndex.ts:14`, `filesystem.rs:448` |
| R14 | **Terminals do not survive a restart.** Upstream retains up to 128 inactive sessions with debounced scrollback persistence (`Manager.ts:77–81`). laplus reaps every terminal when the window closes — **declared** at `terminal.rs:120`, and reasoned: restored scrollback would describe a shell already killed. Correct as far as it goes; the shells themselves are still gone, which upstream's are not.                                                                                                                                                                                                                                                  | `terminal.rs:115`                                 |
| R15 | **No command idempotency.** `commandId` is not remembered — **declared** at `orchestration.rs:52`. Upstream keeps an `OrchestrationCommandReceipts` table so a re-dispatched command answers with the sequence the first one committed at; laplus refuses it as "already exists". Safe, not idempotent, and visible to any client that retries.                                                                                                                                                                                                                                                                                                               | `orchestration.rs:52`                             |
| R16 | **`createWorkspaceRootIfMissing` is ignored.** The upstream UI sends `true` on every project add and the reference server obeys, turning a mistyped path into an empty directory. laplus refuses and names the path — **declared** at `orchestration.rs:61`, deliberate, and better. Listed so it is not mistaken for an oversight.                                                                                                                                                                                                                                                                                                                           | `orchestration.rs:61`                             |

### Measured against the real binary

Everything above this line is code read against code. These two are experiments,
with the captures kept in `2026-07-28-cli-stream-audit/`. The committed fixtures
in `server/fixtures/claude-cli/` are eighteen recordings of ticket-shaped
scenarios, and **not one of them contains a hook, a thinking delta, a
`thinking_tokens` line, a `status` line or a `usage` object** — which is why the
reducer passed its golden tests while dropping a third of the stream.

| #   | Finding                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| --- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| R18 | **Ticket 38's premise does not hold.** A turn run in a scratch directory whose only feature is `.claude/commands/zzz-probe-marker.md` produced an `init` listing **79 commands, including `zzz-probe-marker`**. The ticket declines project commands because "a probe per project would be a `claude` per project on every refresh" (`catalogue.rs:36`) — true of a probe, and they arrive free on the first real turn in that project. `InitEvent` **already declares `slash_commands`** (`protocol.rs:364`) and nothing reads it. The catalogue's other objection (`:112`, init is not written until the CLI is prompted) rules init out for the _handshake_ only. The genuine trade is `:120`: the handshake returns descriptions and argument hints, `init` returns bare names — so the answer is the union, not a choice. |
| R19 | **65% of the CLI's output produces nothing** (66 of 102 lines over two short turns). About half of that is redundant rather than lost — thinking and tool-input deltas re-arrive whole in the buffered message. The other half has no second source at all: 25 `thinking_tokens`, 4 hook lines, 3 `status`, 3 `message_delta` usage payloads. **A third of the stream reaches the developer nowhere, and none of it increments a drift counter**, because `SystemEvent::Other` and `Delta::Unknown` are silence by construction. The drift counter's job is to notice when the CLI moves; on the evidence it cannot notice what the CLI already does.                                                                                                                                                                          |

### The structural one

| #   | Finding                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| --- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| R17 | **Upstream is event-sourced; laplus keeps current state only.** `OrchestrationEventStore`, `OrchestrationCommandReceipts`, `decider.ts`, `projector.ts`, `ProjectionPipeline` and 34 projection migrations, against `store.rs`'s tables of the world as it now is. ADR-0016 takes this deliberately. It is the _reason_ for three separate gaps rather than a gap itself: `replayEvents` cannot be answered, `commandId` cannot be honoured (R15), and `getArchivedShellSnapshot` has no history to query (M4). Anything wanting "what did this thread look like at turn 7" is structural work, not a handler. |

### What this section did _not_ find

Worth recording, because a comparison that only finds gaps has not been done
honestly. The turn translation table (`turn.rs:26`), the tool classification and
titles (`worklog.rs`, ported from `classifyToolItemType` / `titleForTool` /
`summarizeToolRequest`), the permission decision mapping, the terminal shell
resolution (which **extends** upstream's `ComSpec`/`SHELL` handling), the
keybinding merge-by-command semantics, the settings patch semantics and the
checkpoint diff range arithmetic — including `ignoreWhitespace`, which is
honoured — all match, and in three places laplus is deliberately better and says
why: `tool.updated` instead of a dropped `tool.started` (`worklog.rs:24`), the
binary-resolution diagnostic that names what was looked for and where
(`provider.rs:17`), and R16.

### Order, revised

The measurement changes the order. **R1 is now the cheapest item on this
page** — six fields on `ResultEvent`, the same shape off `message_delta`, and one
activity kind — and it lights up a meter the developer looks at constantly. Do it
first, before M1.

Then R5/R5b/R19 together: four arms on `SystemEvent`, which turns hooks,
thinking-token counts and live status from silence into rows, and — more
importantly — settles whether unrecognised subtypes should keep folding to
silence. On the evidence they should not; a `system` subtype this build does not
know is exactly what the drift counter was built to report, and it is the one
thing it cannot see.

R18 next, because it is now a small change to a ticket that was parked as a
design question.

Then R2 and R3 (plan and todo state), which are real work rather than field
declarations. Then R12, on the grounds that everything else gets harder to
diagnose without a log. M1 sits between R1 and R5 on severity — it is the only
item on either list that stops a message from being sent.
