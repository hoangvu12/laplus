# 01 — Capture the socket wire format from the reference server

**What to build:** A pinned, empirical description of the wire format the UI
speaks, captured from the reference TypeScript server rather than inferred from
type definitions. Stand the upstream server up, point the real UI at it, exercise
it by hand, and record the raw socket frames. The output is a set of fixtures plus
a written account of the framing: how a request is correlated with its response,
how a typed error comes back, and how a streaming subscription delivers chunks and
signals completion.

This exists because the transport framing is the project's primary risk. It is
undocumented, it comes from an explicitly unstable module of the Effect library,
and the reference implementation's dependencies are not installed in the vendored
checkout — so it cannot be settled by reading source. Every later ticket conforms
to what this one captures.

Note that the vendored upstream checkout needs its dependencies installed first,
and the package manager is provisioned via corepack and will download on first
use.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] The reference server runs locally and the unmodified UI connects to it
- [x] Raw socket frames are captured to fixture files committed to the repo
- [x] Captured coverage includes: the connection handshake, a successful
      request/response, a request that fails with a typed error, and a streaming
      subscription from first chunk through to termination
- [x] A written description of the framing accompanies the fixtures, covering
      request/response correlation, error tagging, and stream chunk/end semantics
- [x] The credential the client supplies at socket upgrade is identified, so the
      permissive local handshake can accept its shape
- [x] Anything observed but not understood is recorded as an open question rather
      than guessed at

## Comments

**2026-07-26 — agent.** Done. `Status: done` is a sixth label added to
`docs/agents/triage-labels.md`, whose closing line invites the vocabulary to be
edited; the upstream five are all triage roles describing undelivered work, so a
finished ticket had no honest label among them.

Outputs:

- `docs/socket-wire-format.md` — the written account, with an open-questions
  section and a note on what was redacted from the fixtures.
- `fixtures/socket-wire/*.ndjson` — six curated captures.
- `tools/wire-capture/` — the recording proxy, the scripted RPC client, and the
  curation step, with tests for the frame decoder and the credential redaction.

Findings that change later tickets:

- The framing *is* WebSocket framing. One JSON object per unfragmented text
  frame, no envelope of any kind above it — including for the two ~80 KB
  payloads captured.
- There is no transport-level handshake. The socket opens and the first frame is
  already a `Request`; capability negotiation is `server.getConfig` at the
  payload level, which confirms it as the tracer bullet for ticket 03.
- Two credential shapes at upgrade, not one: a `t3_session` cookie (what the
  browser sends) and a `wsTicket` query parameter (what non-browser clients
  send). The permissive local handshake has to accept both. A refused upgrade is
  a `401` with a JSON `EnvironmentAuthInvalidError` body, captured whole.
- The reference server declines `permessage-deflate`. Accepting it would
  compress every frame; declining is the compatible behaviour.
- Responses are correlated by `requestId` only. The UI genuinely issues
  concurrent calls and the server genuinely answers them out of order — a FIFO
  assumption would be broken against the reference server, never mind ours.
- A client-initiated unsubscribe terminates as `Exit`/`Failure` with an
  `Interrupt` cause, not as a success. Ticket 04 has to treat that as a normal
  end.
- `Chunk.values` is an array and does batch; ticket 04 must not assume one value
  per chunk.
- **`Ack` is real back-pressure.** The server holds at one un-acknowledged chunk
  per request. A committed shell change sat queued for two seconds behind a
  withheld `Ack` and arrived the instant it was sent. Ticket 04's server must
  respect this or a busy subscription's memory becomes unbounded; a Rust client
  that skips `Ack` stops receiving after one chunk.

Open questions most likely to bite later work: how deep the `Ack` window is
(one is observed, but only one change was ever outstanding), and what bounds a
`Chunk` batch. Both are in the doc, along with the fact that
`01-browser-session.ndjson` is the UI's boot sequence only — no capture drives
the UI through a user-facing flow, which tickets 04 onward will.
