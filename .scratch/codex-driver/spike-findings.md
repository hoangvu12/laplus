# What driving `codex app-server` by hand established

Recorded 2026-07-31 against **codex-cli 0.146.0**, by hand, before any laplus
code exists. `captures/` holds the recordings; this file is what they mean.

`upstream-research.md` was written without running anything and says so. Four of
its claims are corrected here, and each correction is marked **⚠**. Read this
file after that one.

## The handshake decides what the server sends

**⚠ `capabilities: {"experimentalApi": true}` suppresses `turn/completed`.** A/B
with the same prompt and nothing else changed:

| `capabilities`              | how the turn ends                                             |
| --------------------------- | ------------------------------------------------------------- |
| `{"experimentalApi": true}` | no `turn/completed`; `thread/status/changed: {"type":"idle"}` |
| `{}`                        | `turn/completed`, then idle                                   |

Upstream sends the flag from `buildCodexInitializeParams()`, and that one
function feeds both their probe and their session runtime — so their turns end on
a status change. They carry `turn/completed` handling anyway
(`CodexAdapter.ts:770`), which under their own handshake does not fire.

laplus sends `capabilities: {}` and settles on `turn/completed`, because it
carries `turn.error` and `thread/status/changed` carries nothing about how the
turn went — and `Ending` is exactly that distinction. `idle` is handled as a
fallback so the flag can be flipped later without breaking the settle.

Two more shapes from the same handshake:

- **Responses carry no `"jsonrpc"` member.** `{"id":1,"result":{…}}` is the whole
  envelope. A decoder that requires the member fails on every message.
- The version is in `initialize.userAgent`, which begins with **our own** client
  name: `laplus/0.146.0 (Ubuntu 20.4.0; aarch64) unknown (laplus; 0.0.1)`.
  Upstream's "text after the first `/`" parse works only because a client name
  contains no slash.

## An interrupt is answered by a response and by nothing else

`04-interrupt.jsonl`, and it is the capture that matters most.

- **106 deltas arrived after `turn/interrupt` was sent.** In-flight output
  flushes first; the acknowledgement is last.
- **No `item/completed` for the streaming message.** The partial text never gets
  an authoritative version.
- **No `turn/completed`. No `idle`.** `{"id":4,"result":{}}` is the only terminal
  signal.

**⚠ Reconciliation does not hold on an interrupted Codex turn.** `CONTEXT.md`
defines it as the buffered message replacing the accumulation, and
`fixtures/claude-cli/11-interrupted-turn.ndjson` records `claude` handing the
partial text over whole. Codex hands over nothing, so the accumulation _is_ the
text, and the driver settles from a response rather than from a notification.

## Approvals are not the free 1:1 mapping they looked like

**⚠** The contract's four literals — `accept`, `acceptForSession`, `decline`,
`cancel` — are the words Codex uses, but **which of them apply is per request**.
The sandbox-escaping write in `03-write-approval.jsonl` offered:

```json
"availableDecisions": ["accept",
                       {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": [...]}},
                       "cancel"]
```

No `decline`. No `acceptForSession`. So the panel's buttons are gated by the
request rather than by the contract, and a driver that always offers four will
offer decisions the server did not.

**⚠ The server's own request ids start at 0 and are a separate id space from
ours.** One map keyed by id across both directions collides.

Also from that capture: `item/started` for the `commandExecution` arrives
_before_ the approval request, so the tool call is on screen when permission is
asked — which is the order the work log wants. And under
`approvalPolicy: untrusted` with a read-only sandbox, `ls` ran with **no**
approval at all: what triggers a request is the sandbox escape, not the policy
name.

## Continuity costs one string

`05-resume.jsonl`: a **new process** resumed `01`'s thread from its id alone and
the agent quoted the earlier prompt back. Nothing else is stored; the rollout
under `CODEX_HOME` is the continuity.

`06-resume-missing.jsonl`, resuming an id that has none:

```json
{ "error": { "code": -32600, "message": "no rollout found for thread id 019f0000-…" } }
```

**Upstream's recovery would not fire on it.** `isRecoverableThreadResumeError`
matches `"not found"`, `"missing thread"`, `"no such thread"`, `"unknown
thread"`, `"does not exist"` — and `"no rollout found"` is none of those. laplus
treats **any** `thread/resume` error as recoverable instead, which is both
simpler and not a list that goes stale: the fallback publishes an activity
saying the agent's memory is gone, so nothing is hidden by being generous.

## What a turn is made of

`01`, `02` and `03`, in order of arrival:

```
thread/started → thread/status/changed(active) → turn/started
  → item/started+completed (userMessage)
  → item/started+completed (reasoning)
  → item/started (agentMessage, phase "commentary") → item/agentMessage/delta ×N → item/completed
  → item/started (commandExecution) → [item/commandExecution/requestApproval] → item/completed
  → item/started (agentMessage, phase "final_answer") → deltas → item/completed
  → thread/tokenUsage/updated → account/rateLimits/updated
  → thread/status/changed(idle) → turn/completed
```

- `agentMessage` carries a **`phase`**: `commentary` before tool use,
  `final_answer` after. Whether commentary is a message or an activity is a
  decision the driver owes the work log.
- `commandExecution` carries `command` (`/bin/bash -lc "…"`), `cwd`,
  `processId`, `status`, `exitCode`.
- `thread/tokenUsage/updated` and `account/rateLimits/updated` arrive **per
  turn**, which is where **Standing** and the context-window reading come from.

## Noise that arrives whether or not anything was asked

Day-one drift, all of it seen before the first prompt:

- `configWarning` — on this machine, missing `bubblewrap`; codex falls back to a
  bundled copy.
- `remoteControl/status/changed`.
- `mcpServer/startupStatus/updated` ×N — **codex starts its own MCP server
  (`codex_apps`) per thread**, so a per-conversation app-server carries an MCP
  child with it. That is a cost of the one-process-per-conversation shape, and it
  is worth measuring before the shape is called settled.
- stderr carries `ERROR`-level lines that are benign — the bubblewrap warning is
  one. A driver that treats stderr `ERROR` as fatal calls a working Codex broken.

`ServerNotification.json` declares **70** notification methods. A v1 handles
around a dozen and counts the rest as drift.

## The provider probe, confirmed

- Not logged in: `account/read` → `{"account":null,"requiresOpenaiAuth":true}`,
  and `model/list` answers **anyway**.
- Logged in: `{type, email, planType}` — here `planType: "prolite"`.
- `model/list` returned **7** models on one page (`nextCursor: null`), each with
  its own `supportedReasoningEfforts` — 6 for `gpt-5.6-sol` and `-terra`, 5 for
  `-luna`, 4 for the rest. So `optionDescriptors` is per model, not per provider.
- The contract's defaults all exist (`gpt-5.6-sol`, `gpt-5.6-terra`,
  `gpt-5.6-luna`). Its alias table points `5.3` at **`gpt-5.3-codex`**, which
  this account cannot use — only `gpt-5.3-codex-spark` exists. A compiled model
  table would already be wrong; the live probe is not.
