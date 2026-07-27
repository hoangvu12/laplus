# Spec — lightcode v1: Rust server + Tauri shell reusing t3code's UI

Status: ready-for-agent

## Problem Statement

A developer who wants a local coding-agent desktop app today installs t3code and
pays 318 MB on Windows for it. Roughly half of that is not the app — it is an
Electron runtime bundled as both browser engine and Node host, plus a duplicate
Linux `node_modules` tree shipped for an opt-in WSL backend that defaults to off.
The user downloads it, waits through it, and gives up the disk regardless of
whether they ever use the features that justify the weight.

The cost is not only install size. Every update is a full re-download of that
payload, and the app's baseline memory footprint is set by a runtime the user did
not ask for. A developer evaluating a coding agent for the first time meets the
download before they meet the product.

The UI is not the problem. It is a plain browser application that already runs
standalone and speaks to a server over a socket; it does not care what language
serves it. The weight is entirely in the runtime and the server that needs it.

## Solution

lightcode is the same UI, served by a Rust server, wrapped in a Tauri shell that
uses the operating system's existing webview instead of shipping its own. Target
artifact: **20–30 MB**.

From the user's perspective nothing about the interface changes. They open the
app, add a project, browse files, start a conversation with Claude Code, watch it
stream token by token, run terminal commands, and see git status — exactly as
before. What changes is that the download is an order of magnitude smaller, the
app starts faster, and no Node runtime exists on their machine on lightcode's
behalf.

v1 is deliberately narrower than t3code: one agent (Claude Code), one platform
(Windows), no accounts, no cloud, no remote environments, no browser-preview
subsystem. It is the coding-agent core, at a tenth of the weight.

## User Stories

### Installing and running

1. As a developer evaluating a coding agent, I want the installer to be under
   ~30 MB, so that I can try it without committing to a large download.
2. As a developer on a metered or slow connection, I want the app not to bundle a
   browser engine, so that install and update transfers stay small.
3. As a Windows user, I want the app to use the webview already on my system, so
   that no second rendering engine is installed on my behalf.
4. As a developer, I want the app to launch to an interactive window quickly, so
   that it feels like a native tool rather than a web page in a wrapper.
5. As a developer who already has Claude Code installed, I want lightcode to find
   and use my existing `claude` binary, so that I am not made to install a second
   copy or re-authenticate.
6. As a developer whose `claude` binary is missing or not on PATH, I want a clear
   diagnostic naming what was looked for and where, so that I can fix it without
   reading logs.
7. As a developer, I want the app to run entirely on my machine with no account,
   login, or network service of its own, so that I can use it offline and on
   private code.

### Projects and files

8. As a developer, I want to add a local folder as a project, so that the agent
   has a working directory.
9. As a developer, I want my project list to persist across restarts, so that I
   do not re-add folders every session.
10. As a developer, I want to remove a project from the list without touching the
    files on disk, so that the list stays relevant.
11. As a developer, I want to browse the filesystem to pick a folder, so that I
    can add a project without typing a path.
12. As a developer, I want to see my project's file tree, so that I can navigate
    the codebase.
13. As a developer, I want to expand directories lazily, so that a large
    repository does not stall the UI.
14. As a developer, I want to open a file and read its contents, so that I can
    review what the agent is working on.
15. As a developer, I want to search for files by name within a project, so that
    I can jump to a file without walking the tree.
16. As a developer, I want to edit and save a file from the UI, so that I can make
    a small correction without leaving the app.
17. As a developer, I want the file tree to reflect changes the agent makes on
    disk, so that what I see stays true while the agent works.
18. As a developer, I want to open a file in my external editor, so that I can
    switch to my normal tooling when I want to.
19. As a developer, I want binary and very large files to be refused gracefully
    with an explanation, so that the UI does not hang or render garbage.

### Talking to the agent

20. As a developer, I want to start a conversation with Claude Code scoped to a
    project, so that the agent has the right working directory.
21. As a developer, I want my prompt to be sent to the agent and acknowledged
    immediately, so that I know it was received.
22. As a developer, I want the agent's reply to appear token by token as it is
    produced, so that the app feels alive during a long turn.
