# Context

laplus's domain language. One term per entry, as the code uses it.

Two vocabularies meet in this project and it is worth naming which is which:
the **agent protocol** is what the `claude` CLI speaks, the **contract** is what
the reused UI speaks. Where a term exists on both sides, the entry says so.

## Conversation

**Thread** — one conversation, scoped to a project. What the UI reads. Survives
a restart; the agent process behind it does not. `crate::threads`.

**Change** — one thing that can happen to a conversation, as a value. Every move
a thread makes is one of these, whoever asked for it: a command from the
developer, an event from the agent, or this server's own
**lifecycle reset**. `crate::threads::Change`.

**Fold** — taking a change and a conversation and answering with the conversation
as it now is, together with the payload that describes the move to the client.
`crate::threads::fold`, and the one thing this server does that upstream also
does, in `threadReducer.ts`.

Given the same conversation and the same change it answers the same way — it
reads no clock, takes no lock, opens no channel and touches no process. The
moment a change happened is decided before the fold and handed to it, which is
why a re-emitted settle can report the stamp the conversation already carried.
Five constructors in the module do read a clock, and they are named in its doc:
they build the changes that are handed _to_ the fold, from outside it. ADR-0025
is the decision and what was weighed against it; ADR-0002 is why the cut is here
and not between the transcript and the running agent.

Not the refs picker's **folded away**, which drops a remote ref shadowed by a
local branch. This entry has seniority: it is the meaning an identifier carries.

**Title** — what the developer calls a thread or a project, and the only field
either one carries purely so that it can be found again. Both are the contract's
trimmed non-empty string, so a blank one is refused rather than stored — a
conversation called "" is not a smaller thing than a named one, it is a row
nobody can pick out of a list. A project registered with no title takes its
folder's name (`projects::WorkspaceRoot::inferred_title`); a thread with none
takes its project's.

An OpenCode thread is the exception to developer-only ownership: every non-empty
title on an upstream `session.updated` event becomes the thread title, matching
T3 Code even when that overwrites a manual rename.

Moved by `thread.meta.update` and `project.meta.update` — and **the first of those
is not only the rename control.** The composer sends it before every message whose
model or branch differs from the thread's, and writes those two fields with it, so
a refusal there stopped the message queued behind it.
`orchestration::UpdateThreadMetaPayload` is the whole of what it may carry; a
project's title is the whole of what the other one changes here. See the
thread-lifecycle tracker, ticket 03.

**Turn** — one exchange within a thread: the developer's prompt and everything
the agent does before it goes quiet. Has an id the client mints.

**Steer** — an additional developer prompt incorporated into the OpenCode turn
already running, retaining that turn's id. This is an OpenCode capability, not
the behavior of a developer message sent while Laplus is working. _Avoid_:
Queued turn.

**Queued turn** — one or more developer prompts accepted while another turn is
running or settling. The prompts stay separate in the transcript but start one
new exchange after settlement. The new exchange retains the preceding reply,
including a partial interrupted reply, as conversation history. _Avoid_: Steer,
follow-up message.

**Chat attachment** — a file stored by laplus and included with a developer
prompt. Its provider representation is driver-specific: Claude and Codex
receive the image contents, while OpenCode receives its resolved local `file://`
URL. An external OpenCode server can consume it only when it shares that
filesystem or an equivalent path mapping. _Avoid_: Attachment alone, which
already means a terminal output subscription in this context.

**Session** — the agent process behind a thread, as the client sees it. A thread
with no session is normal — after a restart, every thread has none.

**Driver** — what runs one agent behind a session: it owns the process, speaks
the selected protocol, and answers with the changes a conversation is owed. A
trait, `crate::session::Driver`, and its surface is the I/O verbs only — open a
session, take the next event, send a prompt, interrupt, answer what the agent
stopped for, ask how full the window is, retune, say there will be no more
turns, reap. Everything a session does _around_ those is `crate::session`'s and
is written once: baselines, checkpoints, epochs, settling, and every session
event the client reads. A proprietary protocol may have one dedicated driver;
a standard protocol may have one driver shared by many conforming agents.
ADR-0001 is why an encoder belongs to a driver and the decoder does not.
`crate::turn` drives the `claude` CLI and `crate::codex` drives Codex app-server.

Selected through `crate::provider`'s configured-instance resolver: a provider
instance id is the routing key a conversation records, while the driver slug
says which implementation runs it. They happen to both be `claudeAgent` for
the default Claude instance and are separate fields;
`crate::provider::ProviderIdentity` keeps the pair durable on the thread.

**Provider instance** — one configured identity of a driver, with its own
settings, catalogue and continuation namespace. Several instances may use the
same driver; a thread routes to the instance id, not merely the driver kind.
The durable Claude and Codex defaults are ordinary entries in
`Settings.provider_instances`; their legacy `providers` buckets are accepted
only at the settings boundary and normalized there. Snapshots, refreshes and
sessions never fall back to the legacy shape.
_Avoid_: Provider, when the distinction affects routing or configuration.

**ACP agent instance** — a provider instance whose agent speaks the Agent
Client Protocol. Conforming agent harnesses share the ACP driver; their command,
environment and compatibility policy distinguish instances without creating a
new protocol implementation.
_Avoid_: ACP provider, when it implies a separate driver for every agent.

**ACP runtime** — the shared Agent Client Protocol boundary used by every ACP
agent instance. Agent-specific compatibility policy may amend the boundary but
does not create another ACP runtime.
_Avoid_: Cursor runtime, generic provider.

**App-server** - Codex's JSON-RPC mode, started as `codex app-server` and spoken
to over newline-delimited JSON on stdio. Responses omit `jsonrpc`, may arrive out
of order, and are correlated by client request id; requests travelling the other
way have their own id space. A provider probe owns one short-lived app-server.
Each Codex conversation owns another for its session, one process per
conversation by ADR-0032. `crate::codex_protocol` is the pure wire vocabulary
and decoder; `crate::codex` owns both app-server lifetimes and implements the
Codex driver.

**Owned OpenCode server** — an `opencode serve` process laplus starts for one
conversation and stops with that conversation. _Avoid_: Local server, embedded
server.

**External OpenCode server** — an OpenCode HTTP endpoint configured by the
developer whose availability, transport security and lifetime laplus does not
own. _Avoid_: Remote server, because the endpoint may be on loopback.

**Provider resume cursor** — a driver's opaque, versioned description of how to
continue a conversation after its runtime is gone. _Avoid_: Agent session id,
because a cursor may contain more than one upstream identifier.

Legacy Claude and Codex rows contain only the upstream id as a string; each
driver reads that as its v0 cursor. New cursors are JSON owned and validated by
the driver that wrote them. An established cursor that is malformed or from an
unsupported future version is surfaced as incompatible rather than silently
discarded into a fresh conversation.

Writing it down is the whole of what a successful open does. Claude announces
the handle on its `init` line; Codex returns it from `thread/start` or
`thread/resume`. A successful start or resume produces no activity: `init` used
to open every conversation with a row naming the model, the permission mode and
the tool count, and that row was a standing difference from upstream restating
what the composer already shows. Its one real loss is recorded in `crate::turn`,
`Folded::Initialized` — the mode the CLI reports is the one _in force_, and the
picker shows the one that was asked for.

A Codex resume refusal is recoverable rather than a dead session: every error,
without classifying its wording, falls back to `thread/start`, replaces this id
with the fresh thread id, and publishes `session.resume-failed` so the developer
knows the agent no longer has the previous context. `crate::codex`.

OpenCode resumes more narrowly: only a structured missing-session answer starts
fresh. Transport, authentication and server failures preserve the cursor and
surface the failure; a session found under another canonical working directory
is forked into the requested one so its history follows the thread. An adopted
session has the current runtime permissions re-applied before it is used.

**Runtime mode** — how much latitude the agent is given, as the contract's four
closed literals: `approval-required`, `auto-accept-edits`, `auto`, `full-access`.
A property of the **thread**, editable between turns by
`thread.runtime-mode.set`, and `crate::orchestration::RUNTIME_MODES` is the whole
of what one may be.

On restore, a stored value outside that set is rounded to `full-access`; the
reader does not migrate the database row.

Not to be confused with the CLI's **permission mode**, which is what a runtime
mode is _translated into_ — `--permission-mode`, one of the agent protocol's own
words. There are **two translations**, because the question is asked at two
moments: `crate::agent::permission_mode_for` is the launch flag and is lossy —
`approval-required` maps to _no flag at all_, because upstream expresses it by
answering the CLI's permission callback instead — while
`crate::agent::pushed_permission_mode_for` is the same table as a _push_ and is
total, mapping `approval-required` to the CLI's `default`. A push has no way to
say "pass no flag", and `default`'s behaviour is to ask, which is what the mode
means.

Codex translates the same runtime mode into an approval policy and sandbox at
`crate::codex_protocol::Access::for_runtime_mode`: `approval-required` is
`untrusted` and read-only, the two middle modes are `on-request` and
workspace-write, and `full-access` is `never` and danger-full-access. The
developer is always named as the reviewer, including on resume. `auto`
deliberately matches `auto-accept-edits` until the client can render the OpenAI
reviewer's work; the mapping carries the reason for that divergence.

