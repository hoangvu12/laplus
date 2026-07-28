# 12 — Tool-use round-trips

**What to build:** The developer can follow what the agent is actually doing. When
the agent invokes a tool, the transcript shows which tool and what it was given;
when the tool returns, the result appears. The developer can tell at a glance
whether a step succeeded or failed.

Thinking is distinguished from writing in the UI, so a pause while the agent
reasons does not read as a hang.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** done

- [x] A tool invocation appears in the transcript naming the tool and its input —
      a `tool.updated` activity carrying the tool's name and its arguments, and
      published as `status: "inProgress"` so the row exists while the tool runs
- [x] The corresponding result appears and is visually associated with its
      invocation — the pair shares the id the agent minted, which is the key the
      work log collapses two rows into one by
- [~] A failed tool call is distinguishable from a successful one — the tool's own
  `is_error`, as `status: "failed"`, rather than left to the client's guess
  from the shape of the output text. A real failure is now never missed; the
  converse is not fully true, and "What review caught" says why
- [x] Several tool calls within one turn are rendered in order and correctly
      paired — the CLI interleaves each call with its own result, _and_ every row
      carries the `sequence` the client orders the work log by, without which it
      re-derives an order from a millisecond clock
- [x] Thinking is shown as distinct from assistant output — its own row, in the one
      kind this UI renders with its thinking affordance, and _out_ of the reply,
      which is where the old placeholder put it
- [x] Large tool inputs and outputs are truncated for display without losing the
      underlying record — 180 characters on the row, the whole input and the whole
      output in the payload beside it
- [x] A turn mixing text and tool use renders both in the order they occurred
- [x] Scripted fake-agent captures cover single tool use, multiple tool use, and
      tool failure — and all three are **recordings** rather than hand-written
- [x] Tests assert the event sequence for each case through the socket boundary

## Comments

### The three captures answered questions no amount of reading the contract could

`fixtures/claude-cli/04-tool-use.ndjson`, `05-tool-failure.ndjson` and
`06-several-tool-calls.ndjson` are real `claude` output, recorded for this ticket
with the flags `crate::agent` passes. Every design decision below rests on one of
three things they settled, and each was a live uncertainty beforehand:

- **A tool result arrives as a `user` message.** Not under a tool-specific event
  type, and not gated behind `--replay-user-messages` — which this server does not
  pass, and which is about echoing the _developer's_ turns back. So the `user`
  branch of the fold, which ticket 10 deliberately dropped on the floor, is now the
  only place a tool call can be seen to have returned.
- **The CLI emits one buffered `assistant` message per content block**, as the
  block closes, rather than one per API message. A turn that reasons, calls a tool
  and then answers produces four of them, and three carry no text at all.
- **Calls and results interleave in block order** even when the model was asked for
  parallel ones: `06` is `use A`, `result A`, `use B`, `result B`. The work log's
  collapse of an invocation into its result is _adjacency_-based
  (`collapseDerivedWorkLogEntries` only ever merges with the row before it), so
  this is the reason several calls in one turn pair up at all. Had the CLI batched
  both invocations and then both results, the criterion would have needed a UI
  change to satisfy — and the UI is upstream's. It is _not_, on its own, enough:
  the client re-sorts before it collapses, which is what the `sequence` below is
  for.

The second finding is the load-bearing one, because it produced a defect that had
nothing to do with tool use: a message with no text was published anyway, so every
thinking block and every tool call arrived as an **empty assistant chat bubble**.
The fold now publishes a message only when there is text to publish — with one
exception, below.

### The reply used to contain the model's reasoning, verbatim, as prose

`flatten` rendered the blocks it could not describe into the message text:
`[thinking]`, `[tool_use: Read]`, `[?]`. That string was the buffered message, and
the buffered message is authoritative — so it replaced what the deltas had built
and went into the developer's chat bubble. A turn that used a tool showed the reply
as `[tool_use: Read]`.

So `Turn` now carries `text` _and_ `content`: the text blocks joined, and every
block in order. `text` is what the transcript holds and what the delta
accumulation is reconciled against; `content` is what the driver reads to find the
work. The golden diff for `03-synthetic-drift` is the whole change in one line —
`"partial answer, [tool_use: Read][?]"` became `"partial answer, "`, with the two
blocks beside it saying what they are.

