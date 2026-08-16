# OpenCode integration audit: T3 Code upstream vs Laplus

Date: 2026-08-13
Upstream audited: `pingdotgg/t3code` at [`5015d7cf9f98fe551115b625031f01e3f022cd2d`](https://github.com/pingdotgg/t3code/commit/5015d7cf9f98fe551115b625031f01e3f022cd2d)
Laplus audited: current working tree (implementation was not changed by this audit)

## Executive summary

Laplus has already ported most of the important OpenCode work that landed in T3 Code during July and August 2026: a 30-second owned-server startup bound, native provider titles, durable session IDs, fail-closed resume, cwd-aware forking, connected-provider model filtering, nested-slash model parsing, permissions/questions, agent and variant choices, attachments, MCP registration, structured errors, retry-status display, rollback, and owned/external lifecycle separation.

Three gaps still stand out:

1. **Critical — turn settlement has no loss recovery.** Both current T3 and Laplus treat the OpenCode SSE stream plus a `session.status: idle`/legacy `session.idle` event as the authoritative completion signal. If the subscription ends or drops an idle event, there is no reconnect, REST status reconciliation, or watchdog. Upstream issue [#2644](https://github.com/pingdotgg/t3code/issues/2644) remains open and includes a trace in which `/event` unsubscribes milliseconds after connecting; a report from OpenCode 1.18.9 and T3 0.30.0 was added on 2026-08-01. [#2886](https://github.com/pingdotgg/t3code/issues/2886) was closed only as a duplicate of still-open [#2173](https://github.com/pingdotgg/t3code/issues/2173), not as fixed. Current upstream code still has a one-shot `event.subscribe` pump and only emits completion on `session.status.type === "idle"`; current Laplus similarly turns stream closure into an error and settles only on idle/error. The correct improvement is a bounded reconnect/reconciliation state machine, not another event-shape patch.
2. **High — first-open models lack last-known-good hydration in Laplus.** T3 persists per-instance provider snapshots, hydrates them at boot, then replaces stale OpenCode models after an authoritative successful refresh. Laplus has no equivalent disk-backed provider snapshot cache; it synchronously waits for discovery and therefore shows only configured fallback custom models—or an error/empty discovered inventory—until a potentially slow cold `opencode models --verbose` succeeds. The user’s “default models only on first open” symptom is consistent with catalogue cold-start/transient failure. Laplus’s new 30-second timeout helps but does not provide immediate last-known-good models or T3’s one-time retry after SQLite/CLI contention.
3. **Medium — local catalogue discovery is sequential and has no retry.** Current T3 runs `models --verbose` and `agent list` concurrently and retries failed commands once after one second; model failure is authoritative error while agent failure degrades only option metadata. Laplus gives each command 30 seconds but runs models, agents, and skills sequentially and does not retry transient failures. This can make first open unnecessarily slow and brittle.

T3 upstream is not, by itself, evidence that OpenCode is now fully stable. Its current issue tracker and current adapter show the stuck-working class remains unresolved.

## Architecture and lifecycle

### T3 upstream

`OpenCodeDriver` constructs three independently scoped facilities per configured instance: provider snapshot discovery, conversation adapter, and text generation. An external `serverUrl` is borrowed and never terminated. Without one, each active conversation owns an `opencode serve` child scoped to that session. Text-generation calls share a separate lazily created owned server with idle shutdown.

Owned servers:

- run `opencode serve --hostname=<loopback> --port=<reserved>`;
- receive `OPENCODE_CONFIG_CONTENT={}` so T3’s wrapper does not inject its own config while OpenCode still loads the user/project configuration according to OpenCode behavior;
- are placed in a process group;
- bind child, stdout/stderr readers, exit watcher, SSE pump, and abort controller to a single scope;
- terminate the whole group with TERM then KILL on scope close;
- allow up to 30 seconds for readiness after [`398140a9bde5596cd6b45dc546150c6f5e3b23b7`](https://github.com/pingdotgg/t3code/commit/398140a9bde5596cd6b45dc546150c6f5e3b23b7).

T3 originally used a scoped serve process to probe inventory, then stopped doing so because refresh could leak serve descendants. [`35822884d19ba64c6529d7736fb0182b361f3e4c`](https://github.com/pingdotgg/t3code/commit/35822884d19ba64c6529d7736fb0182b361f3e4c) hardened process-group cleanup; [`0ca3240691bf1773802b3ed70330515d68b0a6b8`](https://github.com/pingdotgg/t3code/commit/0ca3240691bf1773802b3ed70330515d68b0a6b8) then changed local provider health/inventory checks to CLI calls rather than starting a server at all.

### Laplus comparison

Laplus follows the same ownership boundary. `OwnedServer` reserves loopback, launches `serve`, polls `/global/health`, uses a 30-second timeout, puts Unix children in their own process group, and terminates the tree gracefully then forcibly. External servers are borrowed and may use Basic auth; the password is redacted from errors. Owned servers are reaped after idle/session stop, project closure, server shutdown, and startup failure. This is at least parity with the lifecycle lessons in the upstream commits above.

One implementation difference is readiness: current T3 parses the URL announced on stdout; Laplus polls the health endpoint on its reserved port. Health polling is more semantically meaningful, while stdout capture gives T3 better startup diagnostics. Laplus currently discards owned-server stdout/stderr, so **a medium-quality improvement** is to retain bounded startup stderr/stdout and include redacted tails when the child exits or times out.

## Provider and model discovery

### T3 current behavior

For a local CLI T3 first runs `opencode --version`, enforces OpenCode `>=1.14.19`, then calls:

- `opencode models --verbose` for the authoritative provider/model inventory;
- `opencode agent list` for model option enrichment.

Those calls run concurrently. If either fails or exits non-zero, only failed calls are retried once after one second (the source calls out transient SQLite `database is locked`). A persistent models failure fails inventory; a persistent agents failure yields models without agent options. For external servers it uses SDK v2 `provider.list` and `app.agents` concurrently.

Only providers in OpenCode’s `connected` list are exposed. Each model is keyed as `providerID/modelID`. Model JSON supplies display name and variant keys; visible primary/all agents become an `agent` select option. Provider-specific defaults are inferred for common variant names and `build` is preferred as the default agent.

The verbose CLI parser is deliberately brace-depth based. [`2ea51bd31f494ec3214a0c9f9adc3262c71e6530`](https://github.com/pingdotgg/t3code/commit/2ea51bd31f494ec3214a0c9f9adc3262c71e6530) fixed a parser that mistook a nested slash-looking value in JSON for the next model header. Laplus’s current parser already uses header plus balanced JSON blocks and accepts slashes after the provider separator, so this upstream fix is already represented.

### Cache semantics and first-open behavior

T3 writes each provider instance’s last snapshot to `<cacheDir>/<instanceId>.json`. At boot it creates a settings-derived fallback, validates cached `instanceId` and driver, preserves current enabled/configured custom-model settings, and hydrates installed/version/status/auth/models/commands/skills from disk while the live probe runs.

The subtle part is live merge semantics. Before [`4e09cddb40eb1bb1e111a0374b46e73b38ffbb29`](https://github.com/pingdotgg/t3code/commit/4e09cddb40eb1bb1e111a0374b46e73b38ffbb29), previous OpenCode models could survive an authoritative successful refresh, leaving models from disconnected plugins/accounts. Now T3 retains old OpenCode models only while the initial probe is pending or an installed provider probe failed. A ready/warning successful inventory, disabled provider, or missing CLI authoritatively removes absent models. The same commit made persistent model CLI failure an error rather than an empty “success.”

Laplus’s `ConfigStore` stores provider configuration, including authored custom model slugs, but not provider discovery snapshots. Its snapshot on startup is created directly by synchronous discovery. Consequences:

- no immediate last-known-good connected model list during a cold probe;
- no resilience to a one-off catalogue failure except custom models configured explicitly in Laplus;
- no cache staleness problem, but also none of T3’s responsive boot behavior;
- local model, agent, and skill calls are sequential and individually bounded at 30 seconds.

**Recommended design (high):** add a per-instance, schema-versioned last-known-good provider inventory cache. Hydrate it immediately but label it with its prior `checkedAt`; merge current configured custom models from settings; launch refresh; retain cached discovered models only for pending/transient installed-provider failure; replace them exactly on any authoritative successful inventory, including empty connected providers. Correlate by provider instance ID, driver, binary/server identity fingerprint, and preferably normalized cwd/config context so one OpenCode installation cannot contaminate another. Do not persist passwords or update state.

**Recommended near-term fix (medium/high):** parallelize `models --verbose` and `agent list`, retry only failed commands once after about one second, and make skills best-effort. Preserve the current useful distinction that models are authoritative while agents/skills are enrichment.

## Sessions, names, durable continuation, and cwd moves

T3 stores a versioned cursor containing the upstream `ses_…` ID. On start:

1. `session.get` verifies a stored ID.
2. Only structured 404/`NotFoundError` permits a fresh session. Transport, auth, and other server failures propagate, preventing silent context loss. This fail-closed path was introduced around [`d0b9f8d40d7f269a4c77b7c0302b5889a31ba66a`](https://github.com/pingdotgg/t3code/commit/d0b9f8d40d7f269a4c77b7c0302b5889a31ba66a) and corrected for follow-ups by [`f4da4f3b4037260bbb0d8914acbebafd2206607a`](https://github.com/pingdotgg/t3code/commit/f4da4f3b4037260bbb0d8914acbebafd2206607a).
3. Same-directory sessions are reused and permissions are re-applied.
4. A session created under another directory is forked into the requested directory so history survives a worktree/cwd move.
5. The cursor is re-emitted on every turn so persistence is refreshed.

T3 stopped passing a fixed title to ordinary session creation in [`3235658c080bc12fcd1ffaa275aced98d225f2f6`](https://github.com/pingdotgg/t3code/commit/3235658c080bc12fcd1ffaa275aced98d225f2f6). It listens for `session.updated`, extracts OpenCode’s title, and publishes a thread-name update. It still supplies explicit internal titles for throwaway text-generation sessions, which is appropriate.

Laplus now matches these behaviors: versioned durable cursor, exact adoption, fail-closed non-404 errors, verified cwd and fork/move migration, permissions re-application, fresh replacement only on structured missing-session, and provider title propagation. The earlier forced `Laplus conversation` title removal is therefore the correct upstream-aligned fix.

## Event stream, normalization, and turn settlement

### Current upstream pipeline

T3 starts one SDK v2 `event.subscribe` per session with an abort controller. It filters events to the OpenCode session ID, while tracking child sessions for relevant subagent data. It normalizes:

- `session.updated` to thread title;
- message metadata and parts to assistant text/reasoning;
- tool states to lifecycle items;
- permission and question asked/replied/rejected events;
- `session.status.retry` to a warning;
- `session.status.idle` to `turn.completed`;
- `session.error` to failed assistant item, runtime error, and aborted turn.

The raw delta bug fixed by [`271d65e047c557b1bf74c66dc18c8f09d42893a0`](https://github.com/pingdotgg/t3code/commit/271d65e047c557b1bf74c66dc18c8f09d42893a0) is handled by cumulative/delta bookkeeping. A prompt sent while busy is treated as steering and reuses the active turn ID. Prompt dispatch failure resets a fresh turn to ready and emits `turn.aborted`; a failed steer leaves the original active turn running.

### The unresolved stuck-working failure

The code has no automatic recovery after an unexpected event-stream end. It emits an unexpected-exit/session error, but it does not:

- reconnect with a backoff;
- query current session status;
- fetch messages and reconcile missing final text;
- synthesize settlement if REST says idle;
- bound how long a UI can remain “working” with no provider activity.

That matters because upstream’s reports are not historical-only:

- [#2644](https://github.com/pingdotgg/t3code/issues/2644) is open. A detailed report shows OpenCode subscribing then unsubscribing about 3 ms later, with completed messages available through REST but no renderer events. Reports span macOS, Windows/WSL, Linux, OpenCode 1.14.48 through 1.18.9, and T3 through 0.30.0.
- [#2886](https://github.com/pingdotgg/t3code/issues/2886) describes repeated mid-run steering followed by a persisted working state. It was closed as a duplicate of [#2173](https://github.com/pingdotgg/t3code/issues/2173), which remains open and also has reports involving non-OpenCode providers. Closure therefore is not evidence of a fix.

Laplus avoids the suspected SDK/HTTP SSE one-shot defect by implementing SSE over `reqwest::bytes_stream`, and its fixture suite exercises fragmented SSE and duplicate idle. That is a meaningful advantage. But its logical recovery boundary is the same: `EventStream::next()` returns `StreamClosed`; the driver has no resubscription or REST reconciliation, and normal completion still depends on `session.idle` or `session.status.idle`. A network flap, reverse proxy idle timeout on an external server, malformed event, or missed idle can still wedge/fail a turn.

**Recommended reliability work (critical):** introduce an explicit event-stream supervisor:

1. Keep a monotonic last-event/activity timestamp and current session/turn generation.
2. On EOF or retryable transport failure, query session status and messages before deciding failure.
3. If status is idle, reconcile assistant parts from `session.messages`, emit any missing suffixes, and settle once.
4. If busy/retry, resubscribe with capped exponential backoff and jitter. Preserve pending permission/question maps and normalization state.
5. If session is structured-not-found, fail the active turn without silently creating a new conversation.
6. If auth/protocol error, fail immediately with the structured provider error.
7. Add a conservative no-event watchdog that performs reconciliation (not blind abort) while a turn is running.
8. Make settlement idempotent across duplicate idle, reconnect replay, abort, error, and server-exit races.

Tests should simulate EOF before idle with completed REST history, EOF while still busy followed by successful reconnect, duplicate replay, external proxy disconnect, malformed SSE, permission pending across reconnect, abort racing reconciliation, and server death.

## Permissions and questions

T3 builds OpenCode permission rules from the shared runtime mode. Full access allows all. The interactive mode defaults to ask, explicitly asks for shell/edit/web/external-directory/doom-loop classes, and allows the OpenCode `question` tool so it can flow through the dedicated question API. Permission decisions map to OpenCode `once`, `always`, or `reject`. Questions preserve order, support multi-select, reply with an ordered array of answer arrays, and have a distinct reject route/event.

Laplus implements both current v2 and legacy permission routes, maps all shared approval decisions, retains requests until reply events resolve them, preserves question IDs/order, and distinguishes reject. This is broader compatibility than current upstream. No major upstream-derived gap was found here.

One hardening opportunity is reconnect persistence: pending permissions/questions currently live in memory in both implementations and rely on an uninterrupted event stream. The supervisor proposed above should query/replay pending provider requests if OpenCode exposes that state, or at minimum preserve maps and deduplicate replayed asked events.

## Model, agent, variant, and interaction options

T3 validates model slugs as `provider/model`, sends `{ providerID, modelID }`, passes selected `agent` and `variant`, and maps plan interaction mode to agent `plan` when no explicit agent is selected. It prefers the `build` primary agent and infers common variant defaults. It advertises in-session model switching.

Laplus exposes agent and variant descriptors and sends them with prompts, including plan-mode agent selection. It does not appear to mark the inferred default option as richly as current T3 (T3 title-cases labels and selects common defaults). This is **low severity UX parity**, not a stability defect: port default-agent/default-variant inference if the shared contracts/UI consume `isDefault` consistently.

## Attachments and MCP

T3 resolves stored attachments to local paths, converts them to `file://` parts with MIME type and filename, and permits attachment-only prompts. Missing attachment references are omitted. For owned servers it registers a per-thread remote MCP endpoint called `t3-code` with bearer authorization and OAuth disabled; external servers are not mutated with the local MCP endpoint. Text-generation requests also accept relevant attachments.

Laplus matches this boundary: attachment resolution/file parts, attachment-only prompts, and authenticated per-thread MCP registration only on owned servers. Laplus additionally validates the returned MCP connection status and fails startup if the injected MCP could not connect. No material gap was found.

## Errors, retry, abort, stop, and rollback

T3’s SDK wrapper uses `throwOnError: true` and normalizes response/body/cause shapes. Text generation classifies session-create failures, transport prompt failures, provider-reported prompt errors, and empty structured output; [`8c1605b38327176bae94c33bbc6de86e8b09fe02`](https://github.com/pingdotgg/t3code/commit/8c1605b38327176bae94c33bbc6de86e8b09fe02) is the relevant structured-error change. Conversation abort calls `session.abort`, emits `turn.aborted`, closes the event subscription, and tears down owned process scope. Rollback loads assistant messages and calls `session.revert` at the retained message boundary.

Laplus has equivalent structured HTTP/SSE errors with password redaction, displays `session.status.retry`, aborts remotely, keeps partial output, settles duplicate idle idempotently, and implements the same assistant-boundary rollback. It also explicitly tears down process groups. No major gap was found beyond stream loss recovery and startup diagnostics.

## Text generation and titles

T3 shares a lazily owned OpenCode server for short structured jobs (commit message, PR content, branch name, thread title), schedules idle close, creates a deny-all session, sends a synchronous prompt with model/agent/variant and files, validates provider errors and structured JSON, and sanitizes output. Ordinary conversations delegate naming to OpenCode; T3’s own text-generation title is a separate optional UI feature.

Laplus has an OpenCode text-generation path and the first-turn title system. The user-visible naming defect came from supplying a fixed title to the ordinary provider session, not from the text-generation feature; removing that fixed title is correct.

## Maintenance and updates

T3 recognizes native OpenCode installs, npm, and Homebrew, offering `opencode upgrade`, `npm install -g opencode-ai@latest`, or `brew upgrade anomalyco/tap/opencode`, and supports disabling update checks. Laplus’s maintenance resolver additionally recognizes pnpm and uses comparable native/npm/Homebrew actions. Both enforce the same minimum OpenCode version. Laplus is not behind upstream here.

## Prioritized action list

| Priority | Work                                                                                                                              | Why                                                                                                                        |
| -------- | --------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| P0       | Add SSE disconnect reconciliation and bounded resubscription                                                                      | Only durable fix for the still-open stuck-working class; protects external servers and network faults too.                 |
| P1       | Persist and hydrate per-instance last-known-good OpenCode inventories with authoritative replacement rules                        | Directly improves first-open models and avoids blank/default-only menus during cold discovery.                             |
| P1       | Parallelize local models/agents discovery and retry failed calls once after 1 s                                                   | Matches current upstream handling of transient OpenCode/SQLite contention and lowers startup latency.                      |
| P2       | Capture bounded/redacted owned-server startup stdout/stderr                                                                       | Makes “bugs without reason” diagnosable when the process exits before readiness.                                           |
| P2       | Add provider-native diagnostics around stream connect/disconnect, last event, reconcile attempt, session ID, and owned child exit | Enables the next instability report to identify lifecycle vs event vs provider failure without exposing prompts/passwords. |
| P3       | Port T3’s agent/variant label and default inference                                                                               | UX polish; not required for stability.                                                                                     |

## Evidence notes

Primary sources used were current upstream source and history, upstream’s first-party GitHub issues, and Laplus source/tests. Stable commit links are used above instead of branch links. Issue reports are evidence of observed behavior, not proof of root cause; the recommended SSE supervisor follows from combining those reports with the absence of a recovery path in both current adapters.
