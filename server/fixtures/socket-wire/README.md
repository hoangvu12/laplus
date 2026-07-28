# Socket wire captures

Recordings of the **reference TypeScript server** answering the real UI, made
in ticket 01 through a byte-transparent proxy. They pin the protocol the UI
speaks, and `crates/lightcode-server/tests/socket_conformance.rs` holds
lightcode's answers against them.

Sibling of `fixtures/claude-cli/`, which pins the _other_ protocol. That
directory pins the one the agent speaks; this one pins the one the UI speaks.

**`docs/socket-wire-format.md` is the document to read first.** It describes the
framing these files evidence — request/response correlation, error tagging,
chunk/end semantics, the credential at upgrade — and every claim in it names the
capture that backs it. It also carries the open questions, which matter as much
as the answers.

## The captures

| File                                       | What it holds                                                                                                                                                      |
| ------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `01-browser-session.ndjson`                | The unmodified UI's boot sequence: upgrade, `server.getConfig`, six subscriptions, four concurrent unary calls answered out of order, then the idle keepalive loop |
| `02-request-response.ndjson`               | A single successful `server.getConfig`, plus `Ping`/`Pong`                                                                                                         |
| `03-typed-error.ndjson`                    | A `projects.readFile` failing with a typed `ProjectReadFileError`, and an unknown method tag answered with `Defect`                                                |
| `04-streaming-subscription.ndjson`         | The minimal subscription lifecycle: first chunk, `Ack`, client `Interrupt`, terminal `Exit`                                                                        |
| `05-orchestration-and-backpressure.ndjson` | The orchestration surface end to end, including a withheld `Ack` stalling the stream across a committed change                                                     |
| `06-upgrade-rejected.ndjson`               | An upgrade with no credential: `401` and its JSON body; the socket never opens                                                                                     |

Each line is one record — `connection-opened`, `http-request`, `http-response`,
`http-response-body`, `ws-frame`, `ws-message`, `error`, `connection-closed` —
carrying `seq` and `tMs`. `ws-message` holds the assembled payload as `text`,
byte-for-byte as it crossed the wire.

## Provenance and redaction

Recorded with `tools/wire-capture/`, curated from
`.scratch/wire-capture/raw/`, which is gitignored because it holds live session
tokens. Only the credentials presented at upgrade are redacted, replaced by a
marker naming the token's claim names and length; everything else passes
through unaltered. `docs/socket-wire-format.md` explains what was deliberately
_not_ redacted, and why.

Adding a capture means re-running the proxy and the curation step — see the
"How the captures were made" section of that document.