23. As a developer, I want the final rendered reply to be exactly what the agent
    actually said, so that a dropped streaming update never silently truncates
    output I rely on.
24. As a developer, I want to send a follow-up message in the same conversation,
    so that the agent retains the context of what we already discussed.
25. As a developer, I want my conversation to survive closing and reopening the
    app, so that I can resume work the next day.
26. As a developer, I want to see when the agent is thinking versus writing, so
    that a pause does not read as a hang.
27. As a developer, I want to see which tool the agent is invoking and on what,
    so that I can follow its reasoning.
28. As a developer, I want to see the result of each tool call, so that I can tell
    whether a step succeeded.
29. As a developer, I want to approve or reject an action the agent asks
    permission for, so that I stay in control of what runs against my code.
30. As a developer, I want a rejected permission prompt to return control to the
    agent cleanly, so that the session continues instead of dying.
31. As a developer, I want to interrupt the agent mid-turn, so that I can stop it
    when it is heading the wrong way.
32. As a developer, I want an interrupted turn to leave the conversation in a
    usable state, so that I can immediately send a correction.
33. As a developer, I want to see the model in use for the session, so that I know
    what I am talking to.
34. As a developer, I want to see the permission mode in effect, so that I know
    how much latitude the agent has.
35. As a developer, I want to see token cost and duration for a completed turn, so
    that I can manage spend.
36. As a developer, I want an agent error to be reported in the conversation
    rather than crashing the session, so that I can retry.
37. As a developer, I want the app to survive the `claude` process exiting
    unexpectedly, so that one bad turn does not take down the window.
38. As a developer running a long session, I want context compaction to be handled
    without losing the visible transcript, so that long work stays coherent.
39. As a developer, I want to run more than one conversation at a time across
    different projects, so that I can parallelise work.

### Terminal

40. As a developer, I want to open a terminal in my project directory, so that I
    can run commands alongside the agent.
41. As a developer, I want the terminal to render interactive programs correctly,
    so that normal shell tooling works.
42. As a developer, I want the terminal to resize with the pane, so that output
    wraps correctly.
43. As a developer, I want to reattach to a still-running terminal after
    navigating away, so that long-running processes are not lost.
44. As a developer, I want to clear and restart a terminal session, so that I can
    reset a broken shell.
45. As a developer, I want terminals to be closed and their processes reaped when
    I close them, so that nothing is orphaned.

### Git

46. As a developer, I want to see my working tree status, so that I can tell what
    the agent changed.
47. As a developer, I want status to refresh as files change, so that it stays
    accurate during a session.
48. As a developer, I want to see the diff for a single agent turn, so that I can
    review one step in isolation.
49. As a developer, I want to see the cumulative diff for a whole conversation, so
    that I can review the session as one change.
50. As a developer, I want to see the current branch and switch between branches,
    so that I can keep work separated.
51. As a developer, I want to create a branch from the UI, so that I can start
    work without switching to a terminal.
52. As a developer, I want to initialise a repository in a project that has none,
    so that agent changes become reviewable.

### Settings and configuration

53. As a developer, I want my settings to persist across restarts, so that I
    configure once.
54. As a developer, I want to configure the Claude Code provider instance, so that
    model and options match how I work.
55. As a developer, I want to customise keybindings, so that the app matches my
    muscle memory.
56. As a developer, I want the UI to receive configuration changes without a
    restart, so that edits take effect immediately.

### Trust in the port itself

57. As a lightcode maintainer, I want the unmodified upstream UI to connect to the
    Rust server and complete its initial handshake, so that I know the transport
    is genuinely compatible rather than approximately so.
58. As a lightcode maintainer, I want the socket wire format pinned as captured
    fixtures from the real reference server, so that framing correctness is
    verifiable without guessing from type definitions.
59. As a lightcode maintainer, I want an unrecognised agent event to increment a
    counter rather than terminate the session, so that a CLI upgrade degrades
    instead of breaking.
60. As a lightcode maintainer, I want protocol-drift counters surfaced, so that I
    learn the format moved from a number rather than a bug report.