### Tool use is read off the buffered messages, not off the stream

The stream announces a tool call twice: once as a `content_block_start` with the
arguments still arriving as partial JSON, and again as the buffered `assistant`
message once the block has closed. This reads the second, which is the same
reconciliation rule the text follows, applied where it matters most — deltas are
best-effort and may be shed, and a shed tool call would be a step the developer
never saw the agent take. The buffered message always arrives, and it arrives with
the input whole rather than as fragments this server would have to reassemble.

What it costs is announcing the call when the block closes rather than when it
opens: a moment before the tool runs rather than a moment after the model decided
to run it. What it buys is that a pair is never half-published.

Thinking follows the same rule for the same reason, and pays a larger price for it:
the row appears when the reasoning block _closes_, so it is a record of a pause
rather than an indicator during one. The ticket's concern — that a pause not read
as a hang — is answered by the session being `running` throughout, which is what
the UI's own active-work indicator is driven by. The alternative was
`content_block_start`, which would put the row up immediately and carry no
reasoning, since an activity cannot be amended later; that trades the content for
timeliness the session state already supplies.

### Three deliberate divergences from upstream's payloads, each fixing what it shows

The vocabulary is upstream's throughout — `classifyToolItemType`, `titleForTool`,
`summarizeToolRequest` and the 180-character `truncateDetail`, ported into
`crate::worklog` — because a conversation held through this server has to look like
one held through that server, and the UI in between is the same code. Three places
differ, and each is a defect in what upstream renders rather than a difference of
taste:

- **An invocation is announced as `tool.updated`, not `tool.started`.**
  `deriveWorkLogEntries` **drops** `tool.started` on its first line, so upstream
  shows nothing at all while a tool runs: a developer watching a minute-long `Bash`
  sees the turn spinning and no reason for it. `tool.updated` with
  `status: "inProgress"` is the same information in the kind the log renders, and
  it collapses into the completed row rather than adding a second one.
- **`data.toolCallId` is set.** The collapse key prefers `tool:<toolCallId>` and
  falls back to the title and the detail. Upstream's Claude adapter sets no id, so
  two `Bash` calls running the same command in one turn collapse into a single row.
  The id the agent already minted is the honest key, and it is the one thing that
  makes "correctly paired" a property rather than a coincidence.
- **`status` is on the `tool.completed` payload.** Upstream's mapping omits it,
  which leaves the client defaulting the row to `completed` and recognising failure
  only through `toolDetailTextLooksLikeFailure` — a list of regular expressions over
  prose, `enoent` and `is not recognized as the name of a cmdlet` included. The tool
  said whether it failed; saying so is better than guessing. It fixes one direction
  only, and review was right to catch the overclaim — see below.

A failed call keeps `tone: "tool"` rather than becoming `error`, and that is not
timidity: `showDestructiveRowStyle` styles an error row as the _server's_ failure.
The tool failed; this server did not.

### What the row shows and what is kept behind it are two different jobs

`detail` is one line, truncated. `data` is the record: the whole input, the whole
output, untruncated, so the transcript holds what actually happened even where the
row cannot show it. A tool that read a large file leaves 180 characters in the work
log and the file's contents in the payload.

The invocation's detail names the tool and its input; the result's detail is the
_output_, because that is the new information and the row it collapses into already
showed the input. A result with no output leaves the detail off entirely rather than
blanking the row it merges into — `mergeDerivedWorkLogEntries` takes
`next.detail ?? previous.detail`, so an absent field is how "unchanged" is spelled.

`data.command` is set when the tool has one, because `extractToolCommand` looks
there before it falls back to parsing the detail: with it, a command execution's row
reads `git status`; without it, `Bash: git status` appears twice, once as the
command and once as the detail. It is trimmed, because it is for the row — the
record is `data.input`, verbatim.

### What review caught

`/code-review` ran both axes and found five things. Three were substantive, and the
first is the one that mattered: an argument in this file was simply wrong.

