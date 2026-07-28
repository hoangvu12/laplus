# CLI stream audit — what `claude` says and what laplus hears

**Date:** 2026-07-28 · **CLI:** `claude` 2.1.220 · **Model:** `claude-haiku-4-5`

Three captures, recorded against the **real binary** with the flags
`crate::agent` passes verbatim (`--print --input-format stream-json
--output-format stream-json --include-partial-messages --verbose
--permission-prompt-tool stdio`). They exist because the committed fixtures in
`server/fixtures/claude-cli/` cover eighteen cases and **none of them contains a
hook, a thinking delta, a `thinking_tokens` line, a `status` line, or a `usage`
object** — so every one of those went unexamined, and the reducer was held to a
set of recordings that happened not to include the majority of what the CLI
emits.

These are raw evidence, not test inputs. They are not golden files and nothing
asserts against them. Provenance and paths inside them happened; do not edit.

| File                                | What it is                                                                                                                                                     |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `01-trivial-turn.ndjson`            | One turn, prompt "Reply with exactly: ok". 24 lines. The floor: what the CLI emits when asked to do essentially nothing.                                       |
| `02-tool-turn.ndjson`               | One turn that reasons and calls a tool. 78 lines.                                                                                                              |
| `03-project-scoped-commands.ndjson` | A turn run in a scratch directory containing `.claude/commands/zzz-probe-marker.md`, to settle whether `system/init` carries project-scoped commands. It does. |

## The measurement

Folding both turns line by line through `crate::protocol`'s rules:

```
total lines:                            102
lines that produce nothing at all:       66  (65%)

  25  system/thinking_tokens          → SystemEvent::Other → Nothing
  23  delta/thinking_delta            → Delta::Unknown     → Nothing
   4  delta/input_json_delta          → Delta::Unknown     → Nothing
   3  system/status                   → SystemEvent::Other → Nothing
   3  stream_event/message_delta      → carries live usage; unread
   2  system/hook_started             → SystemEvent::Other → Nothing
   2  system/hook_response            → SystemEvent::Other → Nothing
   2  delta/signature_delta           → Delta::Unknown     → Nothing
   2  rate_limit_event                → below the reporting threshold (deliberate)
```

**Be precise about what that 65% means.** Roughly half of it is redundant rather
than lost: `thinking_delta` and `input_json_delta` carry content that arrives
again, whole, in the buffered `assistant` message, and laplus renders it there.
What those lines cost is _liveness_, not information.

The other half has **no second source**:

```
  25  system/thinking_tokens      a live thinking-token count
   3  system/status               "requesting", and whatever else it says
   4  hook_started / hook_response hook name, event, exit code, stdout, stderr
   3  message_delta.usage         live token usage mid-turn
```

That is **34 of 102 lines — a third of the stream — reaching the developer
nowhere**, and none of it increments a drift counter, because
`SystemEvent::Other` and `Delta::Unknown` are silence by construction rather
than by accident (`protocol.rs:785`, `:804`).

## Finding 1 — the context window is in a line laplus already parses

The `result` line carries this, verbatim:

```json
"modelUsage": {
  "claude-haiku-4-5": {
    "inputTokens": 10, "outputTokens": 40,
    "cacheReadInputTokens": 0, "cacheCreationInputTokens": 27065,
    "costUSD": 0.05434,
    "contextWindow": 200000, "maxOutputTokens": 32000,
    "canonicalModel": "claude-haiku-4-5", "provider": "firstParty"
  }
},
"usage": { "input_tokens": 10, "cache_creation_input_tokens": 27065,
           "cache_read_input_tokens": 0, "output_tokens": 40, ... }
```

`ResultEvent` declares `subtype`, `is_error`, `stop_reason`, `num_turns`,
`duration_ms`, `total_cost_usd`, `errors` and `result` — and serde drops the
rest silently. So `usage` and `modelUsage` arrive on every turn and are
discarded on the floor of a struct definition.

