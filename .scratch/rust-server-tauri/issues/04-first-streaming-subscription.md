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

**Status:** done

- [x] The UI subscribes and receives an initial update — _proven against the
      harness client, not a browser; see comments_
- [x] A subsequent server-side change pushes a further update to the subscriber
- [x] The stream terminates cleanly when the client unsubscribes
- [x] Stream framing matches the ticket 01 captures
- [x] Server-side resources for a subscription are released on unsubscribe and on
      abrupt disconnect
- [x] Multiple concurrent subscribers each receive their own updates
- [x] A test drives a subscription through the socket boundary and asserts the
      sequence of events and its termination

## Comments

**2026-07-26 — agent.** Streaming works and is proven at the socket boundary.
All seven criteria are met, with one caveat on the first that is the same
caveat ticket 03 carries: no browser has done any of this yet, because nothing
serves the UI bundle until ticket 23. `cargo build --all-targets`, `cargo test`
(105 tests, up from 70) and `cargo clippy --all-targets` are clean, no warnings.

### What landed

- `src/subscriptions.rs` — the streaming mechanism, and the only new _idea_ in
  this ticket. An `EventSource` is a snapshot function plus a feed of updates;
  a `Subscriptions` registry owns the pumps open on one connection; the pump
  itself is one loop. It knows nothing about configuration, so the
  orchestration, terminal, file-tree and git subscriptions can arrive later
  without touching it.
- `src/config_store.rs` — the configuration as a _live_ value rather than a
  constant, with the closed set of changes that can happen to it
  (`ConfigChange`, mirroring the contract's three update events). Ticket 09
  publishes `Providers`; ticket 22 publishes `Keybindings` and `Settings`.
  Nothing publishes yet, which is why the tests are the only caller.
- `src/server.rs` — the connection is now a read loop, a frame queue and a
  writer task, because the read loop is no longer the only thing producing
  frames. This closes the "make the connection loop concurrent" loose end
  ticket 03 left, in the form that ticket predicted. The mutable half is a
  `Connection` — state, subscriptions, frame queue — which is also what the
  module's own tests drive, with the socket taken off the end.
- `src/rpc.rs` — `dispatch` answers with an `Answer`, which is either a value
  or a stream. Nothing in a `Request` says which it will be; the method name
  carries that and the client already knows it.
- `tests/socket_streaming.rs` (18) — the ticket's criteria, at the socket.
  Two conformance tests in `socket_conformance.rs`, unit tests in the two new
  modules, and the existing `server.rs` frame tests rewritten against the queue.

### The three decisions worth knowing

**`Ack` is implemented as real back-pressure, with a window of one.** The
server sends one chunk and stops until the client answers. This is the
conservative reading of what fixture 05 shows the reference server doing —
open question 2 asked whether one was its fixed window or just all that
workload produced, and a client written against a window of one works against
any deeper one. It is also the difference between a busy subscription's memory
being bounded and unbounded, which `docs/socket-wire-format.md` is emphatic
about.

**A batch is whatever accumulated behind the outstanding `Ack`, capped at 64.**
This answers open question 1 for lightcode — the reference server's own policy
is still unknown. The cap and the backlog are deliberately the same number, so
a subscriber that drains a full chunk can never be the reason the next one lags.

**Past the backlog, a subscriber is resynchronised rather than caught up.** A
client that stops acknowledging cannot make the server hold an unbounded queue
on its behalf; past 64 events the cheapest correct answer is to send a fresh
snapshot, which supersedes everything that was dropped. The client's projection
treats a snapshot as a reset, so this is a resynchronisation and not a gap.
`a_subscriber_that_falls_far_behind_is_resynchronised_with_a_snapshot` pins it,
and `subscriptions.rs` has the same test one layer down.

This one had a bug in it that the review caught, and it is worth recording
because the mistake is inviting: **a lagged `broadcast` receiver is not
emptied, it is fast-forwarded to the oldest value it still holds.** Sending
the snapshot alone therefore left the superseded backlog queued behind it, and
a client applying wholesale replacements would have walked its configuration
backwards through values the server had already left before arriving where the
snapshot had put it. `EventSource::resynchronise` now drains first and
describes second — that order, because an event landing between the two is
then merely delivered twice, which the projection absorbs, where the other
order would drop it. Both tests were extended to assert on what follows the
snapshot, which is what let the bug through in the first place.

### `fiberId` is invented, and it has to be

The captured terminal frame is
`{"_tag":"Interrupt","fiberId":2494}` — a real Effect runtime fiber id on the
machine that produced it. lightcode has no fibers. It allocates a distinct
number per cancelled call, which is the nearest true thing: what the client
reads is the `_tag`, which tells it the stream ended normally rather than in
error. A constant would have passed every test here and been a lie about what
the field names.

### On "the UI subscribes"

Everything above is driven by the Rust harness client over `tokio-tungstenite`
— a second, independent WebSocket stack, which is why a passing test means two
implementations agree rather than one agreeing with itself. It is not a
browser. Ticket 03's last two criteria stay open for the same reason they were
opened: the UI derives its socket URL from `window.location.origin`, so nothing
can connect until the server also serves `apps/web/dist` (ticket 23).

Ticket 03's note listed a fourth obstacle behind that one and gave it to this
ticket: the UI mounts `subscribeServerConfig`, `subscribeServerLifecycle` and
`subscribeShell` unconditionally. **This ticket delivers the first of the
three**, which is what its own scope asked for — the mechanism, proven in
isolation. The other two are payload work on a mechanism that now exists:

- `subscribeShell` is the orchestration surface and belongs with tickets 10–12.
  Its failure is the loud one: the UI retries it every 250 ms.
- `subscribeServerLifecycle` (the `ready`/`welcome` pair, feeding the UI's
  welcome state) **has no owner in the ticket list.** It is small — two events
  from a capture — but it needs `cwd` and a project name, so it most naturally
  lands with ticket 05. Whoever picks up 05 should decide.

### Loose ends

- **`server.getConfig` and the subscription now read one store**, so they
  cannot disagree — there is a test for exactly that, because two sources of
  the same payload is precisely the bug this would otherwise grow. Ticket 09
  and ticket 22 should publish through `ConfigChange` rather than rebuilding a
  `ServerConfig`, or the subscription will silently stop reflecting reality.
- **A subscription that ends by itself has never happened**, so open question 5
  is still open. The configuration stream runs until the client cancels it.
  The first subscription that can genuinely complete — a finished thread, an
  exited terminal — has to decide whether that is `Exit`/`Success`.
- **Request-id reuse** (open question 8) is still unobserved. A second
  subscription on an id that already has one now replaces it, logs, and
  releases the old pump. Nothing in a capture says that is right; it is simply
  the option that cannot leak.
- **Unary calls are still answered inline, in arrival order.** Both implemented
  methods answer from memory. The frame queue means a slow method would now
  only block the read loop rather than the whole connection, but the first
  method that genuinely waits should still spawn.
- The two `*ResumeCompletionMarker` divergence notes in
  `socket_conformance.rs` were re-pointed at the orchestration tickets, since
  they were attributed to this one.
