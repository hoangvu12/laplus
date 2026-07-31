# A second driver, and it is Codex

Status: ready-for-agent

Evidence and provenance: `.scratch/codex-driver/upstream-research.md` for how
`pingdotgg/t3code` drives Codex, and `.scratch/codex-driver/spike-findings.md`
for what driving `codex app-server` **by hand** established — including four
places where the wire contradicts what upstream's source suggested. The
recordings behind the second file are in `captures/`. Where the two disagree,
the captures win.

## Problem Statement

laplus drives one agent. The model picker offers Claude models and nothing else,
because the server has one **driver** and a single constant serves as both its
driver slug and its instance id — every conversation publishes that constant as
the provider it ran under, because there is no other answer to give.

The developer's side of that is narrower than it looks. It is not "laplus lacks
a feature": it is that a developer who has paid for a Codex subscription, has
`codex` installed and authenticated, and wants to run a conversation in laplus
under it, cannot. They can see the shape of the missing thing — the bundled UI
already draws a provider settings form with a Codex section, already keys model
defaults off a `codex` driver slug, and already knows how to render a second
provider in the picker. All of it is client-side furniture for a server that
answers only one way.

The contract has been ready for this the whole time. `ProviderDriverKind` is a
deliberately open slug so a fork can add a driver. `CodexSettings` is declared
with a binary path, a `CODEX_HOME`, a shadow home and launch arguments. The
approval decision vocabulary — `accept`, `acceptForSession`, `decline`, `cancel`
— is Codex's own words, and the request kinds `command`, `file-read`,
`file-change` are Codex's three approval requests. The gap is entirely on the
server side, and it is the gap the parity ledger exists to count.

## Solution

Codex becomes a driver this server can run, and a conversation can be held under
it end to end: pick a Codex model in the composer, send a prompt, watch it
stream, answer the permission it asks before it escapes its sandbox, stop it
mid-turn, restart the server, and carry the same conversation on.

One Codex account, one instance. The **app-server** — Codex's own name for the
mode `codex` runs in, spoken to in JSON-RPC over its stdio — is started per
conversation, which is the shape laplus already uses for `claude` and the shape
upstream uses for Codex.

The second half of the solution is structural and has no user-visible face: the
one constant that means "the agent" becomes a registry of drivers, and the
session loop that runs a conversation stops knowing which agent it is running.
That work exists because Codex forces it, which is also how it arrived upstream
— their Codex driver's own header calls itself the first concrete driver in
their per-instance model.

## User Stories

1. As a developer with a Codex subscription, I want to select a Codex model in
   the composer, so that I can hold a conversation in laplus under the agent I
   already pay for.
2. As a developer, I want the Codex models offered to me to be the ones my
   account can actually use, so that I never pick a model the agent then
   rejects.
3. As a developer, I want each Codex model's reasoning efforts offered where the
   model supports them, so that I can ask for more thinking on a hard job and
   less on an easy one.
4. As a developer who has not logged into Codex, I want laplus to tell me that
   specifically, so that I go and run `codex login` rather than debugging an
   install that is fine.
5. As a developer with two agents installed, I want to see which OpenAI account
   Codex is running as, so that I know whose quota a turn is spending.
6. As a developer, I want a Codex conversation to stream its reply as it arrives,
   so that I can read along rather than waiting for a wall of text.
7. As a developer, I want the agent's reasoning to be visible as it happens, so
   that I can tell a stuck turn from a thinking one.
8. As a developer, I want a command the agent runs to appear in the work log with
   the command line and its exit status, so that I can see what was done to my
   machine.
9. As a developer, I want to be asked before the agent does something its sandbox
   would not allow, so that nothing writes to my working tree without my say-so.
10. As a developer answering that question, I want to be offered only the
    decisions this particular request allows, so that I am never shown a button
    whose answer the agent will refuse.
11. As a developer, I want the tool call to be on screen before I am asked to
    approve it, so that the question has context.
12. As a developer who declines a command where declining is offered, I want the
    turn to carry on, so that the agent can try another way.
13. As a developer who cancels, I want the turn to stop, so that cancel means
    what it says.
14. As a developer, I want to interrupt a Codex turn mid-sentence, so that I can
    correct a prompt I got wrong without losing the conversation.
15. As a developer who interrupts, I want the partial reply kept as it was on
    screen, so that what I read is what the transcript records.
16. As a developer who interrupts, I want to send a correction immediately after,
    so that the next message continues the same conversation rather than starting
    a new one.
17. As a developer, I want a Codex conversation to survive a laplus restart, so
    that closing the window is not the end of my work.
18. As a developer whose Codex history has been pruned or removed, I want the
    conversation to keep working, so that a missing file is not a dead thread.
