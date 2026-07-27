# `claude` CLI captures

Golden inputs for `crates/lightcode-server/tests/protocol_golden.rs`, the drift
detector over the CLI's stdio wire format. Each `NN-<name>.ndjson` is folded
line by line through a fresh `SessionState`; the result is compared against
`NN-<name>.expected.json`.

Sibling of `fixtures/socket-wire/`, which pins the *other* protocol — the one
the UI speaks. This directory pins the one the agent speaks.

Since ticket 10 they are also **replayed**: `harness::agent::ScriptedAgent`
writes a stand-in `claude` that reads a turn on stdin and prints one of these
files back, so `tests/socket_turn.rs` drives a whole turn through the socket
against what the CLI actually said. That gives a capture two jobs — the reducer
is held to it, and the server is driven by it — and it is why re-capturing after
a `claude` release is worth doing even when the golden files still match.

A capture containing a `control_request` is replayed with a stop where the
request is: the CLI waits there for an answer, and everything after that line in
the recording happened *because of* the answer. Playing it straight through would
have the stand-in react to a decision nobody had made.

A capture containing a `control_response` is replayed with a stop *before* it,
for the mirror-image reason: that line is the CLI answering something the server
asked it, and the request travelled on stdin so it is not in the recording.
Playing it straight through would have the stand-in answer a request nobody had
sent, and then abort a turn nobody had stopped.

## The captures

| File | Provenance | What it covers |
| --- | --- | --- |
| `01-buffered-turn.ndjson` | Recorded, STEP 1 spike (`.scratch/stream-sample.ndjson`) | A turn without `--include-partial-messages`: buffered `assistant` message only, no deltas to reconcile against |
| `02-streamed-turn.ndjson` | Recorded, STEP 1 spike (`.scratch/bidi.ndjson`) | A turn with `--include-partial-messages`: `message_start` → deltas → `message_stop`, reconciled by the buffered message that agrees with them |
| `03-synthetic-drift.ndjson` | Hand-written | Degradation. An unrecognized top-level `type`, an unrecognized `stream_event`, an unrecognized content block, a truncated line, a blank line — none of which a healthy CLI emits, so no recording contains them |
| `04-tool-use.ndjson` | Recorded, ticket 12 | One tool call end to end: reasoning, a `tool_use`, its `tool_result` as a `user` message, more reasoning, then the reply |
| `05-tool-failure.ndjson` | Recorded, ticket 12 | The same shape with `"is_error": true` on the result — a `Read` of a file that is not there |
| `06-several-tool-calls.ndjson` | Recorded, ticket 12 | Two calls in one turn, each answered before the next is announced |
| `07-permission-approved.ndjson` | Recorded, ticket 13 | A `control_request` asking to use `Write`, approved — the tool then succeeds |
| `08-permission-declined.ndjson` | Recorded, ticket 13 | The same request declined: the tool result carries the refusal, and the agent answers anyway |
| `09-permission-unanswered.ndjson` | Recorded, ticket 13 | The same request left hanging until stdin closed — the CLI abandons it and finishes the turn |
| `10-permission-for-the-session.ndjson` | Recorded, ticket 13 | Approved with the CLI's own permission suggestion handed back: two `Write` calls, one request |
| `11-interrupted-turn.ndjson` | Recorded, ticket 14 | A reply stopped mid-sentence: the acknowledgement, the partial text handed over whole, and an aborted `result` |
| `12-interrupt-then-continue.ndjson` | Recorded, ticket 14 | The same, and then a second turn answered normally by the same process |
| `13-interrupt-during-tool-use.ndjson` | Recorded, ticket 14 | A stop in the middle of a run of `Write` calls, as the agent was opening the next one |
| `14-interrupt-with-nothing-running.ndjson` | Recorded, ticket 14 | A stop sent after the turn had already ended — acknowledged, and nothing else happens |
| `15-permission-cancelled.ndjson` | Recorded, ticket 14 | "Cancel" on a permission: a denial carrying `interrupt: true`, which ends the turn the way an interrupt does |
| `16-error-result.ndjson` | Hand-written | A turn the agent reports as failed, with its reason in the `result`'s `errors` array — which is the only thing in a failed turn a developer can act on |
| `17-rate-limited.ndjson` | Hand-written | Three `rate_limit_event`s — fine, close to the limit, refused — and the failed turn the third produces |
| `18-compacted.ndjson` | Hand-written | A `system`/`compact_boundary` between two turns: the agent's memory rewritten, and the transcript deliberately unchanged by it |

