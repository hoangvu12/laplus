# 11 — Multi-turn continuity and transcript persistence

**What to build:** A developer sends a follow-up message and the agent remembers
what was already discussed. They close the app, reopen it the next day, and their
conversation is still there — able to be read, and able to be continued.

Continuity across turns uses the agent CLI's own session identity and resume
capability rather than replaying history into each prompt. Transcripts are stored
alongside the project registry.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-human

- [x] A follow-up message in the same conversation retains prior context — one
      long-lived child takes every turn, and the evidence is the agent's own
      behaviour rather than the server's word for it: the scripted agent's turn
      counter lives in its process, so a re-spawn would answer the second turn
      with the first script.
- [x] Several turns can be exchanged in sequence without the session degrading
- [x] A conversation and its full transcript survive an app restart — messages,
      work log, and how the last turn went, in the project list and in the thread
- [x] A restored conversation can be continued, not just read — `--resume` with
      the session the agent announced on the previous run, observed as the argv
      the second process was given
- [x] Resuming a session whose underlying agent session is no longer available
      fails with an explanation and leaves the transcript readable
- [x] Transcript writes do not block or stutter the live stream — the disk is
      touched at message boundaries and never per token, from a thread of its own
- [~] A very long transcript loads without stalling the UI — a 400-message
      conversation is restored whole and in order, and the snapshot is built on the
      subscription's own task rather than the socket's read loop. What is *not*
      driven is a transcript large enough for the boot-time read to be felt; see
      "Not verified here".
- [x] Tests cover multi-turn exchange and restart-then-continue through the socket
      boundary

## Comments

### The two halves of this ticket are two different mechanisms, and conflating
them would have produced a worse version of both

"A follow-up remembers" and "a conversation survives a restart" read as one
feature and are not. The context of a conversation is in the **agent's** store;
the transcript is in **this server's**. So:

- Continuity is `--resume <session-id>` and nothing else. The alternative — this
  server replaying its transcript into every prompt — would be a second, worse
  copy of the conversation, one the agent had no reason to believe and one that
  would double the cost of every turn in a long session.
- Persistence is rows in SQLite, and its only job is that a developer can *read*
  yesterday's conversation and that the thread list has something to show.

Which is why the two are tested against different evidence.
`socket_continuity.rs` proves continuity from the agent's side — the turn counter
in its process, and the recorded argv — and persistence from the disk's, with a
second server started on the same file. A test that asserted both off the same
observation would be asserting that the server believes itself.

### The session id is the agent's, read off `init`, and not minted here

`--session-id <uuid>` exists and would let the server choose the identity up
front. It is not used, for the reason ticket 10 gave for the model and the
permission mode: what is worth storing is the session **in force**, and the
agent's `init` line is the only account of that. A resumed session announces one
too — the CLI is free to hand back a new id for a forked conversation — so the
thread always holds the most recent, which is the one the next `--resume` is
given. `the_agents_session_is_remembered_without_being_announced` drives both
halves.

It is also the one field on a thread that is neither in the contract nor derived
from it, so `Threads::remember_agent_session` publishes **nothing**. No event
describes it and no client renders it; what it owes is a durable write. The
`session.init` activity beside it is where the same id becomes visible.

### A delta owes the database nothing, and that is the whole of "writes do not
stutter the stream"

The criterion invites a queue with a generous buffer. What it actually needs is
for there to be almost nothing to buffer, and the reconciliation rule already
supplies that: a token delta is superseded by the buffered message a moment
later, and the buffered message is the authoritative one. So the same rule that
governs the transcript governs the table — a write happens at a *message
boundary*, and a reply of any length costs one row.

That is what makes the queue's shape defensible rather than optimistic:

- **Unbounded**, because what fills it is whole messages, which a person and an
  agent produce a few of per turn. A bounded queue would have to choose between
  stalling the publisher and dropping transcript rows, and both are worse than
  the depth this cannot reach.
- **A thread of its own, not a `tokio` task.** Every pass through the loop ends
  in a commit, and a worker parked on an `fsync` is a worker the socket is not
  using.