19. As a developer in that situation, I want to be told the agent no longer
    remembers what came before, so that I do not argue with an agent that has
    quietly forgotten the last hour.
20. As a developer, I want the access mode I picked — supervised, auto-accept
    edits, auto, full access — to mean something concrete on Codex, so that the
    control is not decorative.
21. As a developer on supervised, I want the agent held to a read-only sandbox
    and asked about escapes, so that supervision is enforced rather than
    promised.
22. As a developer on full access, I want no permission questions, so that the
    mode is worth choosing.
23. As a developer, I want the model or access mode I change between turns to
    apply to the next turn, so that the picker tells the truth.
24. As a developer, I want the session status beside my Codex conversation to
    match what the agent is doing, so that I can tell a running turn from a
    finished one at a glance.
25. As a developer whose Codex turn failed, I want the failure and its reason
    recorded, so that I can decide whether to retry.
26. As a developer, I want the composer's skill menu populated for Codex, so that
    the `$` menu is not empty where the agent has skills to offer.
27. As a developer, I want a Codex conversation to run in my project's folder,
    so that relative paths in the transcript mean what I think they mean.
28. As a developer running a Codex conversation and a Claude conversation at
    once, I want each to be unaffected by the other, so that two agents are
    genuinely two agents.
29. As a developer, I want each conversation to record which provider it ran
    under, so that reopening it later shows the truth rather than a default.
30. As a developer who configures a Codex binary somewhere unusual, I want laplus
    to use it, so that a non-standard install is not a blocker.
31. As a developer who points laplus at a `CODEX_HOME` of my own, I want it
    honoured, so that a separate Codex workspace works.
32. As a developer who fills in a setting this server cannot honour, I want to be
    told so at the moment I save, so that I do not spend a week believing I am on
    an account I am not.
33. As a developer on a machine missing an optional Codex dependency, I want
    laplus to run Codex anyway, so that a warning the agent itself shrugs off
    does not read to me as a broken provider.
34. As a maintainer, I want a Codex protocol change to show up as a failing
    golden test, so that I learn about drift from a diff rather than a bug
    report.
35. As a maintainer, I want an unrecognised Codex event to be counted rather than
    fatal, so that a codex release never kills a developer's session.
36. As a maintainer, I want the whole suite to run offline on a machine that has
    never had `codex` installed, so that CI stays free and hermetic.
37. As a maintainer, I want a second driver to reuse the session lifetime logic
    rather than copy it, so that checkpoints, epochs and settling cannot drift
    apart between agents.

## Implementation Decisions

### The seam between the loop and the driver

The session loop becomes generic over a small **driver** trait, with `claude` as
the only implementation first and no behaviour change — the existing suite is
that ticket's whole proof. The trait covers the I/O verbs only: open a session,
take the next event, send a prompt, interrupt, answer an approval, retune, stop.
Everything the loop does around those — baselines, checkpoints, session epochs,
settling, publishing session events — is written once and shared.

`Folded` stays the shared vocabulary between drivers, because it already is one:
its variants map onto Codex without strain. Each driver brings its own protocol
module, its own accumulated state for the two index-carrying variants, and its
own encoder. ADR-0001 already decided that shape — the decoder is mirrored and
shared, the encoder belongs to the driver — and this is the second driver it
anticipated.

The `Driver` glossary entry landed with this spec. Two more belong to the tickets
that make them true: **app-server**, and generalising **agent session id** from
"`claude`'s handle, given back as `--resume`" to "the driver's own handle" —
Codex's is a thread id.

### One app-server per conversation

Each Codex conversation gets its own `codex app-server` child, alive across
turns, reaped when the session ends. This matches how laplus drives `claude`
today, so **session** keeps meaning "the agent process behind a thread" and
session stop, session status and session epoch need no reinterpretation.

It is also upstream's shape, verified in their adapter rather than inferred:
they hold a map from thread to session context and start one runtime per thread.
The protocol would permit one process to host many conversations — a thread
carries its own working directory, model and approval policy — but nobody has
built that, and upstream's per-thread MCP wiring is passed as process launch
arguments, which forecloses it for anyone who wants the agent to reach back.

An ADR records this, because the protocol permitting the opposite makes the
choice surprising to a reader who knows it.

### The handshake, and what settles a turn

laplus sends **empty capabilities** on `initialize`. This is a deliberate
divergence from upstream, who send `experimentalApi`, and the reason is
measured: with that flag set, `turn/completed` is never emitted and a turn ends
on a status change carrying nothing about how it went. Without it,
`turn/completed` arrives and carries the turn's error.

A turn therefore settles on `turn/completed`, whose error decides between
completed and failed. A status change to idle is handled as a terminal fallback,
so the capability can be turned on later — plan mode's collaboration mode looks
to live behind it — without breaking the settle.

