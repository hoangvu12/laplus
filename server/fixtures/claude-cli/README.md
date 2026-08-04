# `claude` CLI captures

Golden inputs for `crates/lightcode-server/tests/protocol_golden.rs`, the drift
detector over the CLI's stdio wire format. Each `NN-<name>.ndjson` is folded
line by line through a fresh `SessionState`; the result is compared against
`NN-<name>.expected.json`.

Sibling of `fixtures/socket-wire/`, which pins the _other_ protocol — the one
the UI speaks. This directory pins the one the agent speaks.

Since ticket 10 they are also **replayed**: `harness::agent::ScriptedAgent`
writes a stand-in `claude` that reads a turn on stdin and prints one of these
files back, so `tests/socket_turn.rs` drives a whole turn through the socket
against what the CLI actually said. That gives a capture two jobs — the reducer
is held to it, and the server is driven by it — and it is why re-capturing after
a `claude` release is worth doing even when the golden files still match.

A capture containing a `control_request` is replayed with a stop where the
request is: the CLI waits there for an answer, and everything after that line in
the recording happened _because of_ the answer. Playing it straight through would
have the stand-in react to a decision nobody had made.

A capture containing a `control_response` is replayed with a stop _before_ it,
for the mirror-image reason: that line is the CLI answering something the server
asked it, and the request travelled on stdin so it is not in the recording.
Playing it straight through would have the stand-in answer a request nobody had
sent, and then abort a turn nobody had stopped.

## The captures

| File                                       | Provenance             | What it covers                                                                                                                                                                                                  |
| ------------------------------------------ | ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `01-buffered-turn.ndjson`                  | Recorded, STEP 1 spike | A turn without `--include-partial-messages`: buffered `assistant` message only, no deltas to reconcile against                                                                                                  |
| `02-streamed-turn.ndjson`                  | Recorded, STEP 1 spike | A turn with `--include-partial-messages`: `message_start` → deltas → `message_stop`, reconciled by the buffered message that agrees with them                                                                   |
| `03-synthetic-drift.ndjson`                | Hand-written           | Degradation. An unrecognized top-level `type`, an unrecognized `stream_event`, an unrecognized content block, a truncated line, a blank line — none of which a healthy CLI emits, so no recording contains them |
| `04-tool-use.ndjson`                       | Recorded, ticket 12    | One tool call end to end: reasoning, a `tool_use`, its `tool_result` as a `user` message, more reasoning, then the reply                                                                                        |
| `05-tool-failure.ndjson`                   | Recorded, ticket 12    | The same shape with `"is_error": true` on the result — a `Read` of a file that is not there                                                                                                                     |
| `06-several-tool-calls.ndjson`             | Recorded, ticket 12    | Two calls in one turn, each answered before the next is announced                                                                                                                                               |
| `07-permission-approved.ndjson`            | Recorded, ticket 13    | A `control_request` asking to use `Write`, approved — the tool then succeeds                                                                                                                                    |
| `08-permission-declined.ndjson`            | Recorded, ticket 13    | The same request declined: the tool result carries the refusal, and the agent answers anyway                                                                                                                    |
| `09-permission-unanswered.ndjson`          | Recorded, ticket 13    | The same request left hanging until stdin closed — the CLI abandons it and finishes the turn                                                                                                                    |
| `10-permission-for-the-session.ndjson`     | Recorded, ticket 13    | Approved with the CLI's own permission suggestion handed back: two `Write` calls, one request                                                                                                                   |
| `11-interrupted-turn.ndjson`               | Recorded, ticket 14    | A reply stopped mid-sentence: the acknowledgement, the partial text handed over whole, and an aborted `result`                                                                                                  |
| `12-interrupt-then-continue.ndjson`        | Recorded, ticket 14    | The same, and then a second turn answered normally by the same process                                                                                                                                          |
| `13-interrupt-during-tool-use.ndjson`      | Recorded, ticket 14    | A stop in the middle of a run of `Write` calls, as the agent was opening the next one                                                                                                                           |
| `14-interrupt-with-nothing-running.ndjson` | Recorded, ticket 14    | A stop sent after the turn had already ended — acknowledged, and nothing else happens                                                                                                                           |
| `15-permission-cancelled.ndjson`           | Recorded, ticket 14    | "Cancel" on a permission: a denial carrying `interrupt: true`, which ends the turn the way an interrupt does                                                                                                    |
| `16-error-result.ndjson`                   | Hand-written           | A turn the agent reports as failed, with its reason in the `result`'s `errors` array — which is the only thing in a failed turn a developer can act on                                                          |
| `17-rate-limited.ndjson`                   | Hand-written           | Three `rate_limit_event`s — fine, close to the limit, refused — and the failed turn the third produces                                                                                                          |
| `18-compacted.ndjson`                      | Hand-written           | A `system`/`compact_boundary` between two turns: the agent's memory rewritten, and the transcript deliberately unchanged by it                                                                                  |
| `19-context-usage.ndjson`                  | Recorded, ticket 76    | The CLI answering how full its own window is, twice — as the session announces itself and as the turn ends — around a turn that uses a tool                                                                     |
| `20-modes-changed-mid-conversation.ndjson` | Recorded, ticket 11    | A runtime mode and a model pushed to a _running_ child between two turns: the first turn writes a file unasked, the second is stopped for permission, and the model on it is the one that was pushed            |
| `21-modes-refused.ndjson`                  | Recorded, ticket 11    | The same two requests refused — an unnameable mode and an unrecognised model — which is the first `control_response` with `"subtype": "error"` in this directory                                                |
| `22-background-subagent.ndjson`            | Recorded               | A background subagent: its `task_*` events, its own messages tagged with the `Agent` call that owns them, and the two `result` lines one invocation ends with                                                   |
| `23-forwarded-subagent-text.ndjson`        | Recorded               | The same wire under `--forward-subagent-text`: a subagent's prompt and its answer arrive as ordinary `user`/`assistant` envelopes carrying `parent_tool_use_id`, which is the only thing that tells them apart  |

