# 04 — First streaming subscription

**What to build:** A server-streaming subscription that the UI subscribes to,
receives multiple updates from over time, and can cleanly unsubscribe from. Use
the server-configuration subscription — the simplest one available — so that the
streaming mechanism is proven in isolation rather than tangled with agent
semantics.

Streaming is a distinct framing mechanism from request/response, and eight
subscriptions plus the entire agent session lifecycle depend on it. Proving it
once, early, on the least complicated case means every later ticket inherits a
working mechanism instead of reinventing one. Chunk delivery, termination, and
client-initiated cancellation are all part of the framing captured in ticket 01.

**Blocked by:** 03 (Socket endpoint, local handshake, and the configuration
method).

**Status:** ready-for-agent

- [ ] The UI subscribes and receives an initial update
- [ ] A subsequent server-side change pushes a further update to the subscriber
- [ ] The stream terminates cleanly when the client unsubscribes
- [ ] Stream framing matches the ticket 01 captures
- [ ] Server-side resources for a subscription are released on unsubscribe and on
      abrupt disconnect
- [ ] Multiple concurrent subscribers each receive their own updates
- [ ] A test drives a subscription through the socket boundary and asserts the
      sequence of events and its termination
