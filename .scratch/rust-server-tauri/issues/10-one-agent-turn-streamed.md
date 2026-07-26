# 10 — One complete agent turn, streamed

**What to build:** The heart of the product. A developer types a prompt into the
real UI, watches Claude Code's reply appear token by token, and ends with a
complete, correct message in the transcript. The agent runs in the project's
directory as a long-lived subprocess driven over newline-delimited JSON.

This is the milestone the whole project has been working toward, and it is kept
whole deliberately: "agent output streams in the real UI, driven by Rust" is worth
having as one demoable thing.

**The reconciliation rule is load-bearing.** Assistant text arrives twice — once
incrementally as content-block deltas, and again as a complete buffered message.
Deltas drive live rendering; the buffered message is authoritative and replaces the
accumulation when it lands. Rendering deltas alone risks silently truncated output;
waiting only for the buffered message makes streaming pointless. From the
prototype's reducer, trimmed to the decision:

```rust
// live rendering: append deltas as they arrive
StreamEvent::ContentBlockDelta { delta, .. } => {
    if let Delta::TextDelta { text } = delta {
        self.live_text.push_str(&text);
    }
}

// reconcile: the buffered message wins
Event::Assistant(env) => {
    let text = flatten(&env.message);          // authoritative
    let from_deltas = !self.live_text.is_empty() && self.live_text == text;
    self.transcript.push(Turn { role: env.message.role, text, from_deltas });
    self.live_text.clear();
}
```

The flag recording whether the two agreed is a cheap, continuous check on that
assumption and should be observable.

Tests use a scripted fake agent executable replaying canned captures, injected
through the agent-executable-path configuration that already exists for real use —
no test-only seam is added. No test calls the real API.

**Blocked by:** 09 (Provider configuration and agent binary resolution), 05
(Project registry), 04 (First streaming subscription).

**Status:** ready-for-human

- [x] A prompt sent from the real UI reaches the agent and is acknowledged
      immediately — the command the real composer sends is what the suite sends,
      including the `bootstrap.createThread` without which the UI's *first*
      message cannot land. The window itself has not been driven by hand here;
      see "Not verified here".
- [~] The reply renders incrementally as it is produced, not in one jump at the
      end — the server publishes each delta as its own event and a test reads
      assistant text off the wire while the session is still `running`. That the
      pixels move is upstream's to render and is the manual check below.
- [x] The final transcript text equals the buffered message, even when deltas were
      shed
- [x] Whether deltas agreed with the buffered message is recorded and observable
- [~] The session's model and permission mode are shown in the UI — both are the
      agent's own account of them, published as a `session.init` activity, which
      is the contract's mechanism for this and is rendered by the work log for
      any kind it does not suppress. That it appears on screen is the manual
      check below; the permission mode is also on the session as `runtimeMode`,
      which the composer's picker reads directly.
- [x] A completed turn reports its duration and cost
- [x] The agent runs with the project directory as its working directory
- [x] The subprocess is spawned once and stays alive across the turn, rather than
      per-request
- [x] The subprocess is terminated and reaped when the session ends
- [x] A scripted fake agent executable replays captured sessions deterministically,
      offline, at no cost
- [x] Tests drive a full turn through the socket boundary and assert the streamed
      event sequence and the final transcript

## Comments

### The reconciliation rule was already in the client's vocabulary

The ticket states the rule as a reducer, which invites implementing it as one —
accumulate deltas, hold them, publish once. That would have been wrong, and the
evidence is in `threadReducer.ts`:

```ts
text: message.streaming
  ? `${entry.text}${message.text}`      // append
  : message.text.length > 0
    ? message.text                      // replace
    : entry.text,
```

There is no delta event in the contract. There is one `thread.message-sent` with
a `streaming` flag, and the flag *is* accumulate-and-reconcile: `true` appends,
`false` replaces. So a token delta is a streaming send carrying the delta, and
the CLI's buffered `assistant` message is a non-streaming send carrying the whole
text — which replaces whatever the deltas built, which is precisely what has to
happen when a delta was shed. The client was already written for this.

`crate::threads::Threads::message_sent` folds the server's own copy the same way,
including the client's one exception: an *empty* buffered message leaves the
accumulation alone. Without that, a turn the developer watched arrive would blank
itself at the end. Both halves are driven —
`the_transcript_holds_the_buffered_message_even_when_deltas_were_shed` and
`an_empty_buffered_message_does_not_erase_the_accumulation`.

### One reducer, because two would agree until they did not

The obvious shape for the driver is its own fold over `protocol::Event`. That
puts "the buffered message wins" in two places — the reducer the golden files
check, and the one the UI actually sees — and the one under test would not be the
one that matters.