This is exactly the field upstream reads:
`maxClaudeContextWindowFromModelUsage` (`ClaudeAdapter.ts:325`) takes
`modelUsage` and returns `max(contextWindow)`;
`makeClaudeTokenUsageSnapshot` (`:408`) turns it and the token counts into the
snapshot that becomes a `context-window.updated` activity, which is what
`apps/web/src/lib/contextWindow.ts:56` reads and `ContextWindowMeter.tsx` draws.

**So the composer's context meter is empty not because the data is unavailable
but because six fields are undeclared.** `message_delta.usage` carries the same
shape mid-turn, including `output_tokens_details.thinking_tokens`, which is what
makes the meter move _during_ a turn rather than at the end of one.

The `result` line also carries, all unread: `permission_denials`,
`terminal_reason` (`"completed"` — a direct statement of how the turn ended,
where `crate::turn::Ending` infers it), `api_error_status`, `fast_mode_state`,
`ttft_ms`, `duration_api_ms`, and `usage.iterations`.

## Finding 2 — thinking is most of what a turn streams, and none of it is live

Across the two turns: **23 thinking deltas against 7 text deltas.** The
`--include-partial-messages` flag was kept, against upstream, on the explicit
ground that streaming "is the only thing filling the first two seconds of a
turn" (`HANDOFF-2026-07-28`). On a reasoning model the first seconds of a turn
are thinking, and thinking is precisely what `Delta::Unknown` drops.

The thinking still renders — it arrives as a whole `thinking` block in the
buffered message and becomes a `task.progress` row — so nothing is lost. But the
window the flag exists to fill is the window it does not fill, and the
25 `thinking_tokens` lines that would let the UI show _something_ moving during
it are dropped too.

## Finding 3 — hooks fire on an ordinary turn, and are invisible

`SessionStart:startup` fired on a bare "reply ok", with a full payload —
`hook_name`, `hook_event`, `exit_code`, `outcome`, `stdout`, `stderr`. A hook
that fails prints its reason into a line laplus folds to `Nothing`.

`protocol.rs:33` names `hook_started` and `hook_response` as system subtypes in
its own doc comment, and the enum has arms for `init` and `compact_boundary`
only. This is the same shape as the bug the compact-boundary test records at
`protocol.rs:1512` — _"before this was recognised it fell to
`SystemEvent::Other`, which is silence"_ — and it is still open, one subtype
over.

## Finding 4 — ticket 38's premise does not hold

`03-project-scoped-commands.ndjson` was run in a directory whose only
distinguishing feature is `.claude/commands/zzz-probe-marker.md`. Its `init`
line lists **79 slash commands, including `zzz-probe-marker`.**

Ticket 38 and `catalogue.rs:36` decline project-scoped commands on cost:

> a `claude` reads `<cwd>/.claude/commands` for the project it was started in,
> and this server has no one project — it has a registry of them, and a probe per
> project would be a `claude` per project on every refresh.

True of a _probe_. But the commands arrive free on the `init` of any real turn in
that project, and **`InitEvent` already declares `slash_commands`**
(`protocol.rs:364`) — the field is parsed and then read by nothing. The
catalogue's separate objection at `:112` (init is not written until the CLI has
been given a prompt) is what rules init out for the _handshake_, and does not
apply to a turn.

The real trade is the one at `catalogue.rs:120`: the handshake returns objects
with descriptions and argument hints, `init` returns bare names. So the answer is
neither source alone — it is the handshake's rich built-ins unioned with each
project's names, learned from the first turn run there. That is a smaller change
than the ticket assumes and it needs no extra process.

## How to redo this

```sh
cd $(mktemp -d)
echo '{"type":"user","message":{"role":"user","content":"Reply with exactly: ok"}}' \
  | claude --print --input-format stream-json --output-format stream-json \
           --include-partial-messages --verbose --permission-prompt-tool stdio \
           --model claude-haiku-4-5 > turn.ndjson
```

Worth rerunning on a `claude` upgrade. The reason these findings existed for as
long as they did is that the fixtures were captured once, against ticket-shaped
scenarios, and the subtypes nobody had a ticket for were never in them.
