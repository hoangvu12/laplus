# Generic MCP HTTP transport and OpenCode `mcp.add` wire research

Research date: 2026-08-02. The provider target is OpenCode `v1.18.10`
(`7902e04c3a67f7c69726bc955efb46e29214c797`). Protocol claims use the
official MCP 2025-06-18 specification because it is a published revision that
OpenCode's pinned MCP SDK supports.
This note records facts for the later generic MCP spec; it does not choose the
platform API, grant shape, route layout, or first toolkit.

## Conclusions that constrain the later spec

- Laplus needs one Streamable HTTP endpoint URL per advertised MCP endpoint.
  The path accepts POST and GET; DELETE is part of optional stateful-session
  cleanup. A minimal server may decline the standalone GET stream with 405 and
  return JSON for request POSTs, while still being Streamable HTTP compliant.
- An HTTP server cannot skip MCP initialization. `initialize` is the first
  protocol interaction, the server returns its negotiated version,
  capabilities and identity, and the client then sends
  `notifications/initialized`. For the first toolkit, advertising `tools`
  entails implementing at least `tools/list` and `tools/call` with their MCP
  schemas.
- Authentication and MCP protocol sessions are separate layers. ADR-0030's
  per-thread bearer value can protect the endpoint, while `Mcp-Session-Id` is a
  server-minted protocol session identifier if the implementation elects to be
  stateful. Neither substitutes for the other.
- OpenCode registration is `POST /mcp`, not an MCP JSON-RPC call. Its body is
  `{ "name": ..., "config": { "type": "remote", "url": ..., "headers":
{ "Authorization": ... }, "oauth": false } }`; `directory` is an optional
  query parameter selecting the OpenCode instance context. The response is a
  map keyed by MCP server name, and HTTP 200 does not prove connection success:
  the selected entry must have `status: "connected"`.
- `mcp.add` connects immediately. It first tries Streamable HTTP, then the
  deprecated SSE client transport if the first transport fails. Supplying
  `oauth: false` prevents OpenCode's OAuth auto-detection; arbitrary configured
  headers are forwarded on transport requests in either mode.
- Dynamic additions live in the running OpenCode instance's in-memory MCP
  configuration. Re-adding the same name replaces the connection and closes
  the old client. OpenCode exposes no corresponding dynamic remove endpoint;
  its disconnect operation closes the client but retains a disabled status.
  Consequently, Laplus lifetime cleanup cannot be specified as a symmetric
  `mcp.remove` wire call that OpenCode 1.18.10 does not have.

## Streamable HTTP requirements