So `protocol::SessionState::fold_line` now *returns* what the line changed
(`protocol::Folded`), and `crate::turn` publishes off that. The rule stays in one
function, `fixtures/claude-cli/` still holds it, and the driver is a match on five
variants. The addition is small and pure, which is what let it into a module whose
whole value is being pure.

`Folded::Streamed` carries the delta text rather than an offset, because the
accumulation is cleared out from under the caller when the turn reconciles.

### The sequence counter had to leave the database, and that is the one
structural change to earlier work

Ticket 05 put the orchestration sequence in SQLite, incremented inside the same
transaction as the write, with a good reason: a client caches a snapshot and
ignores events at or below the sequence it holds, so a counter that restarted at
zero would make the next few changes invisible.

A streamed turn publishes an event per token. Keeping that counter in SQLite would
mean a transaction commit — an `fsync` — per token of a reply that is not
persisted at all.

`store::Sequences` is the answer: an in-memory counter **seeded from the database**,
and a durable write records the number it *was given* rather than choosing one.
Both of ticket 05's properties survive — a restart never re-issues a number a
committed change used, and the stored value stays a high-water mark. What it costs
is gaps, because a number is taken before a command knows whether it will commit.
Nothing reads this as a dense log, so a gap is invisible; a reused number would
not be.

Two consequences worth naming:

- **`stamp` uses `MAX(sequence, ?)`.** Two commits can take their numbers in one
  order and reach the database in the other, and a plain assignment would let the
  slower one lower the high-water mark that the next boot resumes from.
  `a_commit_never_lowers_the_stored_high_water_mark` holds it.
- **The shell snapshot reports the counter, not the stored column.** They differ as
  soon as a conversation is under way, and a snapshot reporting the *stored* number
  would be older than events the client had already folded, so the client would
  re-apply them.

Projects and threads share the one counter because they share one subscription,
and a client folds both against one cursor. Two counters would have a thread's
events overtake a project's and the client would drop whichever fell behind —
`projects_and_threads_are_numbered_from_the_same_counter`.

### `thread.create` is not how the UI starts a conversation

This was the discovery that would have shipped a server the real UI cannot talk
to. A new conversation is a **client-side draft**: the composer subscribes to a
thread id the server has never heard of, and the thread only arrives when the
first turn is dispatched, carrying `bootstrap.createThread`
(`apps/web/src/components/ChatView.tsx`, guarded by `isLocalDraftThread`). A
server implementing only `thread.create` would answer the UI's first message with
"there is no such thread".

Two things follow, and both are in the code because of it:

- `thread.turn.start` creates the thread when the turn asks it to. Every turn in
  `socket_turn.rs` goes through that path, because it is the one the composer
  takes.
- **A subscription to a thread that does not exist opens and stays silent.** It
  cannot refuse — the UI opens it before the server knows anything — and it must
  not answer with an empty thread, which would be a positive claim that the
  conversation is empty and would wipe the messages the composer is optimistically
  showing. `Threads::entry` therefore makes the slot before the thread, and the
  same subscription carries the thread once a turn creates it.

`thread.create` is implemented as well, and is not speculative: the UI dispatches
it directly from `ChatView.tsx:5226` through `EnvironmentCommands.createThread`.
The composer's first message is the bootstrap path; a thread created any other
way is this one.

### `enableAssistantStreaming` is now `true`, and that is a report rather than a
switch

Upstream defaults it `false`, and `false` is not a cosmetic preference: its
ingestion buffers up to 24,000 characters before sending anything
(`ProviderRuntimeIngestion.ts`, `MAX_BUFFERED_ASSISTANT_CHARS`), which is a reply
that appears in one jump at the end. That is the thing this ticket's second
criterion rules out.

The setting is **not honoured as a branch**, and deliberately: nothing can write a
setting until ticket 22, so a `buffered` path would be code no test could reach
through any real route — the same argument that removed custom model slugs in
ticket 09. What the field does here is report what the server does, and reporting
`false` while streaming anyway would be the payload disagreeing with the wire
beside it.

### Duration and cost go in an activity, because the contract has nowhere else

`OrchestrationLatestTurn` carries timestamps and no money, and upstream's own
`totalCostUsd` never leaves its internal provider-runtime bus — it is on
`TurnCompletedPayload` in `providerRuntime.ts` and has no orchestration
equivalent. So a completed turn appends a `turn.completed` activity whose
`summary` is the sentence ("Turn completed in 2.0s · $0.0795 · end_turn") and
whose payload carries the numbers for a later ticket to render properly.

This is genuinely visible rather than merely recorded: `deriveWorkLogEntries`
(`apps/web/src/session-logic.ts`) renders any activity kind it does not
specifically suppress, and `turn.completed` is not on that list. The same
mechanism carries `session.init`, which is where the model and permission mode
the criterion asks for are shown — and they are the agent's *own* account of both,
read off its `init` line rather than off what it was asked for, because a mode can
be overridden by the user's own settings file.