61. As a lightcode maintainer, I want agent-facing tests to run against a scripted
    fake CLI, so that the suite is deterministic, offline, and free.
62. As a lightcode maintainer, I want the final artifact size measured as part of
    the build, so that the project's reason for existing is tracked rather than
    assumed.
63. As a lightcode maintainer, I want the Rust server to stay under roughly 20K
    LOC, so that scope creep back toward parity is visible early.

## Implementation Decisions

### Settled product decisions

- **Hard fork.** lightcode copies from t3code once and diverges. The contracts
  package is a _blueprint_, not a mirror; shapes are reimplemented in Rust to fit
  the server's needs and are free to drift from upstream. Accepted cost: no free
  upstream UI fixes. Upstream is pinned at the vendored checkout's current commit
  and is reference material only, never a build dependency.
- **Claude Code only in v1.** The provider surface is implemented generically
  enough to admit a second driver later, but exactly one driver ships. Codex and
  OpenCode are separate protocols and separate work.
- **Windows only in v1.** Removes the Linux WebKitGTK bundle-size problem and the
  cross-engine QA matrix. The webview is installed via download bootstrapper, so
  it contributes ~0 MB to the artifact.
- **Licence.** Upstream is MIT. Reuse of the UI is permitted; the copyright
  notice is retained in the fork.

### The transport is the contract

The UI talks to the server over **one authenticated WebSocket endpoint** carrying
**Effect RPC framed with JSON serialization**. It is not a REST surface. The
method vocabulary is roughly sixty request/response methods plus a set of
server-streaming subscriptions; the orchestration methods carry the agent session
lifecycle and are the core.

Three consequences shape the whole build:

- **The framing, not the schemas, is the risk.** The payload schemas are readable
  from the contracts package. The envelope around them — request/response
  correlation, error tagging, and the chunk/end semantics of streaming methods —
  is undocumented and comes from an explicitly _unstable_ module of the Effect
  library. This displaces agent-protocol instability as the project's primary
  risk.
- **Capture-and-conform, before the server skeleton.** The first unit of work is
  to run the reference TypeScript server, connect the real UI to it, and capture
  socket frames for a representative set of methods: the initial handshake, a
  plain request/response, an error response, and a streaming subscription through
  to completion. Those captures become the fixtures the Rust implementation is
  written against. This is the same manoeuvre the protocol spike used to retire
  the CLI-format risk, applied to the risk that replaced it.
- **The handshake is the tracer bullet.** The client fetches server configuration
  as its first call and can do nothing until a well-formed response arrives.
  Getting exactly that one method to satisfy the unmodified UI is the thinnest
  possible vertical slice that proves the transport, and it is therefore the first
  functional ticket.

### Connection authentication

The reference server authenticates the socket upgrade before handing off to the
RPC layer, and the client supplies a credential when connecting. Account-based
auth is out of scope, but the _handshake shape_ is not optional — the client will
send it. v1 implements a permissive local-only scheme: accept the client's
handshake shape, bind the server to loopback, and reject non-local origins. The
credential is not verified against any identity store.

### Agent driver

- **Binary resolution is a plain PATH lookup.** On Windows the `claude`
  executable is a native binary, so the upstream server's npm-shim resolution
  logic is dead weight and is not ported. Resolution is: explicit configured path,
  else PATH lookup, else a diagnostic naming both.
- **The subprocess protocol is settled.** The CLI is driven with print mode,
  stream-json in both directions, partial messages enabled, and verbose output —
  bidirectional NDJSON over stdin/stdout, one long-lived process per session.
  Session continuity uses the CLI's session-id and resume flags.
- **The wire format is isolated in one module.** The spike's protocol module lifts
  into the server unchanged. It is pure — parsing and state folding, no I/O — so a
  CLI format change has a blast radius of one file.
- **Unknown variants degrade to counters.** Every enum over the CLI's event types
  has a catch-all arm that increments a drift counter instead of failing. The
  session survives an unrecognised event; the counter is surfaced so drift is
  observed as a number.

### Accumulate-and-reconcile for assistant text

