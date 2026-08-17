# Subagent visibility and navigation research

Date: 2026-08-17

## Question and source policy

This note compares how coding-agent interfaces make delegated work legible:
identity, hierarchy, state, current activity, results, and navigation. It uses
only first-party documentation and official repository source/snapshots. UI
behavior described from source is explicit evidence; recommendations for
laplus are marked as synthesis.

## Executive summary

The strongest products do not treat a subagent as a generic tool call. The
core experience is **entering the child's live work session**: users can select
a worker, watch the same prose and work entries it is producing in real time,
scroll its history, switch to a sibling, and return later to replay it. OpenCode
is the clearest model: its selector opens a fixed 14-row streaming inspector
that reuses the application's normal entry rendering, follows the bottom while
new work arrives, and supports sibling navigation.

That experience has two complementary surfaces:

1. **A compact parent-timeline launcher:** semantic name, assigned task and
   live state, with a clear affordance to open the worker. It is an index into
   child work, not the place where that work is flattened.
2. **A live child-work inspector:** the child's chronological prose, commands,
   reads, edits/diffs, other tool calls, errors, and finally its result. It is
   independently scrollable, follows live output when pinned to the bottom,
   and remains replayable after completion or reload.

That combination answers all three questions users actually have: “what is the
team doing?”, “what exactly is this worker doing?”, and “what did it conclude?”
A label such as `Subagent 1` answers none of them. The result is not a separate
replacement for the work view; it is the terminal part of the same child
stream.

## Comparison matrix