Cost is printed to four decimal places: a short turn costs a fraction of a cent
and two places round every one of them to zero.

### Where a turn ends, and why not at the last thing the assistant said

`threadReducer.ts` settles a turn when the session leaves `running`, not when an
assistant message completes — because a provider sends several messages in one
turn, commentary between tool calls. The server's fold mirrors it exactly
(`threads::bind_assistant_message` and `threads::settle`). Getting this wrong
settles a turn in the middle of itself, which is the failure ticket 12's tool
round-trips would hit first, and it is why the fold is written against the
client's rather than to taste.

The turn's reported duration therefore covers the whole turn rather than stopping
at the last token, which is also what makes `latestTurn.startedAt`/`completedAt`
worth anything.

### The counters are a ratio, not an error count

"Whether deltas agreed" is two numbers on `ServerState` —
`reconciled_messages()` — beside the drift counters ticket 03 established for the
socket and `protocol` keeps for the CLI. It is deliberately a *ratio*: a turn that
used a tool or thought before answering legitimately ends with deltas that do not
equal the buffered text, because the buffered message flattens blocks the deltas
never carried. Reporting only disagreements would report normal turns as failures.
What is alarming is the agreeing count going to zero on plain turns, and both
halves are driven.

A disagreement also prints one line to stderr naming how many streamed characters
were replaced by how many buffered ones, which is the number a developer wants
when a reply looked truncated.

### The scripted agent, and the one test that nearly ran the real one

`harness::agent::ScriptedAgent` writes a `.cmd` (or a shell script) that reads a
turn on stdin, prints a canned NDJSON script, and waits for the next one — the
whole of the protocol from the server's side. Two of the scripts are the committed
recordings in `fixtures/claude-cli/`, so a turn is driven against what `claude`
actually said; the rest are written for cases a healthy CLI does not produce.

It reaches the server through `settings.providers.claudeAgent.binaryPath`, which
is what the spec asks for in so many words — "no test-only seam is added to
production code". Everything downstream is the production path: resolution, the
child, the stdio, the fold, the events, the socket.

Two things about it are worth keeping:

- **`PAUSE`** splits a script into segments with a second between them. It is what
  makes "not in one jump at the end" observable at all: against an agent that
  answers instantaneously, reading the deltas before the buffered message proves
  only that they were in the list.
- **The working-directory marker.** Every scripted agent writes a file into
  whatever directory it was started in, and
  `the_agent_runs_in_the_projects_directory` looks for it in the project folder.
  The alternative — having the agent print its own path — needs a Windows path
  JSON-escaped by a batch file, which is mostly backslashes.

**The failure test nearly started the developer's own agent.** Its first version
configured a `binaryPath` that did not exist, which is exactly the case ticket 09
decided should fall back to `PATH` — so it found the real `claude`, ran a real
turn, and failed on a missing activity. It now points at a file that exists and is
not a program, which is the one unusable case that does *not* fall back. The
comment on the test says so, because the next person to write a failure case will
reach for the same missing path.

### Declared divergences

- **A turn that asks for a git worktree is refused by name.** The composer sends
  `bootstrap.prepareWorktree` when the project is in worktree mode. Running the
  turn in the project root instead would put the agent's changes somewhere the
  developer did not ask for, so it is refused rather than approximated. Reachable
  only by changing `defaultThreadEnvMode`, which is `local`.
- **Attachments are decoded and dropped.** Pasted images need the asset service
  the spec puts out of scope. Carried in the payload so a client that sends them
  is not refused.
- **`approval-required` passes no `--permission-mode`.** Upstream's table
  (`ClaudeAdapter.ts:3510`) maps three of the four runtime modes and expresses the
  fourth by omitting the flag and answering the CLI's permission callback.
  lightcode has no callback until ticket 13, so it omits the flag too — right for
  a turn that uses no tools, and ticket 13's to make right for one that does.
- **The four correlation fields on every event are `null`.** They exist upstream
  so an event can be traced to its command through an event store lightcode does
  not have. Inventing ids would describe a causation chain nothing recorded.

### Threads live in memory, and that is ticket 11's boundary rather than a gap

Ticket 05 argued that a project registry which forgets its projects is a different
feature, not a smaller one, and persisted from the start. A thread is not the same
case: what makes a conversation survive a restart is the CLI's own `--session-id`
and `--resume`, not a table of messages the agent has forgotten about — and ticket
11 owns continuity and persistence *together* for exactly that reason. Storing
transcripts now would mean storing them again differently in a fortnight.

### What review caught