The raw STEP 1 originals these two were curated from lived in `.scratch/` and
were deleted on 2026-07-29; the committed, test-facing copies here are now the
only ones. `04`–`15` were recorded straight into
`fixtures/` against `claude-haiku-4-5`, with the same flags
[`crate::agent`](../../crates/lightcode-server/src/agent.rs) passes, and `19`
was recorded the same way.

## What `22` settled

A recording of one background subagent, against `claude` 2.1.220 with this
server's own flags: the model called `Agent` with `run_in_background: true`, the
subagent worked, and the CLI told the main agent when it finished. It settled
three things, and the first two were costing the developer something on every
background subagent they ran.

**A subagent's messages arrive on this wire, tagged with the `Agent` call that
owns them.** Not behind `--forward-subagent-text` — that flag governs the
_foreground_ case, and a background subagent forwards regardless. Nothing read
`parent_tool_use_id`, so all of it folded into the transcript as the main agent
talking: **eleven of this capture's sixteen transcript entries were the
subagent's**, including its final report, and the golden now has five. That is
the whole of the difference, and it is why the field is read rather than the flag
left off.

**The `task_*` system events are how a subagent can be seen at all.**
`task_started`, `task_progress`, `task_updated` and `task_notification` reached
`SystemEvent::Other` and were dropped in silence, which is why a running subagent
showed as nothing whatsoever. They carry more than enough for a row: a stable
`task_id`, the spawning `tool_use_id`, a `description` that says what the
subagent is doing _now_, and on the notification the `summary` that is the
subagent's own final answer.

**One invocation emits two `result` lines.** The capture ends `result, result`,
and the second arrived after the turn had been taken — reporting a second
completion for a turn already over and settling a turn id of `None`. It is
counted and dropped now.

What this capture does **not** show is a turn ending while a subagent was still
running: the CLI held the process open until the background work was done, then
emitted both results. So the composer going idle mid-subagent is not something
this recording reproduces, and nothing here should be read as evidence for it.

The `sleep` commands inside the subagent were refused — the recording was made
without `--permission-prompt-tool`, so the child had no way to ask. That is an
artefact of how it was captured and not of the feature; it is left in because the
subagent's own account of being refused is exactly the kind of text that used to
end up attributed to the main agent.

## What `20` and `21` settled

Ticket 11 was written on the premise that **the agent protocol has no control
request that moves a running child's permission mode**, and asked for a human
decision between replacing the session and giving up on the claim. The premise
was false, and `20` is the disproof: two requests this server now sends, probed
against `claude` 2.1.220 with this server's own launch flags.

- **`set_permission_mode` moves a live child, both ways.** `20` opens under
  `bypassPermissions`, writes a file with nothing asked of the developer, is
  pushed to `default` between turns, and is then _stopped for permission_ on the
  second turn's identical `Write`. That prompt is the whole evidence: it exists
  only because the push landed.
- **`set_model` moves a live child too**, and takes the bare slug — `opus`, which
  the CLI resolves to `claude-opus-5` itself. The second turn's `message_start`
  carries `claude-opus-5` where the first carried `claude-haiku-4-5-20251001`.
- **The session is not replaced.** One `session_id` throughout, and the
  conversation continues rather than restarting — which is what makes a push
  worth having over a kill-and-`--resume`: no fresh `init`, no lost context
  window.