OpenCode follows T3 Code's more conservative two-way translation:
`full-access` allows every OpenCode permission, while all three other modes ask
for every permission except the separate `question` capability. Consequently
`approval-required`, `auto-accept-edits` and `auto` behave alike for this
driver; their distinct stored names do not imply distinctions OpenCode has not
been configured to enforce.

A mode is given to the child at launch and **retuned afterwards**, at the next
turn's dispatch. Claude uses its control channel: `set_permission_mode`, with
`set_model` beside it for the model. Codex makes the same change through sticky
`turn/start` overrides: the model, approval policy, `sandboxPolicy`, and the
explicit `user` reviewer travel together. Once one value moves, later turns keep
sending the full set rather than inheriting an unknown remainder from the thread.

Both paths keep the process and its history: no `--resume`, no fresh
initialization, no lost context window. Retuning at dispatch rather than when the
picker commits also means a turn already in flight keeps the rules it started
under. A Codex `turn/start` refusal is the acknowledgement that its retune did not
land, and is published as the turn's correlated error rather than dropped.

The wire publishes the two values at different established boundaries:
`runtimeMode` is on `OrchestrationSession`, while `modelSelection` is on the
thread and each `thread.turn-start-requested` event. Session events therefore
stay mode-consistent for a turn, and the turn request records the model paired
with it; adding a model to `OrchestrationSession` would not match the contract.

**Retune** — telling an agent already serving a conversation to change what it
_is_: its permission mode, its model, or both. The name for the act and for what
carries it — `crate::threads::Retune` travels with the prompt,
`crate::session::retune` spends it through the driver, and `session.retune-refused` and `session.retune-failed` are what the
developer is told when it does not land. Distinct from the two other things this
server says to a running agent: an **interrupt** ends the turn and a **permission
decision** answers a question, and neither changes what the agent is.

It travels with the **prompt** rather than as a signal, and that is the pairing
rule: a mode belongs to one turn, so two turns queued behind a running one — with
the picker moved between them — must each be answered under the mode they were
requested under.

**Interaction mode** — whether the developer is planning or acting:
`default` or `plan`. Also a property of the thread, also editable by a command of
its own, and the one mode this server never acts on — it is stored, published,
and never reaches the CLI. Carried so the picker has something true to show
across a restart.

On restore, a stored value outside that set is rounded to `default`; the reader
does not migrate the database row.

**Work log** — the row-per-thing-that-happened view beside the transcript: tool
calls, thinking, permission requests and their resolutions. `crate::worklog`.

**Activity** — one row in the work log.

**MCP session** — a conversation-scoped capability through which its agent can
call Laplus host tools. It ends with the owned agent lifetime and is distinct
from an agent provider's own conversation session.
_Avoid_: MCP server, provider MCP

**MCP grant** — the secret authority to use one MCP session. Possession grants
access only to that conversation's registered host tools.
_Avoid_: token, API key

**Toolkit** — a named collection of host tools offered through MCP, independent
of the agent provider that calls them.
_Avoid_: provider tools, MCP server

Codex's **commentary** phase is not one. `agentMessage` labels prose before tool
use `commentary` and prose after it `final_answer`; both are messages addressed
to the developer and are published in the transcript as separate messages. The
work log records what the agent did between them. Treating commentary as an
activity would require a generic kind the Claude driver does not produce and
would trade streamed prose for a status row. `crate::codex`.

**Approval** — a permission request and the developer's decision on it: one of
`accept`, `acceptForSession`, `decline`, `cancel`. `crate::worklog::Decision`,
which is contract vocabulary. Claude always exposes the four decisions and
translates them to two wire behaviours with a modifier on each. Codex carries
the decisions each request permits; structured execpolicy and network-policy
amendments are recognized but not offered onward. Its JSON-RPC request id stays
opaque beside the client-facing request id so the answer preserves the original
JSON type. `crate::approval::ApprovalRequest`,
`crate::codex_protocol::ApprovalRequest`.

**User input** — the agent asking the developer a multiple-choice question, and
the answers. On the wire it is an approval — a `can_use_tool` for the
`AskUserQuestion` tool, whose input is the questions — and it is one everywhere
else: its own activity kinds, its own fold in the client, its own composer
surface, and an answer that goes back as an `allow` carrying the answers in
`updatedInput` rather than as a decision. `crate::worklog::questions` is the
place the two are told apart, and the fork is by what this server can _render_
rather than by the tool's name: a question it cannot express as one is published
as the approval it arrived as, because the client silently discards a question
payload it cannot parse and the developer would be left with nothing to answer.

Not to be confused with the terminal's **Question**, which is a control sequence.
The collision is upstream's word against a VT one and neither is worth renaming.

**Catalogue** — what the composer can offer the developer to type: the agent's
**slash commands** and the developer's **skills**, the two lists on a provider
snapshot. `crate::catalogue`. Neither is a capability this server implements —
selecting from either menu only types `/name ` or `$name ` into the prompt, and
the CLI is what acts on it — so both are read from where the CLI reads them
rather than written down here. Claude's commands come from its handshake and its
skills from disk; Codex answers `skills/list` for the registered workspaces in
the same app-server probe that supplies its account and models.

OpenCode's catalogue is its connected upstream models plus its visible primary
agents, model variants and skills. A local instance reads them from `opencode
models --verbose`, `opencode agent list` and `opencode debug skill`; an external
instance asks its HTTP API. Only connected upstream providers contribute
discovered models, while custom configured models remain available as fallback
entries.

**OpenCode reports its own skills; this server does not scan for them.** `GET
/skill` and `opencode debug skill` answer the same four fields, so both discovery
paths publish the same list — which is the point, because OpenCode resolves its
own search paths, its own precedence and its own built-ins, and a scan here would
be a second opinion about another tool's rules. Two consequences: the skill body
is dropped, being tens of kilobytes the `$` menu has no use for; and `scope` is
set only for a built-in, because an absolute path OpenCode resolved is one this
server cannot place as `user` or `project` without guessing. Failing to read them
costs the skills and not the models — a provider refused over a menu would lose
the developer the thing they came for.

**OpenCode contributes no slash commands**, though it has them: `GET /command`
answers on an external instance and no CLI lists them at all, so a local instance
could not answer. One catalogue whose contents depend on how the instance was
reached is worse than one this server does not claim yet.

**Sub-provider** — the upstream a discovered model belongs to, carried on every
OpenCode model in the snapshot. OpenCode is a front for other providers, so one
instance offers `openai/gpt-5` beside `siliconflow/Qwen/Qwen3.5-27B`, and two
upstreams routinely name the same model identically — this is what tells those
rows apart, and what the model search matches when somebody types a provider's
id. The HTTP inventory's display name where it has one, the provider id
otherwise, which is all `models --verbose` reports. Authored fallbacks have
none: a developer's slug is theirs and naming an upstream for it would be a
guess.

**Visible** agents excludes three the CLI does not admit to hiding. `GET /agent`
carries a `hidden` flag; `agent list` prints `compaction (primary)` and says
nothing, so `compaction`, `summary` and `title` — OpenCode's own machinery — are
named as hidden in `crate::provider` the way upstream names them. Without that a
local instance offered its summariser as something to run a turn with.

**Text generation** — short, structured provider work used to name commits,
pull requests, branches and threads outside a conversation. OpenCode runs each
request in a temporary deny-all session; local instances share a separate
server that is reaped after thirty idle seconds.

**Handshake** — the driver's `initialize` exchange. For Claude it is the only
way to learn its compiled-in commands, so a session is opened for that question
and killed on the answer. For Codex it opens the app-server protocol, carries the
CLI version in `userAgent`, and is followed by the `initialized` notification.
laplus advertises empty Codex capabilities deliberately.

**Snapshot** — a description of the world that every event after it is a _diff
against_. Not an optimisation and not merely a first chunk: the client folds an
event only into state it already holds, so a subscription that opens without a
snapshot has its whole contents discarded on arrival, silently. Ticket 28 was
that, for a whole turn. The question to ask of any subscription here is never
"are the events right" but "does the client have anything to fold them into".

A snapshot reaches the client two ways, and ticket 31 is why the second exists.
A subscription **opens** with one unless the client is **caught up**, wrapped in
a `{"kind":"snapshot"}` envelope; `GET /api/orchestration/shell` and
`GET /api/orchestration/threads/{threadId}` answer with the **same object,
unwrapped**, which the client prefers because it compresses and stays off the
socket. Each is built once — `Shell::shell_snapshot`, `Threads::detail_snapshot`
— and both transports call the builder, because the client takes whichever
answer it gets first and two builders would let the world it draws depend on
which one that was.

**Resume** — a subscription from a client that says it already holds the
conversation, by sending `afterSequence`. The one case that is _not_ refused for
a thread this server does not have, because a client with its own copy can still
draw it and an empty snapshot would be a claim that copy is wrong.
`crate::threads::Watch`.

**Caught up** — a resume whose cursor is still the newest number this server has
handed out. The replay it asks for is then a replay of no events, which is the
one replay this server can perform: the subscription opens with no snapshot, and
the client is left holding what it already correctly held. The boot case, and
the whole of what an HTTP snapshot saves. `crate::store::Sequences::caught_up`.

Every other cursor is answered with the whole snapshot, because replaying from a
position needs a log of the events to replay and this server keeps none. Note
that a cursor _ahead_ of the newest number is not a client running early but one
holding a number from a previous run — this server's numbering resumes from its
last durable write, so a run reissues everything the run before it did not write
down. Both directions therefore mean the same thing, which is "I cannot tell you
what you missed; here is everything." See ADR-0016.