- **"The order needs no sorting" was false, because the client sorts.**
  `compareActivitiesByOrder` orders the work log by `sequence` when the activities
  carry one and falls back to `createdAt` — a _millisecond_ — and then to a rank that
  puts every `.updated` before every `.completed`. Activities carried no sequence, so
  two pairs landing inside one millisecond would have been re-ordered to
  `updated A, updated B, completed A, completed B`: adjacency lost, neither pair
  collapsed, and both invocation rows left reading as still running. The recordings
  do not produce it — `06`'s gaps are 10–300ms — but the capture also shows two
  buffered messages 2ms apart, and this server folds a line in far less than that.
  `Activity` now carries the `sequence` it was announced under, published, stored
  (schema v3) and restored, and `rows()` in `socket_tools.rs` **sorts by it before
  asserting**, because a test asserting publish order would have passed against this
  defect. It is the same argument `thread_messages.ordinal` already makes here: a
  millisecond timestamp is not a total order.
- **"Counted as drift" was also false.** The ticket and `protocol.rs` both claimed an
  unreadable content block showed up in `unknown_events`; nothing incremented it.
  Before this ticket such a block was at least _visible_, as a `[?]` in the flattened
  text — so the change had quietly removed the only account of it there was.
  `note_unreadable_blocks` now counts them, `turn.completed` carries the total, and
  `03-synthetic-drift`'s golden went from two to three.
- **`status: "completed"` does not stop the client guessing.**
  `workEntryIndicatesToolFailure` short-circuits on `"failed"` and _not_ on
  `"completed"`: a successful call falls through to the prose heuristic. Since this
  server puts the tool's **output** in `detail`, a `Grep` that matched nothing and
  returned `No files found` renders with a failure affordance. There is no way to
  avoid it and still show the result — every field the row can display is a field
  that heuristic reads — so the criterion is marked `[~]` and the asymmetry is now
  stated where the decision lives rather than claimed away. A real failure is never
  missed, which is the direction that matters; a wrong tick on a success is the
  better of the two failures.
- **Thinking rows inherited the same heuristic**, and that one _was_ avoidable. A
  thinking row is tool-like as far as `workLogEntryIsToolLike` is concerned, so
  reasoning quoting an error the agent was working through would have flagged a step
  that never ran. The reasoning moved out of `payload.detail` and into
  `payload.thinking`: the row is a bare `Thinking`, the reasoning is on the record,
  and nothing false is rendered. Less is shown and less is wrong.
- **The capture-coverage counters could not fail.** They were only inserted where the
  thing occurred, so deleting the last capture containing a failed tool call would
  have made the key _vanish_ from the totals — and a key that is absent cannot be
  reported as uncovered, which is exactly what that test's own comment says it
  prevents. Every counter is seeded now.

Two smaller things went the same way: `Kind` became an enum so the item type and the
title are produced from one value rather than by re-matching the string the first
match returned; and `rows()` in `socket_tools.rs` became a struct rather than four
same-typed strings read by index, which is the shape `agent::Launch`'s own
documentation rejects. `Write::Activity` is boxed, because a bigger `Activity` pushed
`Write` past the variant-size lint the crate is otherwise clean of — the same reason
`Write::Thread` was already boxed.

### Thinking uses `task.progress`, and the kind is a compromise worth naming

`tone: "thinking"` is the UI's affordance for reasoning — a robot icon, and
pointedly no success tick, which is right for something that is neither a message
nor a step that can succeed. It is reachable from exactly one activity kind:
`task.progress` (`session-logic.ts`, `toDerivedWorkLogEntry`). Upstream means that
kind for subagent-task progress, and upstream's own Claude adapter routes reasoning
to a `content.delta` with `streamKind: "reasoning_text"` — which this web client
never renders, so **thinking is invisible in upstream**.

A lightcode-specific kind would have been more honest about what it is and would
render as a generic info row with a tick, which is precisely not distinguishing
thinking from doing. The kind is the client's lever; using it is choosing the
criterion over the label.

The row carries no `detail`, so it reads as a bare `Thinking` with the reasoning
behind it in `payload.thinking`. That is a decision review forced and it is the right
one: `detail` is the field the client scans for failure-shaped prose, and reasoning
about an error is reasoning that quotes one.

### The row the client sorts by, and why it is stored