**An interrupted turn settles on the interrupt's own response**, because nothing
else marks it: no completion, no status change, and no authoritative version of
the message that was streaming. In-flight output continues to arrive _after_ the
interrupt is sent and before the acknowledgement.

**Reconciliation does not apply to an interrupted Codex turn.** The accumulated
deltas are the final text, where `claude` hands the partial message over whole.
This is a documented divergence in behaviour between the two drivers, not a bug
in either.

### The wire

The Codex protocol types are hand-written for the subset a v1 uses — roughly six
of the eighteen item kinds, a dozen notifications, the four request/response
pairs and the approval requests — in the idiom the `claude` protocol module
already uses. Everything unrecognised degrades into the existing **drift
counter** rather than failing a session. Generated types were rejected for having
the opposite failure mode: strict decoding turns the next item kind OpenAI ships
into a dead session, and the protocol moved eighty releases in seven months.
OpenAI's own crate is published but frozen at a version seven months behind the
CLI.

Three shapes the schema does not tell you, all confirmed by capture:

- Responses carry **no `jsonrpc` member**. A decoder requiring it fails on every
  message.
- **Responses arrive out of order**, so requests are correlated by id through a
  pending map rather than assumed FIFO.
- **The server's own requests use a separate id space beginning at 0**, so one
  map keyed by id across both directions collides.

The agent's version is read from the `initialize` response's user agent rather
than from a `--version` run; binary resolution is otherwise unchanged.

### The provider probe

A provider refresh starts one app-server, asks it four things, and kills it:
version from the handshake, the account, the model list paged to exhaustion, and
the skills for the workspace. This is the shape the catalogue already uses for
`claude` — a session opened for one question — and for Codex it answers both the
provider snapshot and the composer's skill menu in one process.

Models come from the agent rather than a compiled table, because OpenAI's slugs
churn faster than laplus ships: the contract's own alias table already points at
a model this account cannot use, while the live list is correct by construction.
Reasoning efforts are carried per model, since they differ between them.

An account that is not logged in is reported as exactly that, distinct from
broken — the probe can tell, where the `claude` provider cannot.

### Access modes

The four runtime modes translate to Codex's approval policy and sandbox as
upstream translates them, with one declared divergence:

| runtime mode      | approval policy | sandbox            | approvals reviewer |
| ----------------- | --------------- | ------------------ | ------------------ |
| approval-required | untrusted       | read-only          | user               |
| auto-accept-edits | on-request      | workspace-write    | user               |
| auto              | on-request      | workspace-write    | **user**           |
| full-access       | never           | danger-full-access | user               |

Upstream routes `auto` to an OpenAI subagent that decides approvals on the
developer's behalf. laplus keeps the developer as the reviewer, so `auto` and
`auto-accept-edits` behave identically on Codex for now. The reason is what the
developer would see otherwise: the subagent's work is reported through two
notification kinds a v1 does not handle, so the agent would pause, something
invisible would decide, and it would carry on with nothing in the work log.
Routing to the subagent becomes available once those are rendered.

The reviewer is always sent explicitly rather than omitted, because omitting it
on resume leaves whatever the thread last used.

### Approvals

Codex's approval decisions are the contract's four literals with matching
meanings, and `decline` continues the turn where `cancel` interrupts it. But
**which decisions apply is a property of each request**, not of the contract: a
request carries the decisions available to it, and a sandbox-escaping command
was observed offering only accept, an execpolicy amendment, and cancel — no
decline, no accept-for-session. The panel offers what the request allows.

The two structured decisions Codex adds — an execpolicy amendment and a network
policy amendment — are never sent by this server.

A tool call is published before its approval request, which is the order the
work log wants and the order the wire delivers.

### Continuity

A Codex conversation's continuity is one string, its thread id, stored where the
agent session id already lives. A new process resumes a conversation from that
id alone; the rollout under `CODEX_HOME` is the agent's memory.

**Any** failure to resume is treated as recoverable: the driver starts a fresh
thread and publishes an activity saying the previous context could not be
resumed. Upstream matches a list of error phrases instead, and that list does not
match the message the current codex emits — which is the argument for not keeping
a list.

### The registry

The constant that today means both the driver slug and the only instance id
becomes a registry of drivers, and a conversation records and publishes which one
it ran under rather than inheriting a default. Settings accept a `codex` section
alongside the existing one.

`shadowHomePath` is **refused** rather than stored, with a refusal naming the
reason. It is an account-selection setting, and storing one that does nothing
would let a developer believe they are on an account they are not — which is the
failure ADR-0009's rule exists to prevent, even though the field itself is one
the contract knows.

### Noise the driver must tolerate