**Draft** — a conversation the client has made up and the server has never heard
of. Where every new conversation starts: the composer mints the id, and the
thread reaches this server only when the first turn is dispatched carrying
`bootstrap.createThread`. Not a state this server stores — it is precisely the
absence of one, which is why a subscription to a draft is refused rather than
opened. The opposite of a **resume**: a draft is a client with an id and nothing
else, a resume is a client with the conversation and no need of us to prove it
exists.

**Subagent** — a delegated child agent, running its own work on the parent
conversation's behalf. Each provider spells one differently — OpenCode runs one
as a session with a `parentID`, Claude as a background task, Codex as a
collaboration thread — and laplus keeps one provider-neutral model of them.
`crate::subagents`.

**Subagent work stream** — one subagent's ordered, replayable conversation and
work. Not a progress log and not a result view: the child's prose, and its
terminal outcome as the last entry of the same stream, so the conclusion stays
attached to the work that produced it. Stored beside the parent transcript
rather than in it, fetched only when its surface opens, and removed with the
conversation that delegated it. `crate::subagents::Stream`.

**Compact child row** — the one work-log row a conversation keeps per subagent:
identity, assignment, state, latest meaningful activity or terminal preview, and
the `childId` its work stream is addressed by. It is the index and the launcher;
the work itself is never duplicated into the parent transcript.
`crate::worklog::subagent`.

**Stream head** — a subagent work stream without its work: identity, parent
identity, name, assignment, state and outcome. Exactly the part that travels
alone — it is the durable row, and it is what crosses the wire on every update —
so a child that says one more thing does not cost a copy of everything it has
already said. `crate::subagents::Head`.

**Stream entry** — one thing that happened in a subagent's session, with a
stable id, a position, a kind and a payload. Upserted by id and ordered by
position, which is what lets a client's replay and its live continuation meet
without losing or repeating one — and what lets a tool call that runs and then
finishes be one entry that moves rather than two that repeat.
`crate::subagents::Entry`.

**Entry kind** — what a stream entry _is_, in one vocabulary all three providers
fill: the child's prose, a command, a read or search, an edit, another tool call,
a warning or error, a blocker, a **nested launcher**, and the terminal outcome.
The kind decides the payload and the row the client draws, so a child's command
reads as a command and its edit as a file change. A provider tool laplus cannot
place is a generic tool call rather than a guess at which specific kind it
resembles. `crate::subagents::EntryKind`.

**Nested launcher** — a subagent's compact row inside the stream of the subagent
that delegated it. The same five things the parent conversation's **compact child
row** carries — identity, name, assignment, state and outcome — in the one other
place a launcher may appear, and it opens the descendant as another ordinary
right-panel tab. `crate::subagents::Launcher`.

**Drawn only where the provider proves the relationship, and drawn there
_instead_ of in the root transcript.** It exists because **stream head**'s
`parent_child_id` is set, and that field is filled from evidence alone, so a
hierarchy laplus cannot prove produces no nested launcher and nothing invented to
stand in for one — a descendant announced on the root thread is placed by what
the provider proved about it rather than by where laplus first heard of it. One
worker has one visible parent.

**Delegation tree** — every subagent a conversation has delegated, at every
generation. Not a shape this server stores: a child stream is held per child and
the tree is what they say about each other, so "the whole tree" is a question
answered by reading them all (`crate::subagents::Streams::active`). It is the
subject of two rules.

**The conversation is working while its tree is.** A quiet or settled root does
not make the thread idle while a descendant is pending, running or blocked — the
sidebar draws _Working_ from a session whose status is `running`
(`Sidebar.logic.ts`, `resolveThreadStatusPill`), so
`crate::threads::Threads::follow_delegation` is what keeps that claim true of the
tree rather than only of the root. It **reconciles rather than defers**: the
root's turn settles when it settled before, carrying the state it truthfully
reached, and the session republished afterwards names no turn. The conversation
leaves Working when the last descendant is terminal.

**Stopping the parent stops the tree.** Every child that had not already reported
records an interruption — its terminal state and the last entry of its stream —
and takes no ordinary live work afterwards. Recorded on the developer's own
command rather than waited for, because no provider reports back that a subagent
it was running has been abandoned. **Closing a child tab is not this**: a closed
surface asks the server for nothing at all, which is the whole of what keeps the
common workspace gesture safe.

**Child work** — one piece of what a subagent did, carried in the _client's_
work-log vocabulary rather than any provider's: a title, a lifecycle status, and
whatever the protocol exposed of the command, the files, the pattern and the
output. Translated once, in the adapter, so the child's surface can reuse the
main agent's work rows unchanged. `crate::subagents::Work`.

**Blocker** — a permission or question a subagent stopped for. It lives in three
places at once and deliberately: the child's stream records that it waited and
what it was told, the main conversation gets the ordinary actionable request row
naming the child, and the provider's own request id is what routes the answer
back to the descendant. Asking and answering are one entry under one key, so a
child's history explains the pause rather than leaving two rows to be paired.
`crate::subagents::Blocker`, `crate::approval::Waiting`.

**Resolution** — how a blocker ended, as an identity rather than a sentence:
approved, approved for the session, declined, cancelled, answered, rejected, or
**undelivered** — the developer decided and the decision could not be sent. The
last is why this is not simply the developer's decision: a child that was never
told stays blocked, and its stream has to be able to say so while the
conversation records both the decision and the failure to deliver it.
`crate::subagents::Resolution`.

**Child outcome** — how a subagent's work ended: a result, a failure, an
interruption, or **empty** — completed with nothing to return. Four rather than
three, because a child that finished and said nothing has concluded, and
presenting the last thing it happened to say as its answer would be a guess.
`crate::subagents::Outcome`.

**Latest meaningful activity** — the one line a compact child row shows while its
subagent runs: the most recent thing the child did that tells the developer
something. Announced-but-empty tool parts, blank text parts and status
heartbeats are not it. When the child becomes terminal this is replaced
atomically by a bounded preview of what came back — including for a child that
returned nothing, which says so rather than reverting to what it was doing.
`crate::worklog::subagent`, from the `said` field of
`crate::protocol::SubagentTask`.

**Subagent moved** — one provider line's news about one subagent, carrying both
of the places a child is shown: the compact row's line, when that line tells the
row anything, and the child's identity, assignment and own work for its stream.
One value rather than two answers to two questions, because a row and a stream
that disagreed about whether a child had finished would be two truths about one
child. A line can move either or both — a forwarded message carrying only a tool
call is the child's whole work and nothing the row can say in its one line.
`crate::protocol::SubagentMoved`, `crate::protocol::SubagentDid`.

## Lifecycle

**Session status** — what the agent process is doing. The contract's seven:
`idle`, `starting`, `running`, `ready`, `interrupted`, `stopped`, `error`.
`crate::settling::SessionStatus`.

**`running` is "there is work here", not "there is a turn here".** It usually
names one, on `activeTurnId` — but a conversation whose background subagent
outlived the turn that launched it reports `running` with no turn at all, which
is the whole of how the sidebar keeps calling an active **delegation tree**
working. The client draws its badge from this field alone (`Sidebar.logic.ts`,
`resolveThreadStatusPill`), so there is nowhere else that claim could be made.

**Turn state** — how the most recent turn went. The contract's four: `running`,
`completed`, `interrupted`, `error`. `crate::settling::TurnState`.

**Runtime mode** — how much latitude the agent is given, as the composer's
picker offers it. The contract's four: `approval-required`, `auto-accept-edits`,
`auto`, `full-access`. `crate::orchestration::RUNTIME_MODES` is the closed set
**every** door is checked against — the picker's own command, a turn's per-turn
override, and the two that create a thread — because a literal the contract does
not name fails the client's decode of the whole conversation rather than drawing
a wrong badge. `crate::agent::permission_mode_for` is the separate question of
which `--permission-mode` each one becomes, and answers nothing for
`approval-required` because upstream expresses that by passing no flag;
`crate::agent::pushed_permission_mode_for` answers the same question for a
running child, where the omission cannot stand and `approval-required` becomes
`default`.

A historical stored value outside the closed set is rounded to `full-access` on
read without migrating the database row.

Given to the agent **at launch and again at every turn whose mode has moved** —
`crate::session::retune` pushes it through the driver before the turn is
written, and the session's own copy moves with it, so every session event for one
turn reports the same mode. Ticket 11 of `.scratch/thread-lifecycle/` is the
whole of it, and `fixtures/claude-cli/20-modes-changed-mid-conversation.ndjson`
is a real child being moved. The **model** is the same mechanism through
`set_model`, and had the identical hole.

**Interaction mode** — whether the developer is planning or acting: the
contract's `default` and `plan`. Carried on the thread, published, and **never
sent to the CLI** — nothing in this server reads one, so it is a value the client
keeps here rather than a behaviour this server has. The closed set is
`crate::orchestration::INTERACTION_MODES`.

A historical stored value outside the closed set is rounded to `default` on
read without migrating the database row.

Both are **per-thread and editable**, by a command each, and both also arrive as
a per-turn override on a turn request. Absent on a turn means unchanged; the
command is how a picker moves one between turns.

**Settling** — reading a session status as a turn state. Leaving `running` is
the end of a turn, not the last assistant message, which is what makes a turn's
duration cover the whole turn. Upstream's word (`decider.settled.test.ts`,
`threadSettled.test.ts`), kept.

