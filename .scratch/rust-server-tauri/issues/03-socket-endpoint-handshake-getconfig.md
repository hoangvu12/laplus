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

**Status:** ready-for-human

- [x] The server listens on loopback and accepts a socket upgrade
- [x] The client's credential shape is accepted at upgrade; connections from
      non-local origins are rejected
- [ ] The configuration method returns a payload the real UI accepts —
      *conformant to the capture and to the contract, but never decoded by the
      real UI; see comments*
- [ ] The unmodified UI connects, completes its initial handshake, and remains
      connected without erroring or retrying — *blocked on tickets 04 and 23;
      see comments*
- [x] Responses match the ticket 01 captures for the same calls — structurally
      enforced, with three declared divergences; see the caveat in comments on
      what the check does and does not cover
- [x] A request for an unimplemented method returns a well-formed error the client
      understands, rather than dropping the connection
- [x] A test harness starts the server, connects a socket client, and drives the
      configuration method — this harness is reused by every later ticket
- [x] Disconnection and reconnection are handled without leaking server state

## Comments

**2026-07-26 — agent.** The transport is built and proven against two
independent client stacks. Six of eight criteria are met; the two that are not
both reduce to "we have never watched the real UI do this", and neither can be
closed at this ticket. `cargo build`, `cargo test` (70 tests) and
`cargo clippy --all-targets` are clean, no warnings.

### What landed

- `src/wire.rs` — the socket message vocabulary (`Request`/`Ack`/`Interrupt`/
  `Ping` in, `Exit`/`Chunk`/`Defect`/`Pong` out). Pure, like `protocol.rs`.
  Serialization is asserted byte-for-byte against the captured frames; an
  unrecognised `_tag` degrades to a counter instead of failing the parse.
- `src/auth.rs` — the permissive local handshake. A pure decision over the
  upgrade's query string and headers.
- `src/config.rs` — the `server.getConfig` payload, hand-written from
  `contracts/src/server.ts`.
- `src/http.rs` — the two plain HTTP answers the UI needs *before* it will open
  a socket.
- `src/rpc.rs`, `src/server.rs`, `src/main.rs` — dispatch, the `axum` endpoint
  and connection loop, and a binary (`--port`, `LIGHTCODE_PORT`, default 4773).
- `tests/harness/` — the primary test seam, reused by every later ticket:
  `mod.rs` (start a server, connect a client, drive methods), `captures.rs`
  (read the ticket 01 recordings back into frames), `shape.rs` (structural
  comparison with declared divergences).
- `tests/socket_handshake.rs` (14), `tests/socket_conformance.rs` (7),
  `tests/http_boot.rs` (6).

### The one deliberate divergence from a capture

**lightcode does not send `Defect`.** The reference server answers an unknown
tag with a bare `Defect`; lightcode answers with `Exit`/`Failure` under the
caller's `requestId`, carrying a `ServerMethodNotImplementedError`.

This answers open question 4 in `docs/socket-wire-format.md`, which is updated.
The client handles `Defect` as `clearEntries(Exit.die(message.defect))`
(`effect/unstable/rpc/RpcClient.ts`, `effect@4.0.0-beta.78`): it fails *every*
in-flight request and *every* open subscription on that socket, and the
connection supervisor then reconnects on a 1/2/4/8/16-second backoff. An `Exit`
is scoped by comparison — `decodeExit(...)` is wrapped in `matchCauseEffect`, so
even an error payload that fails to decode is written back under the same
`requestId` and nothing else is touched.

The reference server can afford `Defect` because it implements every tag its
client sends, so one only ever answers a tag no real client uses. lightcode
implements one method of roughly sixty, so during the build-out `Defect` would
be the *normal* answer to the UI's own boot sequence, and each one would tear
down the session. Read from source, not captured — no recording provoked a
`Defect` against the real UI. `socket_conformance.rs` pins both halves: what
the capture holds, and what we do instead.

### How conformance is checked

`shape.rs` walks the captured `server.getConfig` response against a live one and
classifies every difference as missing / added / retyped / uncompared. Each must
be **declared with a reason** in `socket_conformance.rs`. It fails two ways, and
both matter: an undeclared difference is drift nobody decided on, and a
declaration nothing matches is an excuse that has outlived its cause — which is
how a later ticket filling in `providers` or `keybindings` will find out it
should delete the note. `the_comparison_catches_each_kind_of_drift` exists
because a structural check that cannot fail is worse than no check.

Ten fields are declared missing (three unbuilt capabilities, four drivers v1
does not ship, `textGenerationModelSelection`, and the two resume markers) and
six arrays uncompared because one side is empty. Nothing is added or retyped.