Codex emits configuration warnings, remote-control status and per-thread MCP
server startup notices before anything has been asked of it, and writes
`ERROR`-level lines to stderr that it then shrugs off — a missing optional
sandbox dependency is one. None of these make a provider broken, and stderr is
classified rather than trusted.

## Testing Decisions

A good test here asserts what a developer or a client can observe — an activity,
a session status, an approval panel, a model list, a settled turn — and never how
the driver reached it. The two seams below are both existing seams in kind; no
third is introduced. In particular the driver trait is not faked in tests: a
fake would assert this project's own abstraction rather than Codex's behaviour,
and the seams below cover both ends of the real path.

**The socket, against a scripted app-server.** The primary seam, and the same one
the `claude` driver is held to: a stand-in binary replays a recorded capture
while the real server runs the real loop, and the assertions are on what the UI
receives. Prior art is the scripted agent in the test harness and the socket
suites for turns, permissions, interrupts and streaming. The Codex stand-in
differs in one way — it must correlate request ids and answer requests rather
than print a stream — and it honours the same kind of recorded stop points the
`claude` replay already needs, since a capture containing an approval cannot be
played past the point where the answer was given.

**The pure fold, against golden files.** Each capture's received half is folded
through a fresh Codex state and compared against an expected JSON. This is the
drift detector, it needs no process, and it is where the degradation of the
twelve unhandled item kinds is checked. Prior art is the existing protocol golden
suite over the `claude` captures.

Every capture does both jobs, which is the rule the `claude` capture README
already states and the reason re-recording after a codex release is worth doing
even when the golden files still match.

Recordings are made by hand against a real, authenticated `codex` and committed;
CI replays them and never spawns the agent, so the suite runs offline, for free,
on a machine that has never had `codex` installed. The captures for a plain
turn, a command execution, an approval, an interrupt, a resume and a missing
rollout already exist as raw evidence in this feature's directory; a ticket
converts them into fixtures with expected folds. One capture is hand-written
rather than recorded, covering degradation a healthy codex never emits.

The registry change is tested by the existing suites continuing to pass — a
conversation must publish the provider it ran under rather than a constant, and
the socket conformance suite already reads that field.

## Out of Scope

- **A second Codex account.** One instance per driver. Shadow homes, continuation
  groups and switching an existing conversation between accounts are upstream
  features this spec deliberately does not take, and `shadowHomePath` is refused
  rather than half-honoured.
- **Text generation.** Commit messages, PR titles and branch names through
  `codex exec`. laplus has no such surface for any driver.
- **The agent reaching back over MCP.** laplus runs no MCP server; ADR-0030 is
  where that stands. Note that Codex starts an MCP server of its own per thread
  regardless, which is a cost rather than a feature to build.
- **Plan mode.** Codex can honour an interaction mode through a collaboration
  mode with injected developer instructions, and laplus stores interaction mode
  without sending it to any agent. Making Codex the exception would be an
  asymmetry between drivers, and the injected instructions are a body of prompt
  to port. Worth doing, later, for both.
- **Auto-review.** Routing `auto` to Codex's approval subagent, and rendering the
  two activity kinds that would make it visible.
- **The rest of the protocol.** Realtime audio, apps, sub-agents, web search,
  image generation, thread archival and rollback, moderation metadata, fuzzy file
  search, hooks. Around seventy notification kinds exist; a v1 handles a dozen
  and counts the rest.
- **A headless pass.** The done bar is a hand-driven session in the window.
  Driving Codex from a phone through a tunnel is a second pass, not this one.

## Further Notes

**The done bar is a session driven by hand, and it is a ticket.** Suite green and
captures committed, then: open laplus, pick Codex, send a prompt, watch it
stream, let it ask permission and answer, interrupt a turn, restart the server
and continue the conversation. What that finds gets written down. The project's
own guidance is that a green suite is not evidence the application works, and
that an afternoon's findings once came from a minute of driving the window.

**One number is worth measuring before the process shape is called settled.**
Codex starts its own MCP server per thread, so one app-server per conversation
means an MCP child per conversation. Five open Codex conversations is five
app-servers and five of those. If that proves expensive, the alternative is the
shared-process shape this spec rejected, and rejecting it was a judgement about
complexity rather than a measurement.

**The capability flag is the one decision most likely to be revisited.** Sending
empty capabilities buys a turn ending that says how the turn went. It may cost
access to features that arrive behind the experimental flag. Handling both
terminal signals is what keeps that door open, and it is a few lines rather than
a design.

**Upstream is evidence, not a dependency, and it is now demonstrably behind in
two places** — a recoverable-error list that no longer matches the message codex
emits, and completion handling that its own handshake prevents from firing.
Read it for how, verify against a capture for what.