Activities carry the `sequence` they were announced under, taken from the same
counter that numbers the event. It is `Schema.optional` in the contract, so an absent
one is an absent _key_ rather than a `null` — a `null` would fail the client's decode
of the whole activity.

It is stored, in schema v3, for the reason the sequence in the `orchestration` table
is: the client's ordering has to survive a restart. Rows written before the column
existed keep a `NULL`, and the client sorts an activity with no sequence ahead of one
with — which is where an older row belongs.

### A drifted block now costs a block rather than a message

`#[serde(other)]` catches an unrecognized `type` and nothing else, so a `tool_use`
with the id missing failed to parse the _whole line_ — losing the reply beside it
and counting a parse error. `Message::content` now deserializes each block behind
an `untagged` fallback, so a block that does not fit becomes `Unknown` and the rest
of the message survives. That is the module's own blast-radius argument applied one
level down.

`server_tool_use` and `mcp_tool_use` are deliberately not read. Both are tool calls
the _API_ runs, so their results come back as their own block types rather than as a
`tool_result` in the next user message — an invocation this server could announce
and never settle, which after the turn ended would render with a success tick. They
arrive as `Unknown` and are counted as drift, which is the honest answer until there
is something to pair them with. That count is the one review found was never
incremented; see "What review caught".

### The coverage the captures owe is asserted, not assumed

`the_captures_cover_the_wire_format` now counts tool calls, tool results, failed
tool calls, sessions making several calls, and thinking blocks across the whole
fixture set. The ticket asks for three cases to be covered; a capture set that
later narrowed to one happy read would otherwise still pass everything.

### Known costs, none of them fixable from this side

- **A message and an activity inside one millisecond may render out of order.**
  `deriveTimelineEntries` interleaves the transcript and the work log by `createdAt`
  alone, and `OrchestrationMessage` has no ordering field for the server to fill —
  `sequence` exists on an activity and nowhere else. On a tie the stable sort puts
  messages first, so a tool row could appear after a reply that followed it. In
  practice the two are separated by the tool's arguments streaming from the API,
  which is real time; the pair that _can_ land together is a result and the next
  invocation, and both of those are activities, which is the case `sequence` now
  covers.
- **A successful call whose output looks like a failure renders as one.** See "What
  review caught". The cost of the row showing the result at all.
- **An `ExitPlanMode` call leaves its result row behind.** Upstream suppresses a
  plan-mode row by matching `ExitPlanMode:` at the front of `detail`; the invocation
  still matches, the result no longer does because its detail is the output. Today
  that row is the only place a plan appears at all, so it is more useful than not.
  Rendering a plan as a plan is ticket 13's.

### Not verified here

- **The real window.** The spec's rule is that UI rendering is upstream's and that
  the real UI driving a session is verified manually at each build-order milestone.
  That pass has not been run in this session. Everything the server owes it is
  driven through the socket, including the four fields the collapse and the ordering
  depend on.
- **Two calls the CLI batches into one message.** Every capture interleaves, and
  the argument above is that this is what makes pairing work. If a `claude` release
  ever emitted `use A`, `use B`, `result A`, `result B`, the four rows would each be
  correct and labelled and the UI would stop merging them into two. The server would
  need no change; the work log would.
- **A tool call whose result never arrives.** The turn ends, the row stays
  `inProgress`, and the UI's `showSuccessIndicator` gives a settled turn's neutral
  row a tick — so an abandoned call would read as having succeeded. Nothing here
  drives it, because nothing short of killing the agent mid-tool produces one; it is
  the same territory as ticket 14's interrupt, which is where a turn learns to say
  what it left unfinished.
- **A `tool_result` array carrying images.** `text_content` handles the array shape
  and drops non-text blocks, which is unit-tested; no capture contains one, because
  it takes a tool that returns an image.

### The line budget

The server is at just over 18K lines against the spec's "roughly 20K" signal to stop
and re-evaluate — up about 1,700 for this ticket, of which the new `worklog` module is
a quarter and most of the rest is the socket tests over it. Thirteen tickets remain
and one of them (20) is substantial; the 20K signal will be reached before they are,
which is worth saying now rather than discovering at ticket 18. The three recordings
add about 70KB to `fixtures/`, which is evidence rather than code.