Assistant text arrives **twice**: incrementally as content-block deltas, and again
as a complete buffered message. The prototype settled how to handle this, and the
rule is load-bearing enough to state precisely — deltas drive live rendering,
the buffered message is authoritative and replaces the accumulation when it lands.

From the prototype's reducer, trimmed to the decision:

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

Rendering deltas alone risks silently truncated output; waiting for the buffered
message alone makes streaming pointless. The `from_deltas` flag records whether
the two agreed, which is a cheap continuous check on the assumption.

### Server composition

- HTTP and WebSocket via `axum`; async runtime `tokio`.
- Agent subprocess via `tokio::process`, one child per session, with explicit
  lifecycle so a dying child is reported rather than orphaned.
- Terminals via `portable-pty`, one PTY per terminal session, with reattach.
- Git by shelling out to the `git` binary. No libgit2 linkage in v1.
- Persistence via `rusqlite` — project registry, conversation transcripts,
  settings, keybindings.
- File watching via `notify`, feeding the file-tree and git-status subscriptions.
- Contract types as `serde` structs, hand-written from the contracts package and
  validated against captured frames.
- Desktop shell via Tauri v2, webview pointed at the embedded server.

### Effect semantics are not ported

The upstream server is heavily Effect-based — structured concurrency, typed
errors, resource scoping. These are read for _behaviour_, not structure. The Rust
implementation uses `Result`, `?`, `tokio` tasks and RAII. Attempting to mirror
Effect idioms in Rust is explicitly rejected.

### Build order

1. Capture and pin the socket wire format from the reference server.
2. Server skeleton: socket endpoint, permissive local auth, RPC framing, and the
   configuration handshake — the unmodified UI connects and stays connected.
3. Project registry and filesystem methods — the file tree renders.
4. Provider and orchestration — the full agent session lifecycle, streaming,
   tool use, permissions, interrupts.
5. Terminal.
6. Git.
7. Persistence.
8. Tauri wrap.
9. Bundle and measure.

## Testing Decisions

### What makes a good test here

A good test drives the system at the boundary the UI actually uses and asserts on
what the UI would observe. It does not reach into server internals, assert on
struct fields that no client can see, or pin the shape of intermediate state.
Concretely: assert that a method call returns a well-formed response and that a
subscription emits the expected sequence of events and terminates — not that a
particular session struct held a particular value at a particular moment.

Tests must be deterministic, offline, and free. No test invokes the real Anthropic
API.

### Primary seam — the WebSocket RPC boundary

The bulk of testing happens here. A test harness starts the server, connects a
WebSocket client, and drives RPC methods. This is the genuine contract with the
UI, so it is the highest available seam, and the filesystem, project, provider,
orchestration, terminal and git subsystems are all exercised through it rather
than each acquiring its own seam.

Determinism for agent-facing tests comes from a **scripted fake `claude`
executable** that replays canned NDJSON captures. It is injected through the
existing agent-executable-path configuration — a value the server already needs
for real use, so no test-only seam is added to production code. The fake replays
the spike's captures for baseline cases, plus purpose-built scripts for tool use,
permission prompts, interrupts, errors, and abrupt child exit.

Coverage at this seam: the configuration handshake; project add/remove/list;
file tree, read, search and write; a complete agent turn including streamed
deltas and the reconciled final message; multi-turn continuity; tool-use
round-trips; permission approval and rejection; interrupt; agent error; child
process death; terminal open/write/resize/reattach/close; git status and diffs.

### Secondary seam — the protocol module, as a drift detector

The protocol module is pure and already has real captures alongside it. It gets
golden-file tests: feed captured NDJSON, assert the folded state.

This earns its place as a second seam because it isolates a failure the primary
seam cannot. When a `claude` release changes the wire format, re-capturing and
re-running these tells you _the CLI moved_ directly, without standing up a server
or disentangling the failure from server logic. It is the fast signal on the
project's most externally-volatile dependency. Its assertions stay at the level of
observable outcome — parsed events and folded transcript state — not internal
bookkeeping.

### Frame-level conformance

The captured reference frames from the capture-and-conform step are used as
fixtures at the primary seam: the Rust server's responses are compared against
what the reference server produced for the same calls. This is checked through the
RPC boundary rather than by unit-testing an encoder, so that assertions track
observable protocol behaviour rather than codec internals.