| Tool                                         | Identity and hierarchy                                                                                                                                             | At-a-glance state/activity                                                                                                                                                                                                                          | Detailed inspection and results                                                                                                                                                                                                                     | Navigation/layout                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **OpenAI Codex CLI**                         | Children use nickname and role, falling back through path/thread identity; canonical agent paths preserve ancestry. Descendants are discovered from the root.      | Timeline rows say Spawned, Sent input, Waiting, Resumed, Closed, etc. `/agent` also emits a “Sub-agents running” feed with up to three recent activity lines per worker: prose, command, file update, MCP/tool, web, image, or nested-agent action. | Completed waits show each agent's status and a bounded response/error preview. Closed agents remain selectable.                                                                                                                                     | `/agent` opens a searchable Subagents picker with state dot, label/path and UUID; selection switches to the child transcript. Alt-Left/Right cycles threads. [Rendering and events](https://github.com/openai/codex/blob/main/codex-rs/tui/src/multi_agents.rs), [picker and ancestry](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/agent_picker.rs), [activity feed](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/agent_status_feed.rs), [thread lifecycle/navigation](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/session_lifecycle.rs).                                                                                                                                                                                                           |
| **OpenCode CLI**                             | Task rows use the authored description as title and agent type as secondary identity; child sessions are stable tabs.                                              | Inline task row uses running/success/error icon. A footer command advertises `N active` or `N recent`; the selector shows every child's title, agent label, and `running/done/cancelled/error`.                                                     | A 14-row inspector streams the selected child's normal rendered entries, with spinner/status icon, sticky bottom, scrollbar, and “No subagent activity yet” empty state.                                                                            | “View subagents” opens a searchable selector; selecting opens the inspector, Tab cycles workers, Esc returns. [task-row renderer](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/tool.ts#L366-L375), [selector](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/footer.command.tsx#L568-L669), [inspector](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/footer.subagent.tsx).                                                                                                                                                                                                                           |
| **Gemini CLI**                               | Named specialist agents; sibling runs are grouped. Recursion is prohibited, so the UI is intentionally flat rather than a tree.                                    | Bordered group card says `Running Agent`, terminal state, or `N Agents (R running, C completed)`. Collapsed rows show status icon, bold name and latest activity/thought.                                                                           | Ctrl+O expands to chronological thoughts and tool calls (name plus concise argument), followed by Markdown final result or early-finish reason.                                                                                                     | Inline disclosure rather than switching transcripts. [group component](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/messages/SubagentGroupDisplay.tsx), [group snapshots](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/messages/__snapshots__/SubagentGroupDisplay.test.tsx.snap), [detail component](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/messages/SubagentProgressDisplay.tsx), [detail snapshots](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/messages/__snapshots__/SubagentProgressDisplay.test.tsx.snap), [subagent model and recursion rule](https://github.com/google-gemini/gemini-cli/blob/main/docs/core/subagents.md). |
| **pi example extension**                     | Explicit agent name and source scope. Supports one worker, parallel tasks, and sequential chains; chain step numbers expose structure.                             | Call preview includes agent plus task text. Parallel result continuously shows `X/Y done, N running`; every worker has hourglass/check/error plus its latest text and tool-call rows.                                                               | Ctrl+O expands. Single-worker detail separates Task and Output, renders all tool calls and final Markdown, then turns/tokens/cache/cost/context/model. Parallel/chain detail retains per-worker task, calls, result and usage plus aggregate usage. | One expandable tool block in the parent transcript; no separate child navigation. [official example extension](https://github.com/badlogic/pi-mono/blob/0e4d49541477c4fc6e404f845ad40ed47d157f24/packages/coding-agent/examples/extensions/subagent/index.ts#L718-L1055).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| **Claude Code** (useful adjacent comparison) | Named agents are directly mentionable; a configurable color appears in task lists/transcripts. Each invocation has an agent ID and persistent separate transcript. | Foreground agents block; background agents run concurrently. Currently running named agents appear in `@` typeahead with status.                                                                                                                    | Final response returns to the parent; transcripts preserve tool calls/results and can be resumed.                                                                                                                                                   | Background work is available without blocking the conversation. Claude's separate Agent View is deliberately for independent sessions rather than parent-reporting subagents, but its table/peek/attach pattern reinforces overview-then-drill-in. [subagents documentation](https://code.claude.com/docs/en/subagents), [Agent View](https://code.claude.com/docs/en/agent-view).                                                                                                                                                                                                                                                                                                                                                                                                                          |

## Product details worth borrowing

### OpenAI Codex: compact history plus real child navigation

Codex uses semantic, durable identity rather than ordinal labels. Its picker
prefers the canonical agent path for non-primary threads, uses nickname/role
when available, searches both labels and UUIDs, and shows whether a thread is
open or closed. The picker queries all descendants of the root, so nested work
does not disappear merely because the immediate parent is not selected
([picker source](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/agent_picker.rs)).

The parent transcript stays compact: spawn, interaction, wait, resume and close
are readable event rows, and terminal waits include bounded answer/error
previews. When users ask for the agent picker, Codex first writes a status feed
into history. Each running path gets recent text/tool/file/web activity, rather
than an uninformative spinner
([multi-agent rows](https://github.com/openai/codex/blob/main/codex-rs/tui/src/multi_agents.rs),
[status feed](https://github.com/openai/codex/blob/main/codex-rs/tui/src/app/agent_status_feed.rs)).

The Codex app's first-party announcement describes parallel work as separate
threads organized by project, with per-thread diffs and worktree isolation
([Codex app announcement](https://openai.com/index/introducing-the-codex-app/)).
The exact current desktop subagent-panel styling is not documented in
inspectable first-party UI source, so it should not be treated as stronger
evidence than the open-source CLI.

### OpenCode: a lightweight but unusually complete inspector

OpenCode's current CLI is the clearest direct analogue for a laplus web UI. The
ordinary timeline keeps one concise Task row, but the footer exposes “View
subagents” with active/recent counts. A searchable picker shows description,
agent label and terminal/running state. Selecting a child opens a fixed 14-row
inspector whose content reuses normal entry rendering rather than a special
summary format. It streams new child entries, sticks to the bottom while the
user follows live work, exposes a scrollbar and an explicit empty state, and
permits Tab sibling cycling and Esc return
([selector](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/footer.command.tsx#L568-L669),
[inspector](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/footer.subagent.tsx)).

OpenCode's user-facing model also exposes child-session navigation from parent
to first child, among siblings, and back to the parent. Users can invoke a named
subagent directly with `@name`
([official agent docs](https://opencode.ai/docs/agents#usage)). This makes the
child a navigable and revisitable work session, not disposable tool output.
The important behavior is not merely the picker or status label: users can
watch the worker's actual rendered session while it is working.

#### OpenCode blockers: child requests surface at the active parent

**Proven behavior (OpenCode commit `2cba7e2`).** A child can generate a tool
permission request: every tool permission request is tagged with the session
that is executing the tool, and the child session runs with the selected
subagent's rules plus session-level restrictions
([tool request attribution](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/session/tools.ts#L81-L88),
[child-session construction](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/tool/task.ts#L131-L172)).
The defaults deny the `question` tool for agents, including the built-in
`general` subagent, but user configuration is merged after those defaults; a
custom override can therefore enable it. When enabled, the question tool emits
a `question.asked` request carrying the child's `sessionID`
([agent defaults and merge](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/agent/agent.ts#L119-L138),
[question event attribution](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/tool/question.ts#L14-L41)).

OpenCode does not require the user to be looking at that child. In the desktop
app, the session page searches the selected session's entire descendant tree
for the first pending permission or question, then replaces the normal composer
area with the corresponding actionable dock
([tree lookup](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/app/src/pages/session/composer/session-request-tree.ts#L3-L52),
[composer selection](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/app/src/pages/session/composer/session-composer-state.ts#L28-L50),
[permission/question docks](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/app/src/pages/session/composer/session-composer-region.tsx#L39-L59)).
The terminal UI likewise merges root and child queues in arrival order for its
single footer blocker view, and marks the affected child tab `Pending
permission` or `Pending question`
([footer queue merge](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/stream.transport.ts#L292-L308),
[child blocker tabs](https://github.com/sst/opencode/blob/2cba7e227d68a7e7e4a2aa9c85b808e8ecb14daf/packages/opencode/src/cli/cmd/run/subagent-data.ts#L710-L757)).
Thus the child owns and receives the answer, while the currently visible parent
surface provides the interaction so the blocker cannot hide in an inactive
inspector.

The source proves request routing and dock/footer placement, but not a separate
graphical "subagent inspector open versus inactive" presentation: the desktop
implementation navigates session pages, while the 14-row inspector described
above belongs to the terminal runner. No first-party screenshot or automated
visual snapshot found here establishes additional desktop decoration for the
originating child. That styling is unknown and should not be copied as fact.

**What Laplus receives today.** OpenCode's shared SSE feed includes
`permission.*` and `question.*` envelopes with a `sessionID`, and Laplus's wire
decoder already recognizes those event names
([protocol vocabulary](../../server/crates/laplus-server/src/opencode_protocol.rs)).
However, the adapter diverts every event whose session differs from the root
to `child_session_event`; that function intentionally retains only child
assistant text for the compact row and ignores child tool calls, permissions,
and questions
([routing](../../server/crates/laplus-server/src/opencode.rs#L1593-L1610),
[child normalization](../../server/crates/laplus-server/src/opencode.rs#L1234-L1317)).
Consequently Laplus currently receives child blocker envelopes on the wire but
does **not** normalize, persist, display, or reply to them. Its existing
approval/question path only runs for root-session events. The fixture coverage
proves child prose and root blockers separately, but contains no child
permission/question case
([OpenCode socket fixture](../../server/crates/laplus-server/tests/socket_opencode_turn.rs#L840-L889)).

**Design consequence.** OpenCode supports the earlier recommendation: keep the
subagent tab read-only, show the request in that child's replayable stream, and
surface the actionable approval/question dock through the parent conversation's
existing composer area even when the child tab is closed. Implementing that in
Laplus requires preserving child-session identity through normalization; it is
not merely a UI change. Whether Claude and Codex expose equivalent child-owned
blocker events remains unverified here and should not be inferred from
OpenCode.

### Gemini CLI: best inline parallel overview

Gemini gives parallel workers one visual group and makes its header carry
aggregate progress. The collapsed representation preserves one meaningful
line per worker—name, status, and latest activity—while Ctrl+O reveals each
agent's chronological thinking/tool stream and final Markdown result
([group snapshots](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/messages/__snapshots__/SubagentGroupDisplay.test.tsx.snap),
[progress snapshots](https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/ui/components/messages/__snapshots__/SubagentProgressDisplay.test.tsx.snap)).

This is valuable for laplus because it scales from one to several workers
without turning the parent timeline into an interleaved raw event stream. The
flat grouping is consistent with Gemini's explicit recursion protection:
subagents cannot invoke other subagents
([official docs](https://github.com/google-gemini/gemini-cli/blob/main/docs/core/subagents.md#subagent-tool-isolation)).

### pi: rich results without leaving the transcript

pi's bundled subagent example is explicitly an extension rather than core
product behavior, but it is first-party, executable UI code. It previews the
assigned task at dispatch; represents single, parallel, and chain modes; and
updates partial result details as JSON-mode child processes emit messages. The
collapsed parallel view reports aggregate progress and keeps the most recent
five display items per worker. Expanded results distinguish Task from Output,
show all tool calls and final Markdown, and disclose usage/model metadata
([official source](https://github.com/badlogic/pi-mono/blob/0e4d49541477c4fc6e404f845ad40ed47d157f24/packages/coding-agent/examples/extensions/subagent/index.ts#L718-L1055)).

The important principle is progressive disclosure: the default answers “who,
doing what, and is it moving?”; expansion answers “how, what came back, and how
expensive was it?”

## Graphical UI evidence: what is actually visible

The terminal comparisons above establish useful interaction mechanics, but
laplus is a graphical application. This section therefore narrows the evidence
to first-party web and desktop interfaces whose layout and drill-in behavior
are documented or shown by their makers. It also separates **delegated child
agents** from **independent parallel sessions**. The latter are useful layout
references, but they do not by themselves solve parent/child identity.

### True delegated children

#### Devin: nested children that open into a complete work environment

Devin is the closest graphical analogue to the requested experience. Its
"Devin Manages Devins" feature makes the main session a coordinator that
scopes work, monitors progress, resolves conflicts, and compiles results from
parallel managed Devins. Each child is a full session in an isolated virtual
machine. The graphical session list preserves that relationship: sub-Devin
sessions are nested beneath the parent, expose status at a glance, and can be
pinned or reordered independently
([official release notes](https://docs.devin.ai/release-notes/overview)).

Selecting a session is not merely opening a result card. Devin's selected
session has a unified **Progress** view in which every shell command, code edit,
and browser action is logged. Clicking a progress step reveals the concrete
activity for that step. Separate Shell, IDE, and Desktop surfaces let the user
inspect command history and outputs, watch edits in real time, view the
agent's browser, or pause and take over the environment
([official session-tools guide](https://docs.devin.ai/work-with-devin/devin-session-tools)).

The transferable pattern is:

```text
Session sidebar                 Selected child workspace
  Parent task                   Progress | IDE | Shell | Desktop
    ├─ child A · working  --->  full chronological work and live environment
    └─ child B · done           result remains part of the same session
```

This is strong evidence for retaining a compact parent/child index while
making the selected child a first-class, replayable session. Devin's full
remote IDE is more machinery than laplus needs initially; the important part
is that the detail surface contains the work, not only its summary.

#### Warp Oz: a fleet/control-plane view for coordinated subagents

Warp Oz explicitly supports automatic orchestration of parallel subagents and
provides a management interface showing progress across those children, with
tracking and steering across different agent harnesses. The same system keeps
audit logs and exposes both artifacts and raw conversations
([official Oz announcement](https://www.warp.dev/blog/multi-harness-cloud-agent-orchestration)).

This is evidence for a separate orchestration overview when a parent may own
many children. It is less concrete than Devin about the selected-child screen,
but stronger on fleet concerns: aggregate progress, governance, steering, and
cross-agent comparison. For laplus, that argues for keeping status and latest
activity visible in the child list even while one child's stream is open.

### Independent parallel sessions: useful layouts, different semantics

#### Cursor: persistent sidebar plus optional tiled live panes

Cursor lists foreground and background agents in a left sidebar; selecting a
background agent lets the user inspect its remote machine
([official changelog](https://cursor.com/changelog/1-4)). Cursor's newer Agents
Window can split into persistent tiles, drag agents between tiles, expand one
conversation to focus, navigate by keyboard, and jump from a diff to the exact
file line
([official tiled-layout changelog](https://cursor.com/changelog/3-1)).

These are user-launched peer sessions, not children delegated by a visible
parent. The layout is nevertheless the best evidence for an optional
comparison mode: two or three selected agents visible side-by-side. It should
not be laplus's default because multiple narrow streams compete with the
parent conversation and do not communicate hierarchy.

#### OpenAI Codex app: project-grouped threads with review inside the thread

The Codex app organizes agents as separate threads grouped by project. Users
switch between tasks without losing context; each agent uses an isolated
worktree, and its thread contains the change review, diff comments, and an
affordance to open the work in an editor
([official Codex app announcement](https://openai.com/index/introducing-the-codex-app/)).
Newer app surfaces add multiple files and terminals, an in-app browser, and a
summary pane for plans, sources, and artifacts
([official product update](https://openai.com/index/codex-for-almost-everything/)).

The valuable transfer is the union of transcript and review: opening an agent
should lead to both what it did and what changed. The documented graphical app
does not establish a nested parent/child presentation, so it should not be
used as evidence for flattening laplus subagents into project-level peers.

#### Google Antigravity: manager overview, artifact-first verification

Antigravity deliberately separates an Editor View from a dedicated Manager
Surface used to spawn, orchestrate, and observe several asynchronous agents
across workspaces. An agent may work across editor, terminal, and browser, but
its review surface emphasizes artifacts—plans, task lists, screenshots, and
browser recordings—with inline feedback that the agent can incorporate while
continuing
([official Google announcement](https://developers.googleblog.com/en/build-with-google-antigravity-our-new-agentic-development-platform/)).

Google does not document these as a parent delegating to child sessions. The
useful lesson is therefore about review priority, not hierarchy: show a concise
result/diff/artifact summary near the top of a completed child's detail view,
while retaining the full activity stream underneath for audit and debugging.

#### GitHub Copilot: session index opening an auditable log

The GitHub Copilot app groups active sessions by repository in a sidebar and
switches the main surface to the selected session. Every session has an
isolated workspace and may run in a local repository, worktree, or cloud
sandbox
([official app-session guide](https://docs.github.com/en/copilot/how-tos/github-copilot-app/agent-sessions)).
GitHub's web Agents panel opens a session log and overview with live progress,
token usage, duration, steering, stop/archive actions, and durable sharing;
shared viewers can inspect prompts, responses, and file changes
([official session-management guide](https://docs.github.com/en/copilot/how-tos/copilot-on-github/use-copilot-agents/manage-and-track-agents)).

Again these are primarily independent sessions. They validate persistence and
auditability: completed child work should remain addressable, searchable, and
replayable instead of disappearing when its parent timeline is folded.

### Graphical patterns compared

| Product        | Actual relationship                     | Overview/navigation                                 | Selected-work surface                                            | Best lesson for laplus                                  |
| -------------- | --------------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------- | ------------------------------------------------------- |
| Devin          | True coordinator → child sessions       | Children nested beneath parent with live status     | Progress timeline plus IDE, Shell, Desktop and result            | Make every child an enterable, complete session         |
| Warp Oz        | True orchestration → parallel subagents | Management/control-plane progress across children   | Steering, artifacts, raw conversations and audit                 | Preserve fleet context while inspecting one child       |
| Cursor         | Independent peer sessions               | Agent sidebar; optional persistent tile/grid layout | Full conversation, remote environment and diff/file navigation   | Add comparison tiles later, not as the default          |
| Codex app      | Independent peer threads                | Threads grouped by project                          | Transcript plus integrated diff/review and editor handoff        | Put work history and change review together             |
| Antigravity    | Independent asynchronous agents         | Dedicated Manager Surface                           | Editor/terminal/browser work summarized by commentable artifacts | Lead completed review with artifacts, retain logs below |
| GitHub Copilot | Independent isolated sessions           | Repository-grouped sidebar or global Agents panel   | Durable log/overview, progress, usage, steering and file changes | Make child history persistent and auditable             |

### Viable laplus graphical layouts

#### Alternative A — navigate into a full child session

Clicking a child replaces the parent transcript with that child's normal
conversation/work view. A breadcrumb such as `Parent › reviewer` and sibling
controls provide the return path. This most closely matches Devin's semantic
model and makes maximum room for long commands, diffs, and results. Its cost is
context switching: the assignment that caused the delegation is no longer
visible while inspecting the child.

#### Alternative B — resizable right-hand child inspector

Keep the parent conversation visible on the left and open the selected child's
complete live stream on the right. The parent delegation card becomes a
compact index for switching siblings. The inspector can lead with a completed
result/diff summary, then show chronological prose, commands, reads, edits,
tools, errors, and result. At narrow widths it becomes a full-screen drawer.
This combines Devin's enterable child session with Codex's in-context review
without losing the delegation context.

#### Alternative C — optional tiled child workspace

Open two or three child sessions in persistent tiles, inspired by Cursor, with
the parent available as another tile or via breadcrumb. This is excellent for
comparing implementations or watching several long-running children, but it
is visually expensive and makes each work stream too narrow for routine use.

### GUI recommendation

Build **Alternative B, the resizable inspector, as the default**, with
Alternative A as its narrow-window/full-focus state. The selected child must
render the same rich entries as a primary session and end with its full result;
the parent card remains the status-and-navigation index. Add Devin-style
hierarchy in that index and artifact/diff-first review for completed children.
Treat Cursor-style tiling as a later power-user enhancement only after the
single-child inspector and persisted replay are sound.

For laplus specifically, this inspector should be a first-class surface in the
existing thread-scoped right-panel workspace, alongside files, diffs,
terminals, previews, and plans. The existing subagent list is the launcher:
selecting a child opens or activates that child's stable `agent:<child-id>`
surface as a normal browser-style tab. Several child tabs may remain open beside
files, diffs, terminals, previews, and plans, using the workspace's existing tab
activation, ordering, and close behavior. Each surface retains its own replay
and scroll position. It should reuse the main agent's message and work-entry UI
rather than introduce a reduced log renderer. File and diff actions from the
child stream may open neighboring right-panel surfaces without losing the
child tab.

Closing a child surface is presentation-only. It removes the right-panel tab
but does not interrupt, cancel, detach, or otherwise change the subagent or its
parent turn. The inline subagent row remains available to reopen the same child
stream. Stopping work uses the parent Stop action (and, only in a later
capability-gated version, an explicit child Stop action), never the tab close
button.

Open child surfaces participate in the existing thread-scoped right-panel
persistence model. Reloading or restarting restores their tab order and active
selection, then replays each persisted child stream. A child that can no longer
be resolved keeps an explicit unavailable surface instead of disappearing
silently.

The child surface has no additional pinned identity/task header. It opens
directly into the work stream, matching the main-agent presentation. The tab
uses the right-panel workspace's existing tab conventions without new status
decoration or special controls. The inline launcher remains the place where
identity, state, and the full assignment are summarized.

Starting or updating a subagent never opens the right panel automatically.
Only an explicit click on its inline launcher opens or activates the child
surface, so delegated work cannot steal focus from the parent conversation.

An open child surface follows new entries only while its viewport is pinned to
the bottom. Scrolling upward suspends follow mode and uses the main
conversation's jump-to-latest affordance when more work arrives. Every child
surface preserves its own scroll position while the user switches among tabs.

When a child delegates another child and the provider exposes that relationship,
the nested child's inline launcher appears in the spawning child's work stream;
clicking it opens another ordinary right-panel surface. The root transcript does
not duplicate descendants. Laplus preserves only hierarchy it can prove from
provider IDs or paths and does not invent parentage when metadata is absent.

The first version is observational: it deliberately has no composer and does
not imply that every provider can steer an individual child. “Same UI as the
main agent” means the same transcript and work renderers, not the same input
controls. It must still reflect interruption and other lifecycle changes on
each affected child accurately.

The initial interruption rule follows the parent boundary: stopping the parent
stops its delegation tree, and every affected child stream records its terminal
state. Per-child Stop is a later capability-gated control, not part of the
read-only first version; a provider that cannot address one child must never
pretend that it can. Children do not silently continue editing after the user
has stopped the parent.

This recommendation is not simply "use a side panel." It is a two-level
information architecture:

```text
parent delegation point
  └─ compact nested child index (identity, assignment, state, latest activity)
       └─ selected child session (all live work, result, diffs/artifacts, replay)
```

That is the smallest graphical design that preserves the relationship, makes
the work genuinely inspectable, and keeps the parent conversation available.

## Laplus today and the concrete gap

Laplus already has a correct lifecycle distinction: the server creates a
stable row keyed by the subagent task ID instead of conflating the short-lived
spawn call with the long-lived worker. It selects final summary, live agent
prose, then provider description as progressively weaker detail. However, its
visible title is only `Subagent <type>` or `Subagent task`, and the payload does
not expose a richer display name, assigned task, hierarchy/path, latest typed
activity, model, or usage
([server mapping](../../server/crates/laplus-server/src/worklog.rs)).

On the client, `collab_agent_tool_call` takes the generic hammer icon and flows
through `SimpleWorkEntryRow`: a one-line heading plus truncated preview with an
optional generic expanded raw body. It lives among ordinary Work Log entries,
and older entries can be folded behind the previous-log-entries affordance
([timeline renderer](../../apps/web/src/components/chat/MessagesTimeline.tsx)).

So the current implementation answers only “a subagent exists.” It does not
reliably answer:

- which worker this is, what it was assigned, or where it sits in a hierarchy;
- which workers are still running, blocked, completed, failed, or interrupted;
- how to enter a running worker and watch its actual work stream;
- what the child said, which commands it ran, what it read, edited, or called,
  and any errors, in chronological context;
- what result it returned, or why it produced no result;
- how to replay the child session, including its result, after the parent moves
  on or the page reloads.

## Recommended laplus layout

This is synthesis from the products above, not a claim about an upstream
protocol.

### 1. Make every child row a launcher into live work

Place a compact group at the point of delegation, but treat every row as a
stable link to that child's own stream:

```text
Subagents                                      2 running · 1 done
  ◔ reviewer      Review auth boundaries                  Open ›
  ◔ test-scout    Find missing coverage                   Open ›
  ● researcher    Compare upstream behavior              Review ›
```

Each row should expose semantic identity, assigned task, explicit state,
elapsed/terminal time, and an unambiguous Open/Review affordance. A recent
activity line can help scanning, but it must not substitute for the session.
Use animation only for live state, retain terminal rows, and summarize parallel
counts in the group header. The card is primarily navigation and overview.

### 2. Make a dedicated live inspector the core experience

Clicking a child should open a side drawer or split pane modeled directly on
OpenCode. On the web, use available height rather than copying the terminal's
literal 14 rows, while preserving its bounded, independently scrollable,
streaming behavior:

```text
┌ Parent conversation ───────────────┬ reviewer · running ───────────────┐
│                                    │ Review auth boundaries             │
│ Subagents            2 running     │────────────────────────────────────│
│ ◔ reviewer        selected         │ I’ll trace authentication first…   │
│ ◔ test-scout                         │ READ  server/src/auth.rs            │
│ ● researcher                         │ $ rg "refresh_token" server         │
│                                    │ TOOL  search_code     ✓             │
│ Parent messages continue…          │ EDIT  auth.rs  +12 −3   View diff › │
│                                    │ ERROR test failed: expected 401…    │
│                                    │ I found the missing boundary…       │
│                                    │                         Live ↓      │
│                                    │ ‹ Previous   Next ›   Close         │
└────────────────────────────────────┴────────────────────────────────────┘
```

The inspector should include the header/task and chronological child stream,
reusing laplus's normal message/work-entry renderers. Required content types
are child prose, commands with output/status, file reads/searches, edits with
viewable diffs, other tool calls and results, errors, and the final Markdown
result. The result is the terminal stream entry, not a detached replacement
for the work that led to it. Include sibling navigation and parent return.

Append in real time. If the user is at the bottom, keep the view sticky as
OpenCode does; if they scroll upward, preserve position and offer “Jump to
live.” Render loading, no-activity-yet, disconnected, interrupted, failed, and
completed states explicitly. Do not interleave full child transcripts into the
parent flow: concurrent work would make both contexts unreadable.

For terminal children, append the full result when present; otherwise append
the error, interruption, or `Completed — no result returned`. Reopening a child
must show the same ordered session ending in that outcome.

### 3. Route, persist, and replay child events as child events

An OpenCode-like inspector is impossible if child work is collapsed into the
parent's generic work log. At ingestion, route each event by stable child
session/task ID and parent ID:

```text
provider event
  ├─ parent session event ──> parent transcript
  └─ child session event  ──> child stream store ──> live inspector
                                      └────────────> persisted replay
```

Preserve ordered entries for prose, command invocation/output/exit, file reads
and searches, edits/patches/diffs, tool calls/arguments/progress/results,
warnings/errors, and the terminal result/failure/interruption. The parent keeps
only lifecycle/index events and links to the child, not duplicate child work.

Persist timestamps, stable event IDs, ordering, and stream completion state so
reconnect/reload can replay history and subscribe after the last seen event
without loss or duplication. If a provider exposes a separate transcript,
store its reference and hydrate it; if it only exposes tool-scoped progress,
normalize that into the same child stream model.

### 4. Preserve hierarchy without forcing a tree everywhere

Use provider path/parent IDs as durable identity and show the path in the
inspector or secondary text. For the common single-level case, a flat sibling
group is faster to scan. When nested agents exist, indent children or expose a
tree in the inspector/list; do not encode hierarchy only in generated names.
Codex's canonical paths are a good protocol/UI bridge, while Gemini's flat
group demonstrates the simpler presentation when recursion is impossible.

### 5. Make state and content provider-complete, not provider-identical

Normalize a small shared model:

```text
id, parentId/path, displayName, role/type, assignedTask, status,
stream[{ eventId, sequence, timestamp, contentType, payload }],
finalResult, terminalReason, model/effort, usage
```

Render absent fields gracefully. Codex can support navigable child transcripts
and hierarchy; Gemini/Claude/pi may initially provide only a tool-scoped
activity stream and result. A richer provider should not be reduced to the
lowest common denominator, but the basic card should remain consistent.

## Suggested delivery slices

The feature's first release covers all three native drivers: Claude, Codex, and
OpenCode. Implementation may proceed as provider-specific vertical slices, but
an OpenCode-only or otherwise partial slice is not the releasable product.
Providers may expose different detail; the UI degrades honestly rather than
inventing events or reducing richer providers to the weakest one.

Persist each normalized child stream for the lifetime of its parent thread and
delete it with that thread. Historical inline launchers therefore never point
at an intentionally expired work view, and restored child tabs can replay the
same ordered content after restart.

The parent thread stores only each child's lightweight identity, assignment,
status, and stream reference. The client fetches and replays a full child stream
only when its surface opens, subscribes while viewing it, and releases that
loaded view independently of server-side recording. A closed surface therefore
does not stop capture, and opening a large thread does not eagerly hydrate every
historical child.

The parent transcript retains one compact inline row per direct child. It shows
identity/type, assignment, state, and the latest meaningful activity while the
child is live; terminal state replaces that activity with a bounded result or
failure preview. The row is the launcher for the full surface. Detailed child
messages and work entries exist only in the child stream and are not duplicated
into the parent transcript.

Child completion produces no additional toast or desktop notification. Its
inline row moves to the terminal state, its stream records the outcome, and the
parent agent continues its orchestration normally. This avoids notification
bursts when several children finish together.

When a provider attributes an approval or user-input request to a child, the
request is recorded in that child's read-only stream, but the actionable control
uses the main conversation's existing approval/input surface and names the
waiting child. The response routes back through the child's provider request
identity. This matches OpenCode's descendant-request behavior and prevents a
blocker from remaining hidden in a closed or inactive child tab. Providers that
cannot attribute a request to a child retain their existing root behavior.

The feature does not migrate or backfill historical subagent rows. Full child
surfaces are guaranteed only for work captured after the child-stream model is
introduced; older transcript rows retain their existing behavior.

A thread remains **Working** while any descendant subagent is active, even if
the parent turn is temporarily quiet or has settled. Inline rows and open child
surfaces continue updating until every child reaches a terminal state. Thread
status therefore reflects the whole delegation tree rather than root output
alone.

1. **Child event routing and storage:** stop flattening child events into the
   parent transcript; key ordered events by stable child ID, persist them, and
   support replay followed by live subscription without duplicates.
2. **Live inspector:** render child prose, commands, reads, edits/diffs, tool
   calls, errors, and terminal result with normal entry components. Include
   sticky-live behavior, scrollback, empty/disconnected states, sibling cycling,
   and parent return.
3. **Subagent launcher/group:** replace `Subagent 1` with semantic identity,
   task, state, aggregate counts, and Open/Review navigation. A latest-activity
   line is optional supporting context.
4. **Hierarchy and richer metadata:** nested paths/tree, model/effort,
   files/diffs, usage/cost, resume/send/interrupt controls where provider
   semantics safely support them.

The first two slices are the product: users can watch actual child work while
it happens and revisit it later. The compact card matters because it opens that
experience.

Acceptance should cover the full lifecycle: opening a running child replays its
ordered history and streams new prose, commands, reads, edits/diffs, tool calls,
and errors; scrolling up disables auto-follow until “Jump to live”; switching
siblings preserves position; completion appends the full result; failure,
interruption, and empty success append explicit terminal entries; child work
does not pollute the parent transcript; and reload/revisit replays the same
ordered stream before continuing live without gaps or duplicates.