**The converse is not true: entering `running` is not the start of a turn.** A
conversation still working on its **delegation tree** re-enters it naming no
turn, which settles nothing and starts nothing — the client's reducer folds it
against a turn it has already settled and leaves that turn alone. So a reader
counting turns must count the sessions that _name_ one, not the transitions.

Note the two are not opposites of each other. `interrupted` and `stopped` are
different _statuses_ — the developer asked, versus the process went away — and
both settle a turn as `interrupted`, because from the turn's point of view they
are the same thing: it did not finish.

**Not inbox state**, below, despite the contract spelling two of that concept's
fields `settledOverride` and `settledAt` and two of its commands `thread.settle`
and `thread.unsettle`. Settling is a property of a **turn**; inbox state is a
property of a **thread**. This entry has seniority — it is the meaning
`crate::settling` owns and the one the code already uses — so the prose and the
Rust identifiers disambiguate, and the field names, which belong to the contract,
do not move. `docs/adr/0024` is the decision and what was weighed against it.

**Inbox state** — whether a conversation belongs in the developer's working
list, and until when. The contract's six fields on a thread: `archivedAt`,
`settledOverride`, `settledAt`, `snoozedUntil`, `snoozedAt` and `deletedAt`,
carried on both renderings of a thread as one shape,
`crate::threads::Lifecycle`.

**Stored here, classified in the client.** This server keeps the six;
`effectiveSettled`, `effectiveSnoozed`, `threadRaisedHandWhileSnoozed`,
`canSettle` and `canSnooze` all already exist in `@t3tools/client-runtime`, with
their own suite, and are what the developer actually sees. Reimplementing them
here would be a fourth copy of a rule this repository already keeps three of.
Two consequences worth naming: a snooze expires by being **read** rather than by
a timer, so there is nothing to schedule; and a thread raising its hand while
snoozed does not clear the fields, it only stops them classifying.

**Shelf** — which of the developer's two lists a conversation is on.
`crate::threads::Shelf`, and the first of the six fields to become something a
developer can move: `thread.archive` stamps `archivedAt`, `thread.unarchive`
clears it, and the stamp is the whole of the difference between the two lists. A
**deleted** conversation is on neither shelf, which is not symmetry with archive
but what the client's own reducer needs — see **Deleting is soft**.

**Archiving is not deleting.** The thread, its transcript, its work log and its
checkpoints all stay exactly as they were, and a running agent is not told
anything. What changes is which snapshot names the conversation: the project list
(`orchestration.subscribeShell` and `GET /api/orchestration/shell`) carries the
working shelf, and `orchestration.getArchivedShellSnapshot` carries the other —
**one builder, filtered two ways**, because two would let the world a client
draws depend on which of them answered. The archived answer carries the whole
project registry rather than a filtered one, because the settings panel that
reads it groups the threads by project and looks each one up there.

Both commands **refuse a repeat**, unlike the renames and the mode pickers: this
is a move between two lists rather than a write of a value the developer chose,
so a second archive is a click on a control that is no longer there. The refusal
is decided under the fold's own lock — `crate::threads::Threads::apply_unless`,
which exists for this.

**Settling a conversation** — the developer's standing answer to "am I finished
with this?", overriding what the client would otherwise derive.
`thread.settle` writes `settledOverride: "settled"` and stamps `settledAt`;
`thread.unsettle` clears the stamp and writes `settledOverride: "active"`.
`crate::orchestration::Shell::settle` and its twin. **Not `crate::settling`** —
see the entry above and `docs/adr/0024`.

**The two directions are not symmetrical.** A settle takes a conversation out of
the inbox. A **user** unsettle _pins_ it active, so the client's own auto-settle
stays suppressed until real work moves it on — it does not return the
conversation to no override at all. That neutral reset is the server's own,
carried on the same event with `reason: "activity"`, and the contract lets a
command send only `user` so it cannot be forged — see **Lifecycle reset** below.

**What may be settled is enforced here; what counts as settled is not.**
`effectiveSettled` reads these two fields alongside four other things and lives
in the bundled client runtime — see **Inbox state**. The _invariants_ are this
server's, because the client's copy of them exists to avoid a round trip:
`crate::threads::Busy` is `canSettle` mirrored, and refuses a conversation with
an unanswered approval or question, one whose session is `starting` or `running`,
and one whose turn was asked for and not yet adopted. An archived conversation is
refused by both commands, in the same reading — it is not in the inbox to leave
and there is no inbox to pin it back into.

**Snoozing a conversation** — putting it out of sight until a time the developer
chose. `thread.snooze` stores the wake time they picked as `snoozedUntil` and
stamps `snoozedAt`; `thread.unsnooze` clears both.
`crate::orchestration::Shell::snooze` and its twin.

**An overlay, not a destination.** A snoozed conversation stays active in this
data model — not archived, not settled, not deleted — which is why snooze does
not sit in the same vocabulary slot as **Shelf**. And it **never touches the
agent**: a running session is snoozable, because snooze is a decision about the
developer's attention rather than an interruption of the work. That is the one
thing separating it from `thread.session.stop`.

**There is no scheduler**, and this is the thing to know before reading the code
looking for one. A snooze expires by being **read** — once the wake time is past,
`effectiveSnoozed` stops classifying and no event fires — and a raised hand stops
a conversation classifying as snoozed without spending the fields. Both
derivations ship in the client. So the wake time is a _fact about the
conversation_: it survives a restart with nothing to re-register, and nothing
here has a timer to cancel.

**What may be snoozed is `canSnooze`**, which is `canSettle` minus the live
session — `crate::threads::Attention` is the whole of that difference, and it
skips the session _check_ rather than filtering the answer, because a
conversation can be working and holding an unadopted turn at once. What is
refused is what a snooze would hide: an unanswered approval or question, and a
turn no agent has picked up. Archived is refused by both commands for the settle
pair's reason.

**A wake time is judged, not stored blindly.** It must be one this server can
place on its own clock (`crate::clock::epoch_millis_from_iso`, the inverse of the
renderer, strict about both the shape and the calendar) and **strictly ahead of
now** — a conversation snoozed until a moment that has passed would be snoozed
and awake at once, carrying state it can never leave. Refused rather than
normalised, and stored exactly as the client sent it.

One guard, two sentences: a string this server cannot place on a clock is not one
it can call future either, so it takes the same branch — but "that moment has
passed" is a lie about a time this server simply does not read, and the sentence
_is_ the whole diagnostic. `crate::orchestration::Unusable` is the distinction.
The comparison itself is `wake_time`, which takes the instant to compare against
rather than reading the clock, because the boundary that makes "strictly future"
mean anything cannot be reached from a socket — a client samples its clock,
sends, and this server reads its own afterwards.

**A repeat is keyed on the wake time.** Snoozing to the moment a conversation is
already asleep until re-emits without churn; choosing a _different_ time is a new
decision and restamps both fields. That second half is not tidiness — the client
measures a raised hand against `snoozedAt`, so a new snooze carrying an old stamp
would be woken at once by the work the developer had just decided to sleep
through.

**Lifecycle reset** — a conversation returning to the inbox on its own, because
there is real work in it again. The spec's own phrase.
`crate::threads::Change::wakes` decides which resets a change spends,
`crate::threads::Woken` is what each one is and what stands in its way, and
`crate::threads::Threads::woken_by` emits them.

**There are two resets and they do not share triggers**, which is why `wakes`
answers a list rather than a `bool`:

| Reset              | Spent by                                                                  | Event                                  |
| ------------------ | ------------------------------------------------------------------------- | -------------------------------------- |
| the inbox override | a turn request, a working session, a request that blocks on the developer | `thread.unsettled(reason: "activity")` |
| the snooze         | a turn request, and nothing else                                          | `thread.unsnoozed(reason: "activity")` |

**Only the developer re-engaging spends a snooze.** A session starting or failing
does not: the snooze never paused the agent, so work happening is not the
developer changing their mind — and a raised hand already stops the conversation
classifying without spending it, so clearing the fields there would cost them the
rest of their snooze the moment they dismissed the request. Sending a new message
is the one act that says they are back.

**Leaving the inbox must never hide something that needs the developer.** The
invariants above refuse to create that state when the developer asks, and this is
what stops it being reachable a minute later: a conversation settled while quiet
whose agent then asks for permission would otherwise sit outside the inbox while
blocked on a decision only the developer can make. It resets an override in
_either_ direction — a conversation pinned active returns to neutral too, so it
can settle itself again once the burst of work goes stale.

**An archived conversation is not woken, either way.** `Shelf::holds` is asked by
both guards as well as by all four commands, so the filter and the rule stay one
rule: there is no inbox to return an archived conversation to, and clearing state
the commands themselves refuse to touch would lose the developer's decision the
moment they unarchived it. **Nor is a deleted one**, on the same reading, and that
answer is reachable rather than theoretical: deleting does not stop a session, so
an agent winding down behind a deleted conversation goes on producing all three
triggers, and a reset would move a conversation the developer can no longer see. Live work is still never hidden, because the client's
`effectiveSettled` checks its activity blockers _before_ any override.

The inbox reset's three triggers, two of them narrow on purpose — the snooze
reset takes only the first:

| When                                                  | Why it is that narrow                                                                                                                                                                                                                                                                                                                                                                                                                       |
| ----------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| a turn is requested                                   | not narrow at all: the developer typed something and pressed enter                                                                                                                                                                                                                                                                                                                                                                          |
| the session becomes `starting` or `running`           | `ready`, `stopped` and `error` are a status arriving _after_ the fact and must not fight an explicit settle. `SessionStatus::is_working` is the same reading `Busy` refuses a settle with, deliberately. A **delegation tree** that outlives its turn re-enters `running` through this same row, so a conversation settled while quiet returns to the inbox when its own child is still working — which is the rule, not an exception to it |
| an approval or a question is appended to the work log | `crate::worklog::blocks_on_the_developer` — any work-log row would wake a settled conversation on every tool call of every turn                                                                                                                                                                                                                                                                                                             |

Every trigger is a path this server already owned, so a reset is a guarded
emission _beside_ an event that already fires rather than a new mechanism. The
guards are `Thread::wants_waking` and `Thread::wants_unsnoozing`, and both travel
through the refusal `Threads::commit` already takes for the archive commands — so
each is asked under the lock the fold runs under and before a sequence is taken.
Without them a reset with nothing behind it would land in
`Change::re_emitted_at` as a repeat and put a no-op event on the feed at a stale
`updatedAt`, reordering a list the developer had not changed anything in. Both
guards read `Shelf::holds` as well, so an archived conversation keeps both its
inbox state and its snooze through any amount of work — which is one rule with
the commands, since all four refuse an archived conversation.

**One command, several events.** The resets are published after the change that
caused them, and the dispatch answers with the last of the sequences it committed
— the shape a turn request already had when it committed three. A message in a
conversation the developer had both settled and snoozed spends both, as two
events: a client folding one and not the other would hold half a conversation's
lifecycle.

**Adoption grace** — how long a user message with no turn behind it is still a
turn about to start rather than stale data. Two minutes either side of now,
mirrored from the client, and held as two _rendered_ stamps
(`crate::threads::Adoption`): every timestamp on this wire is one fixed-width UTC
shape, so it orders lexicographically and the window needs no calendar.

**Idempotence by re-emission** — a repeat of any of the four inbox-state commands
is answered rather than refused, and this is where they part company with the
archive pair above. Each is a standing answer rather than a move between two
lists, so folding the event again lands on the same state. A re-emission carries
the conversation's _existing_ `updatedAt` — and, through the fold, its existing
`settledAt` or `snoozedAt` — rather than the current time, so a double-click
neither rewinds the conversation nor moves it in a list ordered by when things
changed. `crate::threads::Change::re_emitted_at`, and it is the one change in
this crate that does not stamp the clock. What counts as a repeat is per command:
a settle asks about the override, a snooze about the wake time.

**The controls are gated on capabilities.** `capabilities.threadSettlement` and
`capabilities.threadSnooze` on `server.getConfig`: `useThreadActions.ts` refuses
to dispatch a command to a server that does not advertise its flag, and the
sidebar and chat view hide the menu items outright. Answering the commands
without advertising the flag would be commands nothing sends. `threadSnooze` also
draws the sidebar's snoozed section and its "Woke" indicator, both from
derivations that ship in the client.

The same flag also lets the client's **inactivity auto-settle** classify at all
(`SidebarV2.tsx`), so advertising it does more than reveal two menu items: a
conversation nobody has touched for `autoSettleAfterDays` now leaves the inbox by
itself. That derivation is the client's and ships unmodified, and its premise —
that the server un-settles on real activity — is what **Lifecycle reset** above
is: an auto-settled conversation comes back the moment there is work in it again,
rather than staying gone until the developer goes looking.

**Deleting is soft** — `deletedAt` is a stamp, not a `DELETE`. `thread.delete`
writes it and moves nothing else; the row, its transcript, its work log and its
checkpoints all stay. Three reasons, and none of them is squeamishness: a hard
delete would orphan the git refs the turns wrote, the threads table cascades so
removing the row would take the transcript with it irreversibly, and the contract
carries a deletion time on the thread that is only meaningful if the thread
survives to carry it. `crate::orchestration::Shell::delete`.

**What the developer sees is a conversation that is gone**, and that is four
separate withholdings rather than one:

- **Both lists.** `Shelf::holds` answers `false` for a deleted conversation on
  either shelf. The archived half is the one that had to be _checked_ rather than
  assumed — the settings panel takes that snapshot whole and groups it by project,
  filtering on neither `archivedAt` nor `deletedAt`, so a conversation archived and
  then deleted would be drawn there with an unarchive control on it.
- **The project list's feed.** The change is published as a `thread-removed`
  rather than as a summary, because `OrchestrationThreadShell` does not declare
  `deletedAt` at all and a client therefore cannot filter one out the way it
  filters an archived conversation. Everything _after_ a deletion is withheld too:
  an agent may still be running behind one, and its next `thread.session-set`
  would otherwise upsert the conversation straight back onto the list.
  `crate::threads::Change::on_the_list` is the whole of that rule.
- **Every later command.** Asked once in `Shell::dispatch`
  (`Command::over_a_living_thread`) rather than in nineteen arms, so a stale
  window cannot go on driving a conversation the developer removed.
  `thread.delete` itself is the deliberate exception: whether a conversation is
  _already_ deleted is a question about the field the change is about to move, so
  it is answered under the fold's own lock and a repeat is refused there.
- **A fresh read.** The thread subscription and `GET
/api/orchestration/threads/{threadId}` both refuse one. Both, because the client
  seeds a pane from the route and then subscribes _with a cursor_ because it now
  holds the conversation — a route that answered would leave a window drawing a
  conversation it could never be told was deleted.

**A resume is the exception, and it is ticket 28's rule unchanged.** A client that
says it already holds the conversation is owed the `thread.deleted` it has not
folded yet, and is handed a snapshot stamped `deletedAt` rather than a refusal.
Which is also the only door left for reading what the deletion kept.

**Nothing tells the agent.** A session still running behind a deleted
conversation writes to a transcript nobody is watching until it ends by itself;
ending it is `thread.session.stop`'s job. The upstream client sends that command
_before_ this one, which is where the sequencing belongs.

**Ending** — how a turn ended, as the driver knows it: completed, failed, or
stopped. Distinct from turn state because the CLI reports a stopped turn as a
failed one, so only this server's own knowledge that it asked can tell them
apart. `crate::turn::Ending`.

**Session stop** — ending the agent process behind a conversation, keeping the
conversation. `thread.session.stop`, and **not an interrupt**: an interrupt asks
a running turn to stop and leaves the child alive, this ends the session, and the
case it exists for — an agent that is idle or wedged, holding a process — has no
turn to interrupt. `crate::threads::Threads::stop_session`.

Answered in two stages, like an interrupt. The slot is freed and the driver told
at once, so the next turn starts a _new_ session and the developer's click is
what stops the conversation being drawn as alive; the driver leaves its loop,
closes the agent's stdin, waits, and kills if waiting was not enough, and only
then publishes the session as `stopped`. Nothing else moves: the transcript, the
work log and above all the **provider resume cursor** survive, which is how the
next turn continues the same conversation.

**Session epoch** — which of a conversation's sessions a driver is. Counted per
thread, and the reason it exists is that a stop frees the slot while the child is
still being reaped: a driver can outlive its own session, and must not publish an
ending over the session that replaced it. `crate::threads::Live::epoch`.

## Protocol

**Drift counter** — a tally of agent-protocol events this build did not
recognise. Unknown variants increment it instead of failing, so a CLI upgrade is
learned from a number rather than a bug report. Two of them, because they are two
failures: an unrecognised event type and a line that is not JSON at all.
`crate::protocol::Drift`, which is subtractable — a turn reports its own, the
session reports its total. Both drivers use that same tally: Claude folds it in
`crate::protocol::SessionState`, and Codex in
`crate::codex_protocol::ConversationState`. Parsed Codex JSON whose method, item
kind, or claimed shape this build cannot read is an unrecognised event; only a
line that cannot be parsed is an unreadable line.

**Compaction** — the agent summarising its own conversation to make room, and
carrying on. A fact about what the _agent_ can still see; the transcript is this
server's own copy and is untouched by one. `crate::protocol::Compaction`.

**Standing** — how the developer's account is placed against its usage limits,
as the CLI reports it: allowed, close to the limit, or refused.
`crate::protocol::RateLimit`. Agent-protocol vocabulary with no contract
equivalent, so it reaches the developer as an activity rather than as a field.

**Usage report** — the historical provider consumption attributed to calendar
days, models and provider kinds. It is distinct from a conversation's context
window and from the account's current **standing**.
_Avoid_: Usage, when the distinction is not already clear.

**Reconciliation** — assistant text arrives twice, as deltas and again as a
buffered message. The deltas drive live rendering; the buffered message is
authoritative and replaces the accumulation. Whether the two agreed is recorded.
An interrupted Codex turn is the deliberate exception: app-server sends no
completed message after the interrupt, so the accumulated deltas are closed with
empty authoritative text and kept exactly as rendered. No reconciliation is
recorded because there was no second copy to compare.

**Join** — a place where the agent protocol and the contract meet. `crate::turn`
is the declared one, and it is one **driver**'s: the join is per-agent, while the
session lifetime around it (`crate::session`) is not. `crate::worklog` is a
second.