The `.scratch/*.ndjson` originals stay where they are as raw evidence; these are
the committed, test-facing copies. `04`–`15` were recorded straight into
`fixtures/` against `claude-haiku-4-5`, with the same flags
[`crate::agent`](../../crates/lightcode-server/src/agent.rs) passes.

## What `16`–`18` are, and why they are not recordings

Ticket 15's three cases have one thing in common: a healthy CLI on a healthy
account does not produce them on demand. An API failure cannot be asked for, a
usage limit cannot be reached to order, and a compaction takes a conversation
long enough to fill the context window — so these are hand-written, like `03`,
and the *shapes* are read off the `claude` binary rather than guessed:

- **`rate_limit_event` carries `rate_limit_info`**, whose fields are the API's
  own response headers in the CLI's camel case: `status` (`allowed`,
  `allowed_warning`, `rejected`), `rateLimitType`, `resetsAt`, `utilization`.
  Only the two standings a developer can act on are surfaced — see
  `RateLimit::worth_reporting`, and `17`'s first notice, which is not.
- **`compact_boundary` carries `compact_metadata`** with `trigger` (`auto` or
  `manual`) and the token counts either side. Every field is optional, and a
  boundary with none of them is still a boundary.
- **A failed `result` puts its reason in `errors`, in `result`, or in neither** —
  which is why `ResultEvent::complaint` reads all three in that order and falls
  back to the subtype. `11-interrupted-turn.ndjson` is the recorded proof that
  `errors` is real.

The one thing no capture can contain is the fourth failure mode ticket 15 covers:
an agent that **dies mid-stream**. A CLI that crashes has, by definition, not
finished writing the recording. `harness::agent::DIES` is where that lives
instead, and `tests/socket_resilience.rs` is what drives it.

## What `11`–`15` settled

The interrupt channel is not in `--help` either, and it is the same envelope as
the permission one travelling the other way. What these five record is that it
exists, what it is, and — the part that could not have been guessed — how a
stopped turn *ends*:

- **The request is a `control_request` on stdin** with `{"subtype": "interrupt"}`
  and an id this server mints. The CLI's schema has an optional `reason` beside
  it, forwarded to the turn's abort signal; lightcode sends none. No flag turns
  this on: `--input-format stream-json` is itself a control channel.
- **The answer is a `control_response` naming the same id**, and on `11`–`14` it
  is `{"subtype": "success", "response": {"still_queued": []}}` every time.
  `15` has none, because a cancelled permission stops the turn without any
  request being sent — the stop travels on the decision.
- **A stopped turn is reported as a failed one.** `"is_error": true`, subtype
  `error_during_execution`, `terminal_reason` `aborted_streaming`. Nothing in the
  output distinguishes "the developer pressed stop" from "the turn went wrong",
  which is why [`crate::turn`] carries its own flag rather than reading one.
- **The partial reply is handed over whole.** In `11` and `12` a buffered
  `assistant` message arrives after the acknowledgement carrying exactly what had
  streamed, followed by a `user` message reading
  `[Request interrupted by user]` — the CLI's own marker, which this server does
  not publish because it publishes a row of its own.
- **The session survives.** `12` is the proof and the only one that can be: the
  same process took a second turn afterwards and answered it normally, exiting 0.
- **A stop with nothing to stop is acknowledged and does nothing.** `14` is an
  interrupt sent after the `result`; there is no second `result` and no error.
- **"Cancel" on a permission is an interrupt too.** `15`'s turn ends exactly as
  `11`'s does, marker and all. Ticket 13 sent that decision correctly and left
  what the CLI does with it untested; this is it.