- **A batch of 64, not 256.** One transaction holds the database's single
  connection, and the registry's own commands take that connection from the
  socket's read loop — so a batch is also the longest a `project.create` can be
  made to wait. The number is "a few turns' worth" rather than whatever SQLite
  would tolerate.

`a_stored_transcript_holds_whole_messages_rather_than_a_row_per_token` is driven
with deltas that *disagree* with the buffered message, because that is the only
way to tell "written once" from "written per token and happened to agree".

**What it costs, and it is a real cost:** a reply the app was killed in the
middle of is not on disk. Its deltas were never written and its buffered message
never arrived. An ordinary close is unaffected — shutdown reaps the agents and
*then* flushes, in that order, because a session publishes its last changes on
the way down and those are exactly the ones a flush that ran first would miss.

> **Narrowed by ticket 15.** The cost above was stated for the hard kill and was
> being paid by every ending, including the graceful ones: the buffered message
> never arrives whether the process was killed or merely told to stop, so a
> conversation closed mid-turn came back showing a prompt nobody had answered —
> and, while the app was still open, a reply the UI renders as still growing.
> The driver now settles the streaming message on its way down, which is the
> moment it knows no buffered message is coming. This paragraph's rule is
> unchanged: a delta still owes the database nothing, and what is written is
> still one row at a message boundary. The hard kill still loses the tail,
> because the driver never runs at all. See ADR-0004's consequences, and
> `a_turn_the_app_closed_during_does_not_come_back_running`, which was pinning
> the old behaviour and now pins this one.

### A refused resume is the one failure with no NDJSON to it

Every other agent failure this server reports arrives as a line to fold. A
`--resume` the CLI will not honour does not: the child writes its reason to
stderr and exits without producing anything. So `Agent` now keeps its last stderr
line — one line, not a log — and hands it back from `stop`, once the child has
gone and the stderr reader has been joined, because that is the only point at
which "the last thing it said" is final. The driver turns "asked to resume, never
announced a session" into a sentence naming the session, saying the conversation
can be read but not continued, and quoting the agent's own words, which are far
more useful than anything this server could infer.

**The stored id is deliberately kept.** Clearing it would let the next turn start
a fresh session, which would leave the developer talking to an agent that cannot
see the transcript in front of them while nothing said so. Failing the same way
every time is worse UX and a true statement; the product decision about what to
offer instead belongs to whoever adds a "start over from here" affordance.

### Deleting a project had to learn to delete its conversations, and that is a
defect persistence introduced rather than scope creep

The client's shell reducer answers `project-removed` by filtering the projects and
nothing else (`shellReducer.ts`). Before this ticket that was harmless, because a
thread was in memory and went with the process. Now the rows go with the project
by the schema's own cascade — and without a `thread-removed` beside it, a
conversation whose project was deleted would sit in the project list until the
next restart and vanish after it, which is the worst of both answers.

Two consequences:

- **The registry says which conversations there are, not the database.** A thread
  reaches the database *eventually*, so the stored rows are a subset of what
  exists — and a project deleted seconds after a conversation started would leave
  that conversation behind if the rows were the source of truth. `Threads` is
  asked, and asked before the delete, because the delete is what makes the answer
  unavailable. (This started out the other way round; see "What review caught".)
- **`delete_project` now takes several sequence numbers.** One per event, because
  a client ignores anything at or below the sequence it holds, so a shared number
  would have every event after the first dropped. The commit guard is released
  before the second number is taken — it is not reentrant — and each announcement
  only has to be ordered against the writers it races.

`Threads::detach` also changed from `entry` to `find`. A driver can now outlive
the thread it was driving, and allocating a slot on the way out would have put an
empty one back for every conversation a project delete removed. For the same
reason `forget` keeps the driver's `JoinHandle` rather than dropping it —
`Inner::winding_down` is where a released driver waits for the shutdown that owes
it a reap.

### What is stored, and the two things that are not

A thread's row carries `modelSelection` and `latestTurn` as **JSON verbatim**.
Nothing in this database is ever queried by them — it sorts and joins on ids and
timestamps — so nine more columns would buy a shape this file has to keep in step
with the client's, for no query it enables.