The declared one is crossed in two halves and each is checked on its own.
`crate::protocol` takes a line and answers with a **`Folded`** — what that line
was, in this server's words — and `tests/protocol_golden.rs` pins that against 19
captured sessions. `crate::turn::decide` takes the `Folded` and answers with a
**`Decided`**: the changes the conversation is owed, a provider resume cursor if
the driver learned one, and how the turn ended if it did — which is also the
vocabulary every driver answers a session in. `crate::session::spend` applies it.
Deciding and applying are two functions for the reason the fold and `commit` are
(ADR-0025): a function that applies its own results can only be tested against a
live world, and this one's world is a running `claude`. See ADR-0027.

**Refusal** — how a method this server has not implemented is declined: a typed
error carrying a tag the _called method's own_ contract union declares, and a
sentence naming what was refused. `crate::refusals`. A contract term borrowed
rather than invented — the tag says "authorization" where the truth is
"unimplemented", because no tag in the contract means the latter; see ADR-0017.
Distinct from a **declined setting** (ADR-0009), which is a value refused on the
way in rather than a method refused on the way out.

**Capability** — a flag on `server.getConfig` saying what this server can do, and
the gate on the control that uses it. `environment.capabilities`. Absent is the
safe answer and the client reads it as unsupported, which is what makes version
skew survivable in one direction.

The pairing is load-bearing in **both** directions, and each direction has already
cost something:

- **Answered but not advertised** is a command nothing sends.
  `useThreadActions.ts` refuses to dispatch a settle or a snooze to a server that
  does not advertise `threadSettlement` or `threadSnooze`, and the sidebar hides
  the menu items outright — so the whole thread-lifecycle effort is reachable only
  because `crate::config` sets both.
- **Advertised but not answered** is worse. `connectionProbe` makes `session.ts`
  probe with `server.probe` rather than `server.getConfig`, and that method's
  refusal tag is the one `session.ts` turns into `ConnectionBlockedError` — a
  connection refused on _permission_, and not retried. Advertising it before
  implementing it blocks every connection.

So a capability moves in the same commit as the thing it gates.
`serverSelfUpdate` is the one that is not a boolean; see **Self-update path**.

## Working tree

**Working tree status** — what has changed in a project's folder, as the UI
reads it: the branch, the changed files with their line counts, and how the
branch stands against its tracking ref. `crate::git`. The contract calls the two
halves **local** and **remote**; here they are read together, because neither
costs a network.

**Read** — running git and turning what it says into a status. The unit of work
this subsystem does; everything else is about when to do one.

**Stale** — a working tree that has changed since the last read started. What a
file change produces; a read is what clears it. The distinction is load-bearing
— see ADR-0006.

**Coalescing window** — the pause before each read, in which a burst of changes
becomes one read.

**Kept** — a working tree the server is holding a status for and watching.
`crate::git::Repositories`. Bounded at the same number as watched workspaces,
because a status that cannot be watched cannot stay true.

**Disturb** — telling a kept working tree it is stale because _this server_
just changed it, rather than waiting for the watcher to notice. What a switch,
an init and a worktree removal do. The same door a file change comes through,
opened from the inside; see ADR-0006 for why it marks rather than reads.

## Review

**Checkpoint** — what a project's working tree looked like at one turn boundary,
kept as a parentless commit under a ref of the project's own repository.
`crate::checkpoints`. The contract's word, and the thing that makes a turn a
point in time a diff can run to.

**Baseline** — the checkpoint a turn is diffed _from_: the one taken before the
prompt reached the agent. Turn one's baseline is turn count zero; every later
turn's baseline is the checkpoint the turn before it ended with, which is what
makes a conversation's checkpoints a chain rather than a set of pairs.

**Turn count** — how many turns of a conversation have been recorded. Also the
name of the checkpoint that recorded the last of them, so a turn's diff runs
from `n - 1` to `n` and a whole conversation's from `0` to `n`.

**Turn diff** and **thread diff** — one step in isolation, and the session as one
coherent change. Two methods over one range: a thread diff is a turn diff whose
`fromTurnCount` is zero.

A checkpoint is a _photograph_, not a record of authorship — see ADR-0008. It
does not know who changed a file, so an edit the developer made by hand between
two turns belongs to the turn it happened during, beside the agent's own.

**Revert** — putting the working tree back to a checkpoint. The restore of a
photograph this server already took, and therefore three git commands rather
than new machinery: a scratch index seeded from the checkpoint, the project's
own folder restated into it from the tree as it is now, then `read-tree -m -u`
back to the checkpoint. Files the turn created go, files it deleted come back,
files it modified and files it left untracked return to what was recorded — and
nothing above the project's folder is touched, whatever `HEAD` has done since.
`crate::checkpoints::restore`.

A revert moves the working tree and not the conversation: the transcript, the
work log and the list of turns are all left as they were. The client's own
reducer is more eager and trims them, so a window that watched a revert shows a
shorter conversation than one that reloads afterwards — deliberate, and recorded
in `.scratch/thread-lifecycle/issues/05-…`.

It is answered in two stages. `thread.checkpoint.revert` answers with a sequence
the moment it is accepted, and `thread.reverted` follows once the tree has
actually been written, because the socket's only reader must never wait on a
disk. A restore that fails says so in the work log as `revert.failed` and never
publishes a completion, so a failed revert cannot be read as a finished one.

**Checkpoint status** — how the turn a checkpoint records _went_, not whether
recording it worked. The contract's three (`ready`, `missing`, `error`), of which
this server sends two: the client reads the status back into the turn's state, so
a status that disagreed with how the turn ended would relabel it. There is none
that means interrupted, which is why a turn the developer stopped gets no
checkpoint. `crate::turn::Ending::checkpoint_status`.

## Refs

**Ref** — a branch, in the contract's word. The UI says `refName` everywhere
because a git branch and a jj bookmark are the same field to it, and this
server keeps the word rather than translating it back. `crate::refs`.

Not every ref is a branch: a **remote ref** (`origin/main`) is a record of
where a branch was on a remote, and is not something a working tree can be
_on_. Switching to one means making the local branch that tracks it.

**Current** — the ref this workspace has checked out. A property of a place,
not of a repository: the same branch is current in one worktree and merely
**checked out** in another, and only one of those can be switched away from.

**Default ref** — what a repository considers its trunk: the remote's recorded
`HEAD` if there is one, and otherwise whichever of `main` and `master` exists.
A convention where git has no answer, never a guess where it has none.

**Folded away** — dropped, of a remote ref that has a local branch of the same
name. `origin/main` beside `main` is a row that says nothing, so the picker does
not show one unless the client asks (`includeMatchingRemoteRefs`).

Not the conversation's **fold**, above, which is the older meaning and the one an
identifier carries — `crate::threads::fold`. This one is prose only: no function
here is called `fold`, and the contract's word for asking to keep these rows is
`includeMatchingRemoteRefs`, not a spelling of this one. Named as a participle
rather than a noun so the two read differently at a glance. See ADR-0025, and
ADR-0024 for the same disambiguation made between the two meanings of settling.

## Settings and keybindings

**Preferences** — the directory this server keeps the developer's own files in:
`settings.json`, `keybindings.json`, the logs and the registry. On
`ServerConfig` and deliberately off the wire; the paths that _are_ on the wire
are derived from it.

**Patch** — a settings change, every field optional, where an absent field means
**unchanged** rather than "set to the default". What `server.updateSettings`
takes, and also how a stored file is read: one is all-or-nothing so a refusal
changes nothing, the other is per-field so a key from another build costs only
itself. `crate::settings`.

**Rule** and **resolved keybinding** — the two forms of a binding. A rule is
what a person writes and what is in the file (`mod+shift+d`); a resolved
keybinding is what the UI consumes — the shortcut split into the flags a
`KeyboardEvent` carries, and the `when` expression parsed into a tree. Compiling
one into the other is `crate::keybindings`, because the client holds no file.

**Merge by command** — a custom rule replaces the default for the _same
command_, however either is spelled. What makes rebinding one shortcut leave the
other forty alone, and what makes removing one bring its default back rather
than leaving nothing.

**Issue** — something that went wrong assembling the configuration, shown in the
UI rather than logged. Its `kind` is one of two literals the contract names, not
a label: `ServerConfig.issues` is an array of a closed union, so an invented kind
fails the client's decode of the whole payload — which is why a settings problem
is logged instead, having no member of its own.

## Terminal

**Terminal** — one shell in a project's folder, named by the client and unique
within a thread. The client always chooses the name; the server never allocates
one. `crate::terminal`.

**Pane** — the terminal as the developer sees it: the emulator half, in the UI.
The server owns the pty half and the wire between them, and nothing else. Not a
type here; the word matters because it is what "resize the terminal" is a
consequence of.

**Scrollback** — the server's copy of what a terminal has shown, sent to a
client that attaches or that fell too far behind to catch up. Not the stream: it
is replayed into a live emulator, so the questions are taken out of it.
Bounded, at the same number the client's own buffer is bounded at.

**Question** — a control sequence that asks the emulator something rather than
telling it something: cursor position, device attributes, the colour queries. A
shell blocks on the first one it asks. Kept out of scrollback and remembered
instead, so that whoever attaches next is asked it. See ADR-0005.

**Attachment** — one `terminal.attach` subscription. A terminal outlives every
attachment to it, which is what makes reattaching a thing rather than a restart.