Two rounds. The first was mine while writing:

- **The turn read the thread before applying the composer's selection.** A model
  or runtime mode picked for this turn arrives on `thread.turn.start` and is
  per-turn in the contract; reading the thread first meant the agent was started
  with the model the thread was *created* with, so a mid-conversation model change
  would have been shown in the UI and ignored by the server. The three optional
  fields are now applied before the thread is read, and each is `Option` rather
  than defaulted — a default would have moved every conversation back to
  `full-access` on every turn.
- **`select!` cannot send to the agent from inside a branch** while the other
  branch's future holds the mutable borrow. The loop now names what it heard
  (`turn::Next`) and acts after the select, which is the same shape and compiles.

The second was `/code-review`, and it found four defects the suite was passing
over. Each is worth recording because each was invisible from inside:

- **A unit test was running the developer's real `claude`, against the real API.**
  `orchestration::tests`'s fixture used `ServerConfig::detect()`, whose
  `binaryPath` is the bare name `claude` — and a bare name is looked up on `PATH`,
  which is the whole point of the default. So
  `a_turn_creates_the_thread_it_was_sent_for` dispatched a real turn to the
  installed binary with `--permission-mode bypassPermissions` and waited for it.
  The socket tests had been fixed for exactly this a paragraph earlier and the
  unit fixture had not. It now configures a file that exists and is not a program,
  which is the one unusable case that does *not* fall back to `PATH`, and the
  comment on the fixture says why that specific shape.
- **The session could only ever become `running` once per process.** The
  transition was driven off the agent's `system/init` line, which a long-lived
  child prints once for the whole conversation rather than once per turn. Every
  turn after the first would therefore have stayed `starting` — and a session that
  is not `running` makes `bind_assistant_message` settle the turn at the *first*
  assistant message, which is precisely the mid-turn settle two comments in this
  change claimed to be avoiding. The transition moved to the prompt being sent,
  where it belongs. The test that should have caught it could not: it read
  `live_agents()`, which is 1 whether a session was reused or re-spawned, so it
  now counts *starts* — the scripted agent logs one line per process — and asserts
  the second turn reached `running` and settled.
- **The shared counter did not actually order the two aggregates.** Projects took
  a number under one lock and threads under another, and both published onto the
  one feed the project list folds. A project numbered 5 published after a thread
  event numbered 6 is not reordered by the client, it is *dropped* — the project
  would simply never appear. `store::Sequences::commit` now returns a guard held
  from taking the number to announcing the change, which is the same thing ticket
  05's `Mutex<()>` did for one aggregate, done for both.
  `a_number_is_not_handed_out_while_the_last_one_is_still_being_announced` holds it.
- **`start_turn` published before it could still fail.** The prompt, the turn
  request and the session went out before the project lookup and before the prompt
  was handed to a driver, so a refusal left a conversation showing a message and a
  turn marked running with nothing alive to settle it. Everything fallible now
  happens first, and the one failure that cannot be moved earlier — the driver
  refusing the prompt — ends the session as well as returning the error.

Four smaller things went the same way: a queued prompt arriving mid-turn used to
orphan the turn in flight and misattribute its cost, and now waits; `Threads`'
queries used to *insert* the slot they asked about, so any id a client mentioned
leaked one; `shell_summaries` sorted on a millisecond timestamp alone, which is
not a total order; and three doc comments asserted things the code did not do.

### Not verified here

- **The real window.** The spec's own rule is that UI rendering is upstream's to
  test and that "the real UI connects and drives a session end-to-end is verified
  manually at each build-order milestone". That manual pass has not been run in
  this session. Everything the server owes it is driven through the socket, which
  is the genuine contract, but the first two criteria name the UI and are marked
  accordingly.
- **A `claude` that changes its format mid-turn.** The drift counters survive it
  and the turn's `turn.completed` activity carries them, but no capture contains
  one.
- **Killing a wedged agent on Windows.** `Agent::stop` closes stdin, waits, and
  kills — which for the suite's `.cmd` stand-ins kills `cmd.exe` rather than the
  program behind it. The real agent is a native `claude.exe` spawned directly,
  where the kill does what it says. Same reasoning and same conclusion as
  `provider::probe`'s timeout.
- **Two windows on one conversation.** The registry is per-server and the
  subscription is per-connection, so it should work; nothing drives it.
- **A turn dispatched in the window between an agent exiting and the driver
  noticing.** `Threads::attach` hands back the existing prompt channel until the
  driver has released it, so a turn sent during the reap — at most the two-second
  kill grace — is queued into a channel nobody will read again. The session
  publishes its end immediately afterwards, so the UI shows a stopped session
  rather than a hang; making the turn itself survive is resilience, which is
  ticket 15.
