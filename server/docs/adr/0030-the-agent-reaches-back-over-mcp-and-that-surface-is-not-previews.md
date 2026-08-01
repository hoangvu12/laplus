# ADR-0030 — The agent reaches back over MCP, and that surface is not preview's

Date: 2026-07-30
Status: Accepted

## Context

The contract-parity ledger counts twenty-seven methods this server refuses. Four
of them are the preview automation surface —
`previewAutomation.{connect,respond,focusHost}` and `subscribePreviewEvents` — and
they are unlike the other twenty-three in a way the count cannot show.

**Their host half already ships.**
`apps/web/src/components/preview/previewAutomationRequestConsumer.ts` consumes
automation requests and answers them, with its own tests, and
`packages/client-runtime/src/state/preview.ts` calls all three methods. A laplus
that implemented the four would have a correct router between an agent and a
browser tab.

**Nothing would ask it for anything.** The requester is the agent, and the twelve
operations the contract names — `click`, `type`, `press`, `scroll`, `snapshot`,
`evaluate`, `waitFor`, `recordingStart` and the rest — are tools an agent calls,
not messages a UI sends. Upstream's agent reaches them over MCP:
`apps/server/src/mcp/` is an HTTP-transport MCP server, and
`provider/Layers/ClaudeAdapter.ts:3523` hands the CLI a per-thread session at
spawn time:

```ts
mcpServers: { <name>: { url: mcpSession.endpoint,
                        headers: { Authorization: mcpSession.authorizationHeader } } }
```

laplus drives the same `claude` CLI, so that mechanism transfers unchanged. But
laplus runs no MCP server. `crate::agent` mentions MCP only in doc comments about
`--permission-prompt-tool`.

So four declared methods depend, to be worth anything, on a subsystem **the
contract does not declare**. Nothing in `packages/contracts/src/rpc.ts` names it.
The ledger did not count it, because the ledger counts methods.

### Why this is a question about ownership

The obvious move is to make the MCP server part of the preview effort, since
preview is the only thing that needs it. Upstream's own layout argues against
that: `mcp/toolkits/` has exactly one entry, `preview`, and everything above it —
`McpHttpServer.ts`, `McpSessionRegistry.ts`, `McpInvocationContext.ts`,
`McpProviderSession.ts` — is generic. Of its 1,464 non-test lines, the
preview-specific parts are `PreviewAutomationBroker.ts` and the toolkit, and the
registry, transport and session plumbing are not.

It is also not a foreign body here. This server already serves HTTP
(`crate::http`, `crate::endpoints`) and already mints and verifies credentials
(`crate::auth`, `crate::codes`, ADR-0015 and ADR-0022). An MCP endpoint with a
per-thread bearer token is another route on a server that already does both,
rather than a second listener.

## Decision

**The MCP server is a platform surface with its own effort, and the automation
router ships without it.**

- **`mcp` is not part of the preview effort.** It gets its own feature directory
  and its own spec, citing the parity ledger the way the five parity efforts do.
  A toolkit directory whose first toolkit is preview will outlive preview, and an
  effort that has to stand up an entire second protocol before it can close is an
  effort that does not close.
- **The four automation methods are built inside the preview effort**, to the
  contract, and reach 60 of 60 with the rest. They are a router.
- **Their inertness is recorded rather than discovered.** Until the MCP effort
  lands, `previewAutomation.*` dispatches correctly and carries no traffic. The
  ledger says so under [Limits](../../../.scratch/contract-parity/ledger.md), and
  it is the reason that section now distinguishes _answered_ from _useful_.

## Consequences

- **60 of 60 will not mean every method does something.** Three automation methods
  and `subscribePreviewEvents` are the cases. This is the failure mode the parity
  figure has already had once — "26 of 71" was quoted in three files long after it
  stopped describing anything — so the distinction is written into the ledger
  rather than left to be noticed.

- **A second protocol enters this server.** Today the socket is the whole of what
  speaks to laplus from outside, plus the HTTP snapshot routes. MCP adds a third
  shape with its own session lifetime, and one that a _spawned child process_
  connects back into. `crate::auth` is the boundary for it as for everything else,
  but a per-thread credential handed to a CLI is a new kind of grant and wants its
  own reasoning when the effort is specced.

- **`previewAutomation.focusHost` is the one that is not routing.** It reaches the
  Tauri shell, so it needs a verb on ADR-0021's named list. That is preview's
  work, not MCP's.

- **Upstream's `mcp/` is clean of the surface this fork removed.** Checked file by
  file: `effect/*` imports and its own siblings, no cloud, relay or Clerk. Porting
  from it does not drag `94da6be` back in.

- **The order is fixed by this.** Preview can be built and shipped while MCP does
  not exist. MCP cannot usefully be built before the router it feeds. So preview
  goes first even though the automation half of it will sit quiet.

- **OpenCode consumes the same platform surface rather than growing its own.**
  Matching T3 Code, an owned OpenCode server receives the per-thread endpoint
  through `mcp.add`; an external OpenCode server receives none, because laplus
  does not mutate infrastructure it does not own. Full OpenCode parity therefore
  depends on the MCP effort rather than absorbing it into the provider driver.