**Detaching** — not a call. Navigating away from a pane cancels its attachment
and touches nothing else, so the shell keeps working with nobody listening and
everything it says goes into the scrollback the next attachment is sent.

**Exited** and **closed** — two different endings, and the contract has an event
for each. A terminal whose shell exited is still on the list, still readable,
and can be given a new shell by name. A terminal that was _closed_ is gone: the
developer said so, the process was killed and waited for, and the id no longer
names anything. Reaping happens at the second, never the first.

## Remote access

**Cloudflare tunnel** — a public HTTPS endpoint whose traffic Cloudflare
forwards to a loopback laplus server. The endpoint may be temporary or stable;
the Cloudflare account, tunnel allocation and DNS record remain owned outside
laplus.

**Temporary tunnel** — a public Cloudflare tunnel belonging to one running
laplus process. Its generated hostname is disposable and is never restored as
a boot or service configuration.
_Avoid_: Quick Tunnel, except when naming Cloudflare's mechanism specifically.

**Laplus-managed connector** — a `cloudflared` connector started and supervised
by laplus. Management covers the local connector process, not ownership of or
authority to delete the Cloudflare tunnel or its DNS record.
_Avoid_: Managed tunnel

**Cloudflare account certificate** — the broad, long-lived `cert.pem` created
and owned by `cloudflared` login. Laplus may use it in place for an explicit
account-management action but never copies, replaces or deletes it.
_Avoid_: Tunnel credential

**Tunnel credential** — the narrow credential that can run one stable tunnel
and cannot manage the Cloudflare account. A laplus-created connector keeps this
credential in laplus's private data directory.
_Avoid_: Cloudflare token, account certificate

**Dedicated tunnel** — a stable Cloudflare tunnel reserved for one laplus
environment and either created by laplus or explicitly handed to it while
inactive. Laplus may configure and supervise it.
_Avoid_: Existing tunnel, which does not say who owns its configuration.

**Laplus-created tunnel** — a dedicated tunnel whose Cloudflare allocation and
DNS route laplus created. It is the only tunnel laplus may offer to delete from
Cloudflare; deletion is never implied by turning it off or forgetting it.

**Adopted tunnel** — an inactive existing tunnel explicitly handed to laplus as
a dedicated tunnel. Laplus may configure and supervise it but does not own its
Cloudflare allocation or DNS route and cannot delete either.

**Dedication** — the explicit confirmation that turns an inactive existing
tunnel into an adopted one, and the two mutations behind it: retrieving the
tunnel's run credential and writing laplus's own connector configuration. A
separate step because it is the first thing in the account path that changes
anything (ADR-0045), and journaled because it can be interrupted between the
two. `POST /api/access/cloudflare/account/adopt`, ADR-0050.
_Avoid_: Adoption for the screen that merely offers it — that is a choice, and
`adoptionConfirmed` is still false.

**Creation** — the explicit confirmation that makes a new stable tunnel for this
environment, and the three mutations behind it: allocating the tunnel with its
own private credential, routing a DNS name to it, and writing laplus's own
connector configuration. The only path that ends in a laplus-created tunnel, and
therefore the only one that ends in an ownership authorizing a Cloudflare
deletion. Journaled step by step because any of the three can be the last.
`POST /api/access/cloudflare/account/create`, ADR-0051.
_Avoid_: Provisioning, which says nothing about who ends up owning the result.

**Locally managed tunnel** — a tunnel whose ingress configuration lives on this
computer rather than at Cloudflare, which is what `cloudflared tunnel create`
produces and what laplus creates. Named on the screen that offers it because
Cloudflare recommends the remotely managed kind, and a developer choosing the
convenience of an in-app wizard should know which one they are getting.
_Avoid_: Managed tunnel, which reads as the opposite.

**Recorded DNS record** — the DNS resource laplus created, held on the endpoint
row as a name and, where laplus has them, a zone and record id. `cloudflared
tunnel route dns` reports no identifiers and the account certificate's contents
are never read, so creation records the name alone; whatever later acquires DNS
authority resolves and fills in the rest. A record with a name and no ids is one
laplus made and cannot yet address. `store::DnsRecord`, ADR-0051.

**Stop** — turning a laplus-managed connector off without changing anything
else. The tunnel, the DNS record, the run credential, laplus's own configuration
and the recorded ownership all survive, and starting it again is one action.
_Avoid_: Disconnect, which reads as something ending at Cloudflare.

**Forget** — removing laplus's own local configuration and run credential, after
stopping the connector using them, and then the endpoint record. Never a
Cloudflare change, for any ownership: an adopted tunnel is still allocated
afterwards and an external endpoint's connector is still somebody else's. It
removes no executable either — including the app-managed `cloudflared`, which
shares the private directory but is a tool rather than this exposure's setup. It
is also what releases the credential whose presence makes a creation refuse.
`POST /api/access/cloudflare/forget`, ADR-0052.
_Avoid_: Delete, which is the other operation entirely.

**Delete everywhere** — removing the exact Cloudflare tunnel and DNS record
laplus created, and then doing what Forget does. Offered only for a
laplus-created tunnel, refused by the server on the recorded ownership rather
than hidden by the client, and separately confirmed against the resources the
endpoint row names. Four journaled steps at three places, because the DNS record
is a Cloudflare API call the CLI cannot make.
`POST /api/access/cloudflare/account/delete`, ADR-0052.

**Deletion confirmation** — the one-time value laplus mints for one offered
deletion, naming the exact tunnel and DNS record it would remove. Spent when
read, expiring shortly, held in memory so a restart is a re-confirmation, and
checked against the endpoint row as it stands when the command runs. This is what
"fresh `access:write` authorization" means here: a scope says who may ask, and
only this says what they were shown. `POST /api/access/cloudflare/account/deletion`.
_Avoid_: Token, which is already three other things in this section.

**DNS authority** — a Cloudflare API token with DNS edit permission for the
hostname's zone, supplied for one destructive request and kept nowhere.
`cloudflared` has no `route dns delete`, and the account certificate is used in
place and never read (ADR-0045), so removing the record laplus created needs an
authority of its own. Missing authority refuses before anything is attempted
rather than deleting the tunnel and leaving the record.
`RefusalReason::DnsAuthorityRequired`, `crate::cloudflare_dns`.

**Cleanup state** — what a stop, forget or delete left behind: `intact`,
`stopped`, `cleanup-required`, `partially-deleted`, `forgotten` or
`fully-removed`. Derived from what is observably gone and from the mutation
journal, never stored — a third record of a fact the other two answer is the one
that can disagree with them after a crash. A finished removal is reported only
while no endpoint is recorded; outstanding work is reported either way, and an
endpoint whose local setup is being removed stops being advertised.
`crate::public_exposure::CleanupState`.

**Activation race** — a connector starting between the listing a dedication
offer was drawn from and the confirmation of it. Answered by re-reading the
tunnel's activity immediately before the first mutation and falling back to an
external tunnel endpoint, never by acting on the listing that produced the
screen. `RefusalReason::TunnelBecameActive`, ADR-0050.

**Run credential** — the narrow secret one connector runs on, whichever kind it
is: a connector token for a tunnel Cloudflare configures, or a tunnel credential
for one laplus configures. Named together because the rules are the same —
stored in a private file, passed by file and never by argument, redacted out of
logs and errors — and apart from ownership, which is the endpoint row's.
`crate::cloudflare_connector::RunCredential`.

**Connector state** — what a laplus-managed connector is actually doing:
`unconfigured`, `starting`, `ready`, `degraded`, `restart-exhausted`,
`stopping`, `stopped` or `failed`. The _actual_ state, as against the desired
one, and a local fact throughout — `ready` means the child answers `/ready` on
its loopback metrics address and says nothing about whether a public hostname
reaches it. The compact row's "Locally ready" and "Publicly verified" are both
this word plus the endpoint's verification, which is why they are two fields and
not one. `crate::cloudflare_connector::ConnectorState`.
_Avoid_: Connected, which claims the layer this cannot see.

**Settled connector** — one with no child of laplus's running, which four words
mean rather than one: `stopped`, `restart-exhausted`, `failed` and
`unconfigured`. What a stop, forget or delete waits for before it removes
anything: `restart-exhausted` and `failed` already carry `desiredState: stopped`
and never move again, and `unconfigured` has no configuration to carry a desired
state at all — so waiting for `stopped` alone waits for a word that will not
arrive. `crate::cloudflare_connector::ConnectorState::settled`.

**Redaction boundary** — where a run credential is taken out of what laplus
reports, which is the server and never the client: at capture as the connector's
output is read, and again as the snapshot is built. Substitution rather than
removal, so the sentence a developer needs survives with `[REDACTED]` where the
secret was. The secrets are learned while the credential file is certainly
readable and remembered thereafter — a redaction that re-read the file at log
time redacted nothing exactly when a cleanup had removed it.
`crate::cloudflare_connector` (its `Runtime::secrets` carries the reasoning),
`public_exposure::Refusal::redacting`, ADR-0053.
_Avoid_: Masking, which is a client-side affordance and not a boundary — a secret
that reached the browser has already leaked.

**Deletion verdict** — whether laplus may offer to delete a tunnel's Cloudflare
resources, stated by the server beside the ownership rather than derived by the
client. True only for a laplus-created tunnel, and the same answer the deletion
command refuses on, so an offer and a refusal cannot disagree.
`deletableAtCloudflare`, `TunnelOwnership::deletable_at_cloudflare`.

