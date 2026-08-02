# Ticket 13: OpenCode tool and permission wire

Research date: 2026-08-02. The implementation target is OpenCode `v1.18.10`.
The first-party comparison is T3 Code commit `0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62`, the commit already selected by the feature research. All protocol claims below come from generated OpenCode SDK/OpenAPI output or first-party source.

## Implementation conclusions

- Use the v2/current permission vocabulary: `permission.asked` and
  `permission.replied`. Do not treat the deprecated v1
  `permission.updated` shape as interchangeable with `permission.asked`.
- Apply a runtime ruleset when creating a session and with `PATCH` whenever an
  existing or forked session is adopted. Both operations accept the same
  `PermissionRuleset` value.
- Reply through `POST /permission/{requestID}/reply` with a JSON `reply`, not
  through the legacy session-scoped permissions route.
- Preserve every tool's full `state` and use a generic dynamic-tool rendering
  for names that do not match a known family.

## Session permission rules

`PermissionRuleset` is an array of `{ permission: string, pattern: string,
action: "allow" | "deny" | "ask" }` rules. [OpenCode 1.18.10 generated v2
types](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L158-L168)

Create is `POST /session`. Its JSON body may include `permission:
PermissionRuleset` alongside `parentID`, `title`, `agent`, `model`, `metadata`,
and `workspaceID`; `directory` and `workspace` are query parameters, not body
members. [Generated create type](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L9469-L9505) and [generated client method](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/sdk.gen.ts#L3410-L3458)

Updating permissions is `PATCH /session/{sessionID}` with JSON body
`{ permission: PermissionRuleset }` (the body may also carry `title`,
`metadata`, and `time.archived`). `directory` and `workspace` again belong in
the query. [Generated update type](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L9609-L9642) and [generated client method](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/sdk.gen.ts#L3558-L3602)

T3's exact runtime-mode mapping is:

```ts
full-access => [{ permission: "*", pattern: "*", action: "allow" }]

all other runtime modes => [
  { permission: "*",                  pattern: "*", action: "ask" },
  { permission: "bash",               pattern: "*", action: "ask" },
  { permission: "edit",               pattern: "*", action: "ask" },
  { permission: "webfetch",           pattern: "*", action: "ask" },
  { permission: "websearch",          pattern: "*", action: "ask" },
  { permission: "codesearch",         pattern: "*", action: "ask" },
  { permission: "external_directory", pattern: "*", action: "ask" },
  { permission: "doom_loop",          pattern: "*", action: "ask" },
  { permission: "question",           pattern: "*", action: "allow" },
]
```

Thus `approval-required`, `auto-accept-edits`, and `auto` intentionally collapse
to the same OpenCode rules; only `full-access` differs. [T3 runtime mapping](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts#L328-L343)
T3 passes the mapping to `session.create`, and reapplies it with
`session.update` for both resumed sessions and sessions forked because the cwd
changed. [T3 session setup](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1247-L1311)

## Permission events: current v2 versus legacy v1

The pinned 1.18.10 package exposes both APIs, which explains the apparent name
conflict.

The v2/current request event is:

```ts
{
  id: string,                         // outer event id
  type: "permission.asked",
  properties: {
    id: string,                       // request id
    sessionID: string,
    permission: string,
    patterns: string[],
    metadata: Record<string, unknown>,
    always: string[],
    tool?: { messageID: string, callID: string }
  }
}
```

Its resolution is `{ id, type: "permission.replied", properties: {
sessionID, requestID, reply: "once" | "always" | "reject" } }`.
[OpenCode 1.18.10 v2 event union](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L6855-L6886)

The deprecated v1 event is instead `permission.updated`, whose `properties`
is a `Permission` object: `{ id, type, pattern?, sessionID, messageID, callID?,
title, metadata, time:{created} }`. Its legacy `permission.replied` is
`{sessionID, permissionID, response:string}`. [OpenCode 1.18.10 legacy generated
types](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/gen/types.gen.ts#L423-L452)
The present upstream tree still generates these legacy types while its v2
types retain `permission.asked`/`permission.replied`, so this is an API-version
difference, not evidence that v2 renamed `asked` to `updated`.
[Current legacy source](https://github.com/anomalyco/opencode/blob/01624c8c87457261277440291b455dc16d6cfa3c/packages/sdk/js/src/gen/types.gen.ts#L423-L452) and [current v2 source](https://github.com/anomalyco/opencode/blob/01624c8c87457261277440291b455dc16d6cfa3c/packages/sdk/js/src/v2/gen/types.gen.ts#L6861-L6892)

Implementation consequence: Laplus's current `KNOWN_EVENTS` entry for
`permission.updated` is legacy compatibility only. Ticket 13 needs to recognize
`permission.asked`; it may retain `permission.updated`, but must decode it with
its distinct legacy payload.

## Reply wire and decision mapping

The v2 operation is `POST /permission/{requestID}/reply`, JSON body
`{ reply: "once" | "always" | "reject", message?: string }`, optional
`directory`/`workspace` query, returning boolean on success. [Generated reply
type](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L9260-L9295) and [generated client](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/sdk.gen.ts#L3125-L3154)

The older v1 route is `POST /session/{id}/permissions/{permissionID}` with
`{response:"once"|"always"|"reject"}`. It belongs only with the legacy event
shape. [Legacy generated route](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/gen/types.gen.ts#L2889-L2925)

T3 maps UI decisions exactly as follows: `accept -> once`,
`acceptForSession -> always`, and both `decline` and `cancel -> reject`.
[T3 decision mapping](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts#L345-L359)
It refuses an ID absent from its pending-permission map, then calls
`client.permission.reply({requestID, reply})`; the request remains pending
until the upstream replied event removes it. [T3 request response](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1558-L1579)

T3 maps inbound permission names `bash -> command_execution_approval`, `read ->
file_read_approval`, `edit -> file_change_approval`, else `unknown`. On
`permission.asked`, it uses `properties.id` as the pending/request ID, joins
nonempty `patterns` with newlines for the detail (falling back to the permission
name), and exposes `metadata` as args. On `permission.replied`, it removes
`properties.requestID` and maps `once -> accept`, `always -> acceptForSession`,
`reject -> decline`. [T3 permission event mapping](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L318-L341) [T3 event handling](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L941-L975)

## Tool-part state wire and T3 rendering

A tool part is `{id, sessionID, messageID, type:"tool", callID, tool, state,
metadata?}`. Its state union is:

- pending: `{status:"pending", input:Record<string,unknown>, raw:string}`
- running: `{status:"running", input, title?, metadata?, time:{start}}`
- completed: `{status:"completed", input, output:string, title:string,
metadata, time:{start,end,compacted?}, attachments?:FilePart[]}`
- error: `{status:"error", input, error:string, metadata?, time:{start,end}}`

[OpenCode 1.18.10 tool schemas](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L477-L547)
Tools arrive inside `message.part.updated`, with the complete `Part` under
`properties.part`; the v2 envelope also contains an outer event `id` and
`properties.sessionID` and `properties.time`. [OpenCode part-update event](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L6227-L6235)

T3 classifies tool names case-insensitively by substring:

| Name contains                            | Shared item type         |
| ---------------------------------------- | ------------------------ |
| `bash` or `command`                      | `command_execution`      |
| `edit`, `write`, `patch`, or `multiedit` | `file_change`            |
| `web`                                    | `web_search`             |
| `mcp`                                    | `mcp_tool_call`          |
| `image`                                  | `image_view`             |
| `task`, `agent`, or `subtask`            | `collab_agent_tool_call` |
| none of the above                        | `dynamic_tool_call`      |

[T3 tool classification](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L286-L316)

It uses `callID` as the shared item ID. Pending emits `item.started`, running
emits `item.updated`, and completed/error emit `item.completed`; shared status
is respectively `inProgress`, `inProgress`, `completed`, or `failed`. It retains
the full upstream event as `raw` and the exact `{tool,state}` under payload
data, which is the required diagnostic fallback for unknown tool names.
[T3 tool event handling](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L891-L937)