`tools/interrupt-capture/record.mjs` made `11`–`14` and
`tools/permission-capture/record.mjs` made `15`. Neither capture can be produced
by hand, and `11`–`13` cannot be produced by a clock either: at four seconds the
model was still thinking and at twenty the whole turn had finished, so the
recorder triggers on what it is waiting for — the fortieth text delta, or the
second tool call.

## What `07`–`10` settled

The CLI's permission channel is not in `--help` and not in any contract this
repository has. What these four record is that it exists and what it is:

- **The prompt arrives as a `control_request` on stdout**, with
  `"subtype": "can_use_tool"`, and the CLI **stops** until a `control_response`
  comes back on stdin. It only does so when started with
  `--permission-prompt-tool stdio` — a hidden flag whose documented description
  is "MCP tool to use for permission prompts", where `stdio` is the reserved name
  meaning "ask me, here".
- **The request names the `tool_use` block it is for**, so the approval row and
  the tool row are visibly the same piece of work.
- **It carries `permission_suggestions`**, which are permission updates the CLI
  is offering to apply. Handing them straight back as `updatedPermissions` is the
  whole of how "always allow this session" works, and `10` is the proof: two
  `Write` calls, one request.
- **A denial returns control cleanly.** `08`'s tool result is the deny message
  with `"is_error": true`, the agent reads it and answers, and the turn ends
  `"is_error": false`. A rejection is not a failed turn.
- **An unanswered request costs a tool call and nothing else.** In `09` the
  permission stream closes with stdin and the tool comes back as
  `Tool permission request failed: AbortError`; the CLI retries twice, gives up,
  and finishes the turn normally. Nothing hangs and nothing is orphaned.

One thing they do *not* record, because no capture can: what this server sent.
The answer travels on stdin, and the assertions about it live in
`tests/socket_permissions.rs`, which reads the lines the agent was written.

`tools/permission-capture/record.mjs` is how they were made and how to make them
again; a permission capture cannot be produced by hand, because everything after
the request is a consequence of the answer.

## What `04`–`06` settled that no amount of reading could

Three things about tool use are true of the wire and were not obvious from the
contract, and every design decision in `crate::worklog` and `crate::turn` rests on
one of them:

- **A tool result arrives as a `user` message.** Not under a tool-specific event
  type, and not gated behind `--replay-user-messages` — which this server does not
  pass, and which is about echoing the *developer's* turns. So folding user
  messages is not optional.
- **The CLI emits one buffered `assistant` message per content block**, as the
  block closes, rather than one per API message. A turn that thinks, calls a tool
  and then answers produces four of them, three of which carry no text at all.
  That is why the driver publishes a message only when there *is* text: otherwise
  each would be an empty chat bubble.
- **Calls and results interleave in block order** even when the model was asked
  for parallel ones — `06` is `use A`, `result A`, `use B`, `result B`. The work
  log's collapse of an invocation into its result is adjacency-based, so this is
  what makes several calls in one turn pair up without anything sorting them.

Two things in `03-synthetic-drift.ndjson` deliberately do *not* register as
drift, and the golden file records that by omission:

- `system` with an unrecognized `subtype` — the CLI adds lifecycle subtypes
  routinely and the reducer only cares about `init`.
- `content_block_delta` carrying a `thinking_delta` — `Delta` recognizes only
  `text_delta` on purpose. Real turns emit `thinking_delta`, `signature_delta`
  and `input_json_delta` constantly, so counting them would bury an actual
  format change in noise.

An unrecognized **content block** does register, and since ticket 12 it is the
third `unknown_events` in this capture's golden. It used not to be counted, because
the flattened text carried a visible `[?]` where the block had been; the driver now
skips such a block silently, so the count is the only account of it there is.

## Adding a capture

Drop the `.ndjson` in. The test discovers files by extension, so there is no
test code to change.

```sh
UPDATE_GOLDEN=1 cargo test -p lightcode-server
```

That mints the missing `.expected.json`. Read it before committing — the point
of a golden file is that a human agreed with it once, so that a later machine
disagreeing means something.

## When the CLI moves

Re-run the tests against a new `claude` release. A failure here is the format
having changed, reported without a server in the way. Re-capture, regenerate,
and read the diff: what appears under `unknown_events` is the size of the
change, and what appears in `transcript` is whether it cost anything visible.
