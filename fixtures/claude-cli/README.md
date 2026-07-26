# `claude` CLI captures

Golden inputs for `crates/lightcode-server/tests/protocol_golden.rs`, the drift
detector over the CLI's stdio wire format. Each `NN-<name>.ndjson` is folded
line by line through a fresh `SessionState`; the result is compared against
`NN-<name>.expected.json`.

Sibling of `fixtures/socket-wire/`, which pins the *other* protocol — the one
the UI speaks. This directory pins the one the agent speaks.

## The captures

| File | Provenance | What it covers |
| --- | --- | --- |
| `01-buffered-turn.ndjson` | Recorded, STEP 1 spike (`.scratch/stream-sample.ndjson`) | A turn without `--include-partial-messages`: buffered `assistant` message only, no deltas to reconcile against |
| `02-streamed-turn.ndjson` | Recorded, STEP 1 spike (`.scratch/bidi.ndjson`) | A turn with `--include-partial-messages`: `message_start` → deltas → `message_stop`, reconciled by the buffered message that agrees with them |
| `03-synthetic-drift.ndjson` | Hand-written | Degradation. An unrecognized top-level `type`, an unrecognized `stream_event`, an unrecognized content block, a truncated line, a blank line — none of which a healthy CLI emits, so no recording contains them |

The `.scratch/*.ndjson` originals stay where they are as raw evidence; these are
the committed, test-facing copies.

Two things in `03-synthetic-drift.ndjson` deliberately do *not* register as
drift, and the golden file records that by omission:

- `system` with an unrecognized `subtype` — the CLI adds lifecycle subtypes
  routinely and the reducer only cares about `init`.
- `content_block_delta` carrying a `thinking_delta` — `Delta` recognizes only
  `text_delta` on purpose. Real turns emit `thinking_delta`, `signature_delta`
  and `input_json_delta` constantly, so counting them would bury an actual
  format change in noise.

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