Not stored: the **session**, because a session is a running process and after a
restart there is none. A restored thread comes back with `session: null`, which is
true.

Stored but coerced: a **latest turn that was still `running`**. The app stopped in
the middle of it and nothing is going to finish it, so it comes back
`interrupted` with the last moment the thread is known to have changed as its
end. Leaving it `running` would show a conversation working forever with nothing
alive to settle it. This is the hard-kill path;
`a_turn_stored_while_it_was_still_running_comes_back_interrupted` drives it as a
unit test, because a test cannot kill its own process. The *graceful*-close path
reports `error` — the agent was told there would be no more turns and stopped with
one in flight — and `a_turn_the_app_closed_during_does_not_come_back_running`
pins that it is not `running` either way.

### A message's position is stored, because a timestamp is not an order

`created_at` is milliseconds. Two messages inside one would come back in whichever
order the file happened to yield, and a transcript that reordered itself across a
restart would be a different conversation. So `thread_messages` and
`thread_activities` carry an `ordinal`, and it is the fold's position rather than
an append counter — a buffered message *replaces* one the deltas already put in
the transcript, and that one is not at the end once a turn has said several
things.

The same reasoning makes the message key `(thread_id, id)` rather than a rowid:
the reconciliation rule reaching the disk has to be an upsert of one row, or a
streamed reply would be stored twice.

### A batch is one transaction, and a batch that will not go is retried singly

A turn's several writes are worth one `fsync` rather than six, so the writer
drains what is waiting and commits it together. The hazard that buys is that one
write the database refuses rolls back everything beside it, including writes for
other conversations that had nothing wrong with them. So a failing batch is
retried a write at a time: it costs a commit per write in a case that is already
going badly, and it narrows the loss to the row that actually failed — which is
also the row worth naming in the log.

### The composer's payloads moved into the harness

`socket_turn.rs` and `socket_continuity.rs` both need `thread.turn.start` exactly
as `ChatView.tsx` sends it, including the `bootstrap.createThread` without which
the UI's first message cannot land. Two copies would be two chances for a test to
keep passing against a command the composer no longer sends, so they are in
`harness::conversation` and ticket 10's file was moved onto them.

One behavioural change fell out of that. `events_through_the_turn` now also stops
on a *snapshot* that shows the session settled, and this is not
belt-and-braces: a turn publishing hundreds of events outruns the subscription's
backlog, the pump answers by discarding what it could not deliver and describing
the world again, and the terminal event is then one of the things discarded. A
reader watching only for the event would wait for one that had already been
superseded — which is how the long-transcript test found it.

### The scripted agent grew three things, and each is an observation

- **Per-turn scripts.** The turn counter is a variable inside the process, so
  which script answers a turn is also the answer to "was this the same process".
  This is the discriminator continuity needs; a gauge reading 1 cannot supply it.
- **A recorded argv.** The arguments are the contract between this server and the
  CLI, and `--resume` is the whole of the mechanism, so the honest place to
  observe continuity across a restart is the argv the second process was given —
  not a field inside the server.
- **An agent that refuses to resume.** No recording contains one, because a
  healthy CLI does not produce one.

### What review caught

`/code-review` found five defects, and three of them were races or losses the
suite was passing straight over. Each is worth recording because each was
invisible from inside the change.

- **The refusal's most useful sentence was read before it could exist.** The
  driver asked `Agent::complaint()` and *then* reaped the child — but stdout and
  stderr are drained by separate tasks, so "the agent's output ended" and "the
  agent wrote why" are not ordered against each other. The generic sentence would
  have shipped without the CLI's own words, intermittently, and the test asserting
  the words are in it was a latent flake rather than a check. `Agent::stop` now
  *returns* the complaint, having joined the stderr reader — bounded by the same
  exit grace, for the same `.cmd`-grandchild reason — so the answer is final
  because the child has gone.