- **The CLI confirms twice.** A `control_response` naming the request and the
  mode it moved to, and a `system`/`status` line carrying `permissionMode`. The
  first is what the server keys the outcome off; the second is unrecognised and
  folds to nothing, which the golden records by its absence.
- **A `set_model` push makes the CLI narrate itself a `user` line** reading
  `<local-command-stdout>Set model to opus (claude-opus-5)</local-command-stdout>`
  and marked `isReplay`, with `content` as a bare string rather than a list of
  blocks. This server folded _every_ user line into the transcript on the stated
  grounds that it does not pass `--replay-user-messages`; that reasoning no longer
  holds, and the line is now read and dropped. Both halves of the trap are in the
  golden: no eighth transcript entry, and `parse_errors: 0`.
- **`approval-required` maps cleanly to `default` as a pushed mode**, so
  tightening and loosening are the same operation in opposite directions — the
  asymmetry the ticket worried about was a property of the _launch_ table, which
  is deliberately left lossy.

`21` is the refusal half, and it is why the two are separate captures: a healthy
conversation does not contain one. It records the two sentences the CLI answers
with — `Cannot set permission mode: must be one of acceptEdits, auto,
bypassPermissions, default, dontAsk, plan`, and `Model "not-a-model" is not a
recognized model id` — which are what the developer is shown, verbatim, when a
push does not land. It also pins the list of modes the CLI will take, which is
what `agent::pushed_permission_mode_for` is checked against.

Both were recorded by a script driving `claude` directly with this server's flags,
the same way `19` was. `21` needs no turn at all: the control channel answers a
process that has never been sent one.

## What `19` settled

The first capture in this directory of an exchange the **server** starts — not a
question the CLI asked, and not an acknowledgement of a stop, but an answer to
`{"subtype": "get_context_usage"}` sent on stdin. Ticket 76's, and what it
records could not have been read off anything in this repository:

- **The request exists in the shipped CLI.** It is an SDK control request, so
  the version this project drives might have answered with an error naming a
  callback it never registered — which is the shape
  `an_agent_that_will_not_say_leaves_the_inferred_meter_alone` is written
  against. `claude` 2.1.220 implements it.
- **It is answered while a turn is running.** The first of the two questions
  here goes out on the session's `init`, before the opening turn has produced a
  single delta, and comes back straight away rather than queueing behind the
  turn. That is the whole of why the meter can show a percentage on the first
  turn of a session: `modelUsage` carries the window only on the `result` that
  ends one.
- **The reply is the whole reading in one answer** — `totalTokens`, `maxTokens`
  and `isAutoCompactEnabled` — among seventeen fields of the CLI's own
  accounting: a category breakdown, the grid `/context` draws with, per-skill and
  per-agent counts. Three are read. The rest are left in the capture, because a
  golden that trimmed them could not see them move.
- **`isAutoCompactEnabled` is the reason the request is sent at all.**
  Auto-compact is mentioned nowhere in the eighteen captures before this one, and
  the client renders a sentence from it.
- **The CLI's count and this server's inference disagree**, by 22 tokens on this
  recording: the `result` adds up to 26,959 and the answer that follows it says
  26,937 about the same conversation. Both are in the golden — one on
  `last_result`, one on `token_usage` — which is what pins the precedence.

Asking costs no API call. The recorder can be run against a fresh process with
no turn at all and still get a reading, which is what the probe that settled the
first point above did.

`tools/context-capture/record.mjs` made it, and asks at the two moments
[`crate::turn`] asks at — the timing is what decides where the answers land in
the recording, and therefore what the replay in `tests/socket_turn.rs` sees.

## What `16`–`18` are, and why they are not recordings

Ticket 15's three cases have one thing in common: a healthy CLI on a healthy
account does not produce them on demand. An API failure cannot be asked for, a
usage limit cannot be reached to order, and a compaction takes a conversation
long enough to fill the context window — so these are hand-written, like `03`,
and the _shapes_ are read off the `claude` binary rather than guessed:

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
stopped turn _ends_:

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

One thing they do _not_ record, because no capture can: what this server sent.
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
  pass, and which is about echoing the _developer's_ turns. So folding user
  messages is not optional.
- **The CLI emits one buffered `assistant` message per content block**, as the
  block closes, rather than one per API message. A turn that thinks, calls a tool
  and then answers produces four of them, three of which carry no text at all.
  That is why the driver publishes a message only when there _is_ text: otherwise
  each would be an empty chat bubble.
- **Calls and results interleave in block order** even when the model was asked
  for parallel ones — `06` is `use A`, `result A`, `use B`, `result B`. The work
  log's collapse of an invocation into its result is adjacency-based, so this is
  what makes several calls in one turn pair up without anything sorting them.

Two things in `03-synthetic-drift.ndjson` deliberately do _not_ register as
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