These are requirements of revision 2025-06-18, not of the current draft. The
draft has since removed protocol-level sessions and GET streams and added
`Mcp-Method`/`Mcp-Name` request headers. Importing those draft rules selectively
would produce a transport that does not faithfully implement either revision.
[Current draft changelog](https://modelcontextprotocol.io/specification/draft/changelog)

MCP messages are UTF-8 JSON-RPC 2.0. A Streamable HTTP server provides a single
endpoint supporting POST and GET. It must validate `Origin` on incoming
connections; a local server should bind only to loopback and should authenticate
all connections. [MCP 2025-06-18 transport: encoding, endpoint, and security](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#streamable-http)

Each client JSON-RPC message is a fresh POST. The client sends an `Accept`
header supporting both `application/json` and `text/event-stream`, and the body
is exactly one request, notification, or response. Accepted notifications and
responses receive empty HTTP 202. A request receives either one JSON response
with `Content-Type: application/json` or an SSE stream with
`Content-Type: text/event-stream`; clients must accept both. The SSE response
should eventually contain the matching JSON-RPC response and normally closes
after it. [MCP POST behavior](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#sending-messages-to-the-server)

A client may separately GET the endpoint with `Accept: text/event-stream` for
server-initiated traffic. A server that does not offer that stream must answer 405. Resumability is optional; when implemented, SSE event IDs are unique
within their protocol session/connection, and a reconnect may send
`Last-Event-ID`. [MCP GET behavior](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#listening-for-messages-from-the-server) and [resumability](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#resumability-and-redelivery)

Stateful protocol sessions are optional. A server opting in returns a
cryptographically secure, visible-ASCII `Mcp-Session-Id` header with the
initialize result; the client repeats it on every later request. Unknown or
expired IDs produce 404, prompting a fresh initialize. Clients should DELETE
the endpoint with that header when finished, though a server may answer 405 if
client-initiated termination is unsupported. Subsequent HTTP requests also
carry the negotiated `MCP-Protocol-Version`; an invalid or unsupported value
requires 400. [MCP session management and version header](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports#session-management)

The initialization phase is mandatory and first. The client sends
`initialize` with `protocolVersion`, capabilities, and `clientInfo`; the server
returns a supported version, its capabilities, and `serverInfo`; the client
then sends `notifications/initialized`. Normal operations must stay within the
negotiated capabilities. [MCP lifecycle and initialization](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#initialization)

For a tool server, the server declares the `tools` capability. `tools/list`
returns tool definitions (including object `inputSchema`) and supports cursor
pagination; `tools/call` names a tool and supplies `arguments`. Tool execution
failures belong in a successful protocol result with `isError: true`, while
unknown tools, invalid arguments, and other protocol errors are JSON-RPC
errors. Servers must validate tool inputs, implement access controls, rate-limit
calls, and sanitize outputs. [MCP tool capability and messages](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#protocol-messages), [tool error handling](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#error-handling), and [tool security](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#security-considerations)

The published authorization specification defines OAuth-style discovery and
token use for HTTP transports, but the base protocol explicitly permits clients
and servers to negotiate custom authentication strategies. Thus the bearer
grant in ADR-0030 is an application security design to specify, not an MCP
JSON-RPC message or an `Mcp-Session-Id`. [MCP base-protocol authorization boundary](https://modelcontextprotocol.io/specification/2025-06-18/basic#auth)

## Exact OpenCode 1.18.10 registration wire

OpenCode pins `@modelcontextprotocol/sdk` 1.29.0. That SDK initially offers
protocol version `2025-11-25`, but explicitly supports `2025-06-18` (as well as
older revisions). Under MCP version negotiation, a Laplus server may respond
with `2025-06-18`; this client accepts it and uses that value in subsequent
`MCP-Protocol-Version` headers. Claiming 2025-11-25 would instead commit the
server to that revision's complete contract. [OpenCode dependency lock](https://github.com/anomalyco/opencode/blob/v1.18.10/bun.lock#L595-L596) and [MCP TypeScript SDK 1.29.0 supported versions](https://github.com/modelcontextprotocol/typescript-sdk/blob/e12cbd7078db388152f6e839abdbe09ba01f3f32/src/types.ts#L4-L6)

The v2 SDK's `mcp.add` operation sends `POST /mcp` with JSON content type. Its
optional query parameters are `directory` and `workspace`, and the JSON body is
`{ name: string, config: McpLocalConfig | McpRemoteConfig }`. Success is HTTP
200 with `{ [name: string]: McpStatus }`; schema/invalid-request failures are
HTTP 400. [Generated SDK method](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/sdk.gen.ts#L2425-L2461) and [generated wire types](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L8457-L8488)

For Laplus's HTTP endpoint the remote config schema is:

```json
{
  "type": "remote",
  "url": "http://127.0.0.1:<port>/<per-thread-path>",
  "headers": {
    "Authorization": "Bearer <scoped-secret>"
  },
  "oauth": false
}
```

`enabled` and positive-integer `timeout` are optional. `oauth` is either an
OAuth configuration or literal `false`; omitting it enables auto-detection, so
`false` is the unambiguous choice for a pre-authorized private endpoint.
[OpenCode remote MCP schema](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/core/src/v1/config/mcp.ts#L44-L65)

The route handler passes `name` and `config` to the MCP service and normalizes
the returned value into a status map. The service stores the config in its
instance-local dynamic map, attempts a connection immediately, and returns the
whole status map. [OpenCode HTTP route and payload](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/server/routes/instance/httpapi/groups/mcp.ts#L11-L66), [route handler](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/server/routes/instance/httpapi/handlers/mcp.ts#L13-L23), and [dynamic add implementation](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/mcp/index.ts#L627-L647)

Connection tries `StreamableHTTPClientTransport` first and legacy
`SSEClientTransport` second, passing configured headers as `requestInit` to
both. Connection or discovery errors become a `failed` status rather than
necessarily failing the outer HTTP request. OAuth-related outcomes can instead
be `needs_auth` or `needs_client_registration`. [OpenCode remote connection](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/mcp/index.ts#L236-L330) and [status union](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L2379-L2411)

OpenCode's own test proves that configured authorization/custom headers appear
on every observed Streamable HTTP request both when OAuth is defaulted and when
it is explicitly disabled. [OpenCode header forwarding test](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/test/mcp/headers.test.ts#L35-L101)

The `directory` query parameter selects the local instance context; absent it,
OpenCode falls back to `x-opencode-directory`, then the server process cwd.
Ticket 19 should therefore use the same workspace-directory routing already
used for the owned session rather than rely on process cwd. [OpenCode workspace routing](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/server/routes/instance/httpapi/middleware/workspace-routing.ts#L22-L29) and [directory fallback](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/server/routes/instance/httpapi/middleware/workspace-routing.ts#L85-L88)

Replacement is observable lifecycle behavior: adding the same name installs
the new connected client and then closes the previous client. A failed add can
coexist with other connected servers. [OpenCode replacement and failure tests](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/test/mcp/lifecycle.test.ts#L331-L390)

## Questions deliberately left for the feature spec

- Whether Laplus uses a stateless MCP transport or also mints protocol session
  IDs; per-thread endpoint ownership does not decide this.
- Which published protocol revisions the server accepts in addition to the
  OpenCode 1.18.10 client's offer, and whether deprecated HTTP+SSE compatibility
  is worth supporting beyond OpenCode's automatic fallback.
- The generic platform interface, endpoint naming, grant representation and
  revocation semantics, toolkit registry, tool authorization policy, and exact
  cleanup owner.
- The stable OpenCode registration name and how an owned OpenCode process is
  stopped/restarted to clear its in-memory dynamic registration. These are
  product/lifecycle decisions, not facts supplied by the upstream wire.
