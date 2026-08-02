# Generic MCP platform

Status: ready-for-agent

## Problem Statement

Agents launched by Laplus need a scoped way to call host capabilities back in
Laplus. The existing WebSocket contract describes browser-facing automation,
but it does not provide the MCP transport, authentication, or lifetime that an
agent process needs. Provider drivers must not grow private MCP servers.

## Solution

Laplus hosts one generic Streamable HTTP MCP endpoint on its existing HTTP
listener. A conversation opens an MCP session before its owned agent is made
available. The returned handle contains a loopback endpoint and a bearer grant
scoped to that conversation; dropping or explicitly closing the handle revokes
the grant.

The platform implements MCP revision 2025-06-18 initialization and the tools
capability. Toolkits register tool definitions and calls behind the platform
interface. The initial registry may be empty: preview automation remains owned
by its separate effort, and provider adapters know neither tool names nor
routing internals.

Authentication and protocol state are distinct. The bearer grant selects the
conversation MCP session. This first implementation is otherwise stateless and
does not mint `Mcp-Session-Id`. It rejects missing, invalid, revoked, or
wrong-session grants without disclosing the grant and rejects non-loopback
Origins.

OpenCode is the first consumer. Only a Laplus-owned OpenCode process is
registered, using OpenCode 1.18.10's dynamic `mcp.add` operation before session
recovery or the first prompt. External OpenCode instances are not mutated.
Registration succeeds only when the returned status for Laplus is `connected`.
Every owned-session shutdown and startup-failure path closes the MCP handle.

Protocol and upstream evidence is recorded in
`protocol-and-opencode-wire-research.md`. ADR-0030 owns the generic/platform
scope decision; this spec does not restate parity figures.

## Acceptance

- MCP initialize, initialized, tools/list and tools/call cross the authenticated
  HTTP endpoint with correct JSON-RPC and HTTP status behavior.
- Session grants are unguessable, scoped, revocable, and redacted from Debug,
  errors, logs, and fixtures.
- The platform presents a small session-opening interface with production and
  fake adapters usable at the conversation seam.
- Owned OpenCode registration precedes session recovery and prompting, checks
  the named connected status, and is cleaned up on every lifetime path.
- External OpenCode infrastructure receives no registration request.
