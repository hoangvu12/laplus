# spike-claude-protocol — THROWAWAY PROTOTYPE

**Status: the question is answered. PASS.**

## Run it

```sh
cargo run
```

Type a prompt and press Enter. `/raw` toggles the raw event log, `/q` quits.
Requires `claude` on PATH.

## The question

From `HANDOFF-rust-server-tauri.md`, STEP 1:

> Does the `claude` CLI's stdio protocol bend to Rust cleanly enough to stream
> agent output into t3code's unmodified `apps/web` UI — or does it fight us
> hard enough to fall back to Option 1 (prune the Electron build)?

The handoff budgeted ~1 week for this and named it the single unknown gating a
2–4 month commitment. It is the load-bearing assumption: t3code does not bundle
Claude Code, it resolves the user's installed binary and hands it to the Agent
SDK as `pathToClaudeCodeExecutable`. The Rust code takes the SDK's place, so
everything depends on that subprocess protocol being tractable.

## The answer

Yes, and more easily than the handoff assumed. Three findings:

**1. The wire format is newline-delimited JSON with a `type` discriminator.**
Six envelope variants observed: `system` (`init`, `status`, `hook_started`,
`hook_response`), `assistant`, `user`, `stream_event`, `rate_limit_event`,
`result`. That is a small surface — `protocol.rs` covers it in one file.

**2. The payloads inside the envelope are standard Anthropic Messages API
types, not a bespoke schema.** `assistant.message` is a `Message` object;
`stream_event.event` values are verbatim Messages API SSE events
(`message_start` → `content_block_start` → `content_block_delta`/`text_delta`
→ `content_block_stop` → `message_delta` → `message_stop`).

This materially lowers Risk #1 in the handoff. The risk was stated as "the
stdio wire format is not a stability-guaranteed public contract." True of the
*envelope* — but the envelope is thin, and the part carrying the real
complexity is the public, versioned Messages API schema. The unstable surface
is much smaller than assumed.

**3. Bidirectional streaming works.** The CLI reads NDJSON user turns on stdin
under `--input-format stream-json`, so a long-lived session can be driven turn
by turn. That is what a server needs; it is not just one-shot invocation.

### The flags that constitute the protocol

```
claude -p \
  --input-format stream-json \
  --output-format stream-json \
  --include-partial-messages \
  --verbose
```

`--verbose` is required alongside `-p/--print` for stream-json output.
`--include-partial-messages` is what yields token-level deltas — without it you
get only buffered whole messages, which would make the UI feel dead during a
turn. `--session-id <uuid>` and `--resume` exist for session continuity and are
the next thing to exercise.

### Verified end-to-end

Rust spawned `claude`, sent a user turn, and rendered 16 live frames:

```
session    a396dfc1-2f72-48d6-b04c-9ec6585d6808
model      claude-opus-5[1m]
perm mode  default   tools 32
transcript (1 turns)
  assistant: spike ok (deltas matched buffered message)
last result stop=end_turn turns=1 1867ms  $0.0139  error=false
protocol drift unknown events: 0   parse errors: 0
```

Raw captures used as primary source are in `../.scratch/`.

## The design decision it settled

Assistant text arrives **twice**: incrementally as `content_block_delta`s, and
again as a complete buffered `assistant` message. `SessionState::reduce`
renders deltas for responsiveness and lets the buffered message replace the
accumulated text when it lands — deltas are best-effort and may be shed, so the
buffered message is authoritative. The prototype confirms the two agree on a
clean turn (`deltas matched buffered message`).

Accumulate-and-reconcile is the shape the real server should use. Rendering
deltas alone risks silently truncated output; waiting for the buffered message
alone makes streaming pointless.

## What to keep, what to throw away

- **Keep `src/protocol.rs`.** Pure, no I/O, lifts into the real server as-is.
  Its `Unknown` variants are load-bearing: an unrecognized event type degrades
  to a counter rather than killing the session, and the reducer surfaces those
  counts so protocol drift shows up as a number instead of a crash. That is the
  concrete form of the handoff's "isolate it behind one Rust module."
- **Throw away `src/main.rs`.** Terminal shell, optimized for driving by hand.

## What this did NOT prove

The handoff's pass criterion was output streaming in **t3code's real UI**. This
proved the protocol half — the harder and riskier half — but not the HTTP/
contract half. Still unproven:

- `apps/web` rendering against a Rust server (needs `axum` + the
  `packages/contracts` shapes)
- Multi-turn session continuity via `--session-id` / `--resume`
- Tool-use round-trips and permission prompts (only a plain text turn was run)
- Long-session behavior: compaction, context limits, interrupts

None of these are protocol-format risks, which is what STEP 1 existed to
retire. They are ordinary implementation work.