**What the check does not cover, and it is worth being precise about:** it
compares *keys and JSON types*, never *values*. Two divergences are therefore
prose-only rather than enforced — `settings.enableProviderUpdateChecks` is
`false` where the capture holds `true`, and `auth.bootstrapMethods` is `[]`
where the capture holds `["one-time-token"]` (the latter recorded merely as an
uncompared array). Both are deliberate and reasoned above. A value-aware check
would need an allow-list of fields that *legitimately* differ per machine —
`cwd`, `label`, `serverVersion`, every path — which is a bigger design than
this ticket needed. Worth building if a third value divergence shows up.

### Payload decisions worth knowing

- `environmentId` is the constant `"local"`. The contract types it as a
  non-empty string, not a UUID, and v1 has exactly one environment — a generated
  id would have to be persisted to stay stable across restarts.
- `enableProviderUpdateChecks` is **false**, where upstream defaults it true.
  Story 7 is that the app runs entirely on the user's machine with no network
  service of its own; an update check on boot contradicts that. Ticket 22 can
  make it a user-facing choice.
- No unbuilt capability is advertised. `connectionProbe` is absent, so the
  client probes with a second `server.getConfig` — which is why
  `get_config_is_repeatable` is a test.
- `/api/auth/session` always answers `authenticated: true`. There is no state in
  which a local client is *un*authenticated, and `false` would send the UI to a
  pairing screen with no pairing flow behind it.
- The payload is ~1.2 KB against the reference server's 80 KB, because
  `providers`, `keybindings` and `availableEditors` are empty until tickets 09,
  22 and story 18.

### Why the last two criteria are open

Tracing the UI's boot (`apps/web`, `packages/client-runtime`) turned up three
things standing between us and a browser, and a fourth behind them:

1. The UI derives its socket URL from `window.location.origin`
   (`environments/primary/target.ts`), so **the server must also serve the UI
   bundle**. `VITE_HTTP_URL`/`VITE_WS_URL` are build-time and set nowhere.
   Owner: ticket 23.
2. `GET /api/auth/session` blocks rendering. **Done here.**
3. `GET /.well-known/t3/environment` gates the socket being opened at all; its
   failure is swallowed and retried every 3 s, so its absence looks like a UI
   that simply never connects. **Done here.**
4. Even with all three, the UI mounts `subscribeServerConfig`,
   `subscribeServerLifecycle` and `subscribeShell` unconditionally, and retries
   a failing shell subscription every 250 ms. Owner: ticket 04.

Item 1 was deliberately left out of scope after checking with the maintainer —
serving the bundle is ticket 23's, and adding it here would still not close the
criterion, because of item 4. So the honest position is: the transport is proven
against two independent client stacks (the Rust harness over
`tokio-tungstenite`, and Node's built-in `WebSocket`/`fetch` driven by hand),
and unproven against the browser UI. **The first person to have both ticket 04
and a way to serve `apps/web/dist` should close these two boxes by eye.**

### Loose ends for later tickets

- Requests are answered in arrival order. The protocol does not require it —
  correlation is by `requestId` and the reference server genuinely answers out
  of order. The first method that has to wait on something should make the
  connection loop concurrent.
- `Ack` and `Interrupt` are accepted and ignored, because nothing streams yet.
  Ticket 04 owns real back-pressure, and `docs/socket-wire-format.md` is
  emphatic that ignoring `Ack` turns a busy subscription's memory from bounded
  to unbounded.
- The origin check matches on host and **ignores the scheme**, which cuts the
  opposite way from what you might expect and matters for ticket 23. A Tauri
  build pointed at `http://127.0.0.1:<port>` passes. So does `tauri://localhost`
  — the host is `localhost`, and nothing looks at the scheme. But
  `http://tauri.localhost`, which is the origin Tauri v2 actually uses on
  Windows, is **refused**: the host is `tauri.localhost`, which is neither
  `localhost` nor a `127.0.0.0/8` address. Ticket 23 has to establish which
  origin the shell really presents and widen `is_local_origin` to match, rather
  than assuming the loopback rule already covers it.
- A non-local origin is refused as `invalid_credential`, which is not what was
  actually wrong. `EnvironmentAuthInvalidReason` is a closed union of
  `missing_credential | invalid_credential`, and upstream has no origin check to
  have added a third member for; reusing the closed union keeps the body
  decodable by the unmodified client. The real cause is logged, not sent.
- `src/http.rs` is the least well-founded code in this ticket. Ticket 01's proxy
  recorded `/ws` connections only, so those two endpoints are written from the
  contract with no capture behind them.