**External tunnel endpoint** — a hostname already routed to laplus by a tunnel
managed elsewhere. Laplus may verify and advertise the endpoint but does not
reconfigure, restart or delete its tunnel.
_Avoid_: Imported tunnel

**Verified tunnel endpoint** — a public HTTPS hostname recently proven to
identify this laplus environment and carry an authenticated HTTP request and
WebSocket upgrade. A ready connector alone is not a verified endpoint.
_Avoid_: Connected tunnel, when only the connector is known to be connected.

**Tunnel ownership** — who owns the Cloudflare tunnel behind an endpoint:
`external`, `adopted`, or `laplus-created`. Persisted, and the only thing that
authorizes deleting a Cloudflare resource. A tunnel run from a connector token
is external, because Cloudflare keeps its configuration and allocation.
`crate::public_exposure::TunnelOwnership`, ADR-0049.
_Avoid_: Ownership unqualified, which is also asked of a connector and of an
executable and has a different answer for each.

**Connector ownership** — who runs the connector process: laplus, or an external
supervisor. Independent of tunnel ownership — laplus may run a connector for a
tunnel it may not delete, and an adopted tunnel is laplus-run and externally
owned.

**Mutation journal** — what a multi-step Cloudflare change wrote down before and
after each step, so that a resume knows which mutations already happened and
which were started and never settled. The second list is the _remaining work_; a
recovery that cannot name it can only claim a rollback it did not perform.
`public_exposure_journal`.
_Avoid_: Setup state, which is one word for a thing that has to be a log.

**Public-exposure refusal** — a tagged answer to a refused Cloudflare command,
carrying a reason from a closed set, the sentence to show, and the mutations
completed and left outstanding. Distinct from a scope refusal, which discloses
only the required scope (ADR-0047) and is answered first.
_Avoid_: Error message, which is what this replaced and what could not carry a
reason a client could act on.

**Administrative session** — an authenticated laplus session granted
`access:write`. It may change who or what can reach the environment; an ordinary
paired client may use an endpoint without administering it.
_Avoid_: Local session, because a tunnel also arrives through a local process.

## Shell

**Shell** — laplus as a desktop application: a window, and the server
running inside the same process. `crates/laplus-shell`. The server is a
library to it, not a service it talks to, which is why closing the window is
what reaps the agents.

**laplus** — the product, and the repository: a fork of t3code holding both
halves of it. `apps/web` is the UI and the answer to "can we fix that in the
client?", which used to be no (ADR-0012); `server/` is this Rust workspace, and
was its own repository until ADR-0014. One name for all of it since the rename.

**lightcode** — what laplus was called until ticket 30 closed. Retired from the
live code entirely, and deliberately _not_ rewritten anywhere it is a record of
something: `fixtures/` are captures of real traffic and the paths inside them
happened, and `docs/adr/` are dated statements of what was decided at the time.
Editing either would make them claim to be evidence of things that never
occurred. (The `.scratch/` tickets were the third such record; they were deleted
on 2026-07-29 rather than rewritten.) So the mentions that survive do so on
purpose. If a mention is in code, configuration or living documentation, it is a
bug.

**Bundle** — the built web application, `apps/web/dist`, as it reaches the
executable: a table of names and bytes generated at build time. Source maps are
the one thing dropped, and they are two thirds of it.

**Upstream** — `github.com/pingdotgg/t3code`. Still the origin of every line of
the UI and of the contract, and still the thing laplus is measured against — but
**neither a checkout nor a remote**. `reference/t3code-server/` was deleted, and
there is no `upstream` remote to fetch from: `origin` is this repository and it is
the only one. Upstream is read over the network, in practice `gh api` against the
public repository, and it is evidence of an implementation rather than a
dependency. ADR-0018 is why there are no more syncs.

**Assets** — the bundle as the server holds it, and the rules for answering a
request from it. `crate::ui::Assets`. Empty for every server but the shell's,
which is what keeps a UI out of the test binaries and out of the plain server.

**Entry point** — `index.html`, and the answer to any path that looks like one
of the UI's own **routes** rather than a file. The UI routes in the browser, so
`/settings` is a path this server has never heard of and must not 404. A missing
_file_ still must.

**Product version** — the release identity shared by every shipped part of
laplus: the Rust server and shell, the web UI, the Tauri application, and the
npm launcher and platform packages. A prerelease suffix is part of this identity,
so an RC build reports the same complete value everywhere.

The contract field `environment.serverVersion` carries the product version. It
is not a separate version owned by the server or by whichever UI bundle happens
to be served.
_Avoid_: Server version, UI version, launcher version

**CLI** — the typed command interface owned by the `laplus-server` executable:
its commands, options, validation, help, version, output, and exit behavior.
Both direct execution and the npm entry point reach this same interface.

**Launcher** — the thin npm entry point named `laplus`. It selects the platform
server binary, supplies package metadata such as the bundled UI location, and
forwards the process streams, signals, and exit status. It defines no commands
of its own.
_Avoid_: CLI

**Origin** — scheme, host and port together, and the reason the port is fixed.
The window is pointed at `http://127.0.0.1:4773/`, so the port is part of what
the browser scopes `localStorage` by — and `localStorage` is where the UI keeps
the developer's layout, drafts and open thread. See ADR-0010.

It is also what makes a **remote** laplus a different server to a browser, and
not only to a person: the window's page is served by its own loopback server, so
every call it makes to a second laplus is **cross-origin** and is refused by the
browser unless the answer says otherwise. `http::browser_api_cors_headers` is
that answer, on the routes a remote client calls. **Origin is still not part of
any decision this server makes** — `auth::authorize` does not read it, and a
credential is the whole boundary. What the header settles is whether a page is
allowed to _read_ a refusal it already provoked.

**Install directory** — `%LOCALAPPDATA%\Programs\laplus`, where the
installer writes. Named separately from the **data directory** —
`%LOCALAPPDATA%\laplus`, `config::data_dir`, which holds `state.sqlite`,
`keybindings.json` and `logs/` — because until ticket 30 they were one
directory and nothing in either half said so. Moving the install is what
separated them; the data has not moved. See ADR-0013.

**Self-update path** — how a newer laplus replaces a running one, reduced to the
one word the client is told: `desktop-managed`, `boot-service` or `respawn`, or
absent for "you will relaunch this yourself". `capabilities.serverSelfUpdate`, and
note it is a **literal, not a flag** — ADR-0020's "stays false" describes a shape
the contract does not have.

The word names **who restarts the server**, because the server cannot restart the
thing that is dying:

- **`desktop-managed`** — the shell is supervising, so updating the application
  updates the server with it, and `server.updateServer` is never called. This is
  laplus in a window.
- **`boot-service`** — systemd is supervising. Point the unit at the new version
  and exit; systemd brings it back. ADR-0028 writes that unit.
- **`respawn`** — nothing is supervising. Launch a detached replacement and hand
  off before exiting. This is `npx laplus` in a terminal.

The distinction is not cosmetic: respawning while a supervisor is watching gives
two servers, which is why the marker has to be written into the unit rather than
inferred from `INVOCATION_ID`. See ADR-0031.

**Preview host** — whoever owns the webview a preview tab is drawn in, and it is
**not this server**. The client renders the page; `preview.reportStatus` is the
_client_ telling the server what that page is doing. The server keeps a registry
of tabs per thread — with a `revision` and a `serverEpoch` so a client can discard
a stale answer — and broadcasts changes.

Worth an entry because the opposite is the natural assumption, and it decides
where work lands: the Electron-versus-Tauri difference between upstream's shell
and ours falls on `apps/web` and `crates/laplus-shell`, not here. The one preview
method that does reach the shell is `previewAutomation.focusHost`, which wants a
verb on ADR-0021's named list. Contract vocabulary; unimplemented here — see
`.scratch/contract-parity/ledger.md`.

## Measuring the artifact

The vocabulary of `cargo xtask release`, which exists because the project has a
number to hit. Note that **bundle** is _not_ in this list: it means the web
bundle above and nothing else, which is why the command is `release`.

**Artifact** — what a developer ends up with. Three figures rather than one, and
they differ by more than rounding: the **installer** is what they download
(5.06 MB, LZMA-compressed), the **footprint** is what it leaves on their disk
(24.34 MB), and the **binary** is the application alone. The spec's "under
~30 MB" story is about the first.

**Footprint** — a directory, weighed. Measured two ways and it says which:
**installed** ran the real installer and weighed the **install directory** it
made, **payload** weighed the files the bundle ships without installing
anything. Only the first is the truth; the second is what runs when the machine
has not been volunteered.

**Target** — 20–30 MB, against upstream's 318 MB **baseline**. Only the ceiling
can be _missed_; coming in under the floor is the project working, not a fault,
which is why `size::Verdict` has three cases and not two.

**Breakdown** — a Rust source, split into total, comments, `#[cfg(test)]` unit
tests, blank, and what is left. What is left is **production code**, and it is
the figure set against the spec's ~20K **signal** — the total is three times
larger, mostly prose and this server's own tests, and reporting _it_ would trip
an alarm about scope creep that has not happened.

**Balanced** — a scan that ended where a Rust file is allowed to end: outside
every comment, literal and `#[cfg(test)]` region. Its own correctness check. An
unbalanced scan reports no line count at all, because the way this measurement
fails is silently, with a number that looks fine.