- **`Threads::forget` detached the driver it was releasing.** It dropped the
  whole `Live`, and dropping a `JoinHandle` detaches the task rather than ending
  it. So deleting a project and then closing the app left a `claude` that
  `Threads::shutdown` had nothing to wait for — precisely the one leak
  `Server::shutdown` names as unacceptable. The prompt sender is now dropped and
  the *handle kept*, parked on `Inner::winding_down`, which shutdown drains with
  the rest. Finished ones are swept on each call so the list is bounded by what is
  actually still winding down.
- **The delete asked the database which conversations existed, and the database
  finds out last.** `Removal::thread_ids` came from the `threads` table, but a
  thread reaches that table *eventually* — that is this ticket's whole design. A
  project deleted seconds after a conversation started would therefore have found
  nothing stored: no `thread-removed`, the conversation left in every shell
  snapshot until the next restart, and its queued write then refused by a foreign
  key whose project had gone. Exactly the "worst of both answers" the paragraph
  above claimed to prevent. `Threads::of_project` is now the source of truth, the
  field is gone from `Removal`, and the socket test deletes the project while the
  agent is still alive because that is the ordinary case.
- **A write in flight for a deleted conversation was refused and printed.**
  Fixing the point above left this one behind: the rows go synchronously while the
  writes are still on the queue. `Transcripts::discard` drops them instead. Not
  tidiness — a refused batch is retried one write at a time and each failure is
  logged, so deleting a project mid-conversation would have produced a page of
  complaints about an ordinary thing to do.
- **`remember_agent_session` succeeded at updating nothing.** An `UPDATE` matching
  no row is continuity silently lost, since the stored session is what the next
  run resumes into. It now refuses, which rolls the transaction back and names the
  thread and the session in the log — the same shape as the checked row counts
  `remove_project` already had.

Three smaller things went the same way: `BATCH` came down from 256 to 64, because
the batch holds the database's one connection and is therefore also the longest a
`project.create` on the read loop can be made to wait; the activity's stored
position was `activities.len() - 1` with a `saturating_sub` hiding the empty case,
and is now *found* by id like the message beside it; and the long-transcript
liveness check was tautological — it made its unrelated call after reading and
acknowledging the whole snapshot, so nothing was ever concurrent with it. The call
is now outstanding across the snapshot's arrival.

Two pieces of surface were deleted rather than defended: a scripted-agent
constructor with no callers, and the `pending_transcript_writes` chain out to
`ServerState` and the harness, which no test witnessed. The queue's own gauge
stays, exercised by its own tests.

### Not verified here

- **The real window.** The spec's rule is that UI rendering is upstream's and
  that the real UI driving a session end to end is verified manually at each
  build-order milestone. That pass has not been run in this session. Everything
  the server owes it is driven through the socket.
- **A boot that is slow because the history is large.** `Shell::new` reads every
  conversation in one pass over three tables, before the listener opens. That is
  the size of the history rather than a query per conversation, and a desktop
  app's history is what one person typed — but nothing here drives a database
  large enough to time, and if launch ever feels slow this read is the first place
  to look. The alternative, hydrating a thread's transcript on first subscribe,
  was rejected because the project list opens with every thread's *summary*, so a
  server that had not read them would have to answer its first `subscribeShell`
  with a claim that the user has no conversations.
- **A `--resume` against the real `claude`.** The flag is what the spike's
  write-up records and what upstream passes, and the scripted agent proves the
  server sends it; that the real CLI honours it for a session this server started
  is a manual check.
- **Two windows on one restored conversation.** Same position as ticket 10: the
  registry is per-server, so it should work, and nothing drives it.
- **A conversation whose agent session the CLI *silently* forks.** If a resumed
  session ever announced a new id and kept the old one alive, the thread would
  follow the new one, which is right; no capture contains one.

### The line budget

The server is at just under 17K lines against the spec's "roughly 20K" signal to
stop and re-evaluate — up about 1,800 for this ticket, of which the new
`transcripts` module is roughly a fifth and the rest is schema, SQL and the tests
over them. Worth naming now: fourteen tickets remain and three of them (12, 13,
20) are the substantial ones.
