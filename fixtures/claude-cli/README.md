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

## The captures

| File | Provenance | What it covers |
| --- | --- | --- |
| `01-buffered-turn.ndjson` | Recorded, STEP 1 spike (`.scratch/stream-sample.ndjson`) | A turn without `--include-partial-messages`: buffered `assistant` message only, no deltas to reconcile against |
| `02-streamed-turn.ndjson` | Recorded, STEP 1 spike (`.scratch/bidi.ndjson`) | A turn with `--include-partial-messages`: `message_start` → deltas → `message_stop`, reconciled by the buffered message that agrees with them |
| `03-synthetic-drift.ndjson` | Hand-written | Degradation. An unrecognized top-level `type`, an unrecognized `stream_event`, an unrecognized content block, a truncated line, a blank line — none of which a healthy CLI emits, so no recording contains them |
| `04-tool-use.ndjson` | Recorded, ticket 12 | One tool call end to end: reasoning, a `tool_use`, its `tool_result` as a `user` message, more reasoning, then the reply |
| `05-tool-failure.ndjson` | Recorded, ticket 12 | The same shape with `"is_error": true` on the result — a `Read` of a file that is not there |
| `06-several-tool-calls.ndjson` | Recorded, ticket 12 | Two calls in one turn, each answered before the next is announced |

The `.scratch/*.ndjson` originals stay where they are as raw evidence; these are
the committed, test-facing copies. `04`–`06` were recorded straight into
`fixtures/` against `claude-haiku-4-5`, with the same flags
[`crate::agent`](../../crates/lightcode-server/src/agent.rs) passes.

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