### Prior art

The upstream repository is the reference for how to test this shape:

- The upstream server's own test suite builds an RPC client over the same JSON
  serialization layer and drives the server in-process. That is the direct
  analogue of the primary seam, one language over.
- The contracts package carries schema round-trip tests per contract module —
  the pattern to follow when hand-writing Rust types against captured payloads.
- The upstream orchestration contract has by far the largest test file in the
  package, which is a reasonable signal of where the semantic complexity lives and
  where Rust test effort should concentrate.

### Not tested automatically in v1

Rendering fidelity of the UI itself. The UI is reused unmodified and is upstream's
to test; lightcode's obligation is the server side of the contract. That the real
UI connects and drives a session end-to-end is verified manually at each build-
order milestone.

## Out of Scope

- **Accounts and authentication.** No login, no identity store, no multi-user.
  Only the permissive local handshake described above.
- **Cloud and remote environments.** No relay client, no Tailscale, no SSH, no
  remote or containerised environments.
- **The browser-preview subsystem.** Preview and preview-automation — the
  CDP-driven in-app browser — are excluded entirely, along with local dev-server
  discovery.
- **WSL support.** Excluded, and with it the duplicate Linux dependency tree that
  accounts for most of upstream's Windows-versus-macOS size gap.
- **Agents other than Claude Code.**
- **macOS and Linux.** Including the custom-titlebar drag-region question, which
  only matters off Windows.
- **The mobile and Electron desktop applications.** Only the web UI is reused.
- **Source-control hosting integrations.** No GitHub, GitLab, Bitbucket or Azure
  DevOps provider integrations; no pull-request workflows, stacked-diff actions,
  repository cloning or publishing. Local `git` only.
- **Asset URL service, external diagnostics and process-management surfaces.**
- **Delta updates.** The Tauri updater has no differential download support.
  Full-artifact updates only — acceptable precisely because the artifact is small.
- **Feature parity with upstream.** Explicitly a non-goal. If the Rust server
  grows past roughly 20K LOC, that is the signal to stop and re-evaluate.

## Further Notes

### The risk picture has changed

The original plan named agent-protocol instability as risk #1 and budgeted a week
to test it. The spike passed in hours and materially lowered that risk: the CLI's
envelope is thin — six variants — and the payloads inside it are standard,
versioned Anthropic Messages API types, so the genuinely unstable surface is small
and already isolated behind one pure module with drift counters.

**The transport framing takes its place as risk #1.** It is undocumented, it comes
from an explicitly unstable module, the reference implementation's dependencies
are not even installed in the vendored checkout, and unlike the CLI protocol it
was not touched by the spike. Capture-and-conform is the mitigation, sequenced
first for the same reason the CLI spike was sequenced first.

Two risks carry over unchanged: scope creep back toward parity, and the
temptation to port Effect idioms literally into Rust.

### What the spike did not prove

The spike proved the protocol half — the harder half — and not the transport half.
Still unproven at spec time: the UI rendering against a Rust server, multi-turn
continuity via session-id and resume, tool-use round-trips and permission prompts,
and long-session compaction and interrupts. None are protocol-format risks; they
are ordinary implementation work, and the build order above sequences them.

### Success is measurable

The project exists for one number. The artifact is measured at every build, and
20–30 MB is the target against upstream's 318 MB Windows installer. If the
measured artifact drifts materially above that range, the project's rationale is
weakened and the fallback deserves reconsideration.

### The fallback still stands

If the port stalls, the alternative is pruning the upstream Electron build —
weeks rather than months, no feature loss, reaching roughly 200 MB on Windows.
That work overlaps with analysis already done and is not wasted.

### Reference artifacts

Durable background, not to be re-derived: the original plan and its size and
payload measurements; the protocol spike's write-up, which records the exact CLI
flags, the verified output, and an explicit account of what it did not prove; the
raw NDJSON captures used as its primary source; and the research note on why a
plain Electron-to-Tauri migration was rejected. All are in the repository root and
the spike directory.
