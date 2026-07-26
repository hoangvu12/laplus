# 03 — Socket endpoint, local handshake, and the configuration method

**What to build:** The unmodified upstream UI connects to the Rust server,
completes its handshake, receives a well-formed server configuration, and stays
connected. Nothing else works yet — but the transport is proven.

This is the project's tracer bullet. The client fetches server configuration as
its very first call and can do nothing at all until a valid response arrives, so
satisfying exactly that one method against the real UI is the thinnest slice that
demonstrates the whole transport: socket upgrade, authentication handshake,
request/response framing, and payload schema conformance, all at once.

Authentication is local-only and permissive by design. Accounts are out of scope,
but the handshake shape is not optional — the client sends a credential and the
server must accept it. Bind to loopback, accept the client's handshake shape,
reject non-local origins, and verify the credential against nothing.

Conformance is checked against the frames captured in ticket 01: the Rust server's
responses are compared to what the reference server produced for the same calls.

**Blocked by:** 01 (Capture the socket wire format), 02 (Cargo workspace and
protocol module).

**Status:** ready-for-agent

- [ ] The server listens on loopback and accepts a socket upgrade
- [ ] The client's credential shape is accepted at upgrade; connections from
      non-local origins are rejected
- [ ] The configuration method returns a payload the real UI accepts
- [ ] The unmodified UI connects, completes its initial handshake, and remains
      connected without erroring or retrying
- [ ] Responses match the ticket 01 captures for the same calls
- [ ] A request for an unimplemented method returns a well-formed error the client
      understands, rather than dropping the connection
- [ ] A test harness starts the server, connects a socket client, and drives the
      configuration method — this harness is reused by every later ticket
- [ ] Disconnection and reconnection are handled without leaking server state
