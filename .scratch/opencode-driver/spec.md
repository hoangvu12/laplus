# OpenCode as a full Laplus provider

Status: ready-for-agent

Evidence and provenance: `.scratch/opencode-driver/upstream-research.md` records
the T3 Code behavior pinned at commit `0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62`
and the disposable live-protocol prototype against OpenCode 1.18.10. The
accepted OpenCode decisions are ADR-0035 through ADR-0044, with the shared MCP
boundary in ADR-0030. Where pinned T3 behavior and the current live protocol
differ, the recorded compatibility behavior for 1.18.10 is authoritative.

## Problem Statement

Laplus presents OpenCode in parts of its contract and interface, but the server
cannot configure, discover, start, connect to, or converse through it. A
developer who has OpenCode installed or operates an OpenCode server therefore
cannot select one of its models and use it as the agent behind a thread. The
visible controls describe a provider that the runtime does not actually have.

The absence is broader than one transport. The server's provider registry is a
closed set of built-in instances, continuation is stored as a Claude/Codex-shaped
string, and generic provider maintenance and several supporting contract
surfaces are not yet implemented. Adding a hard-coded OpenCode slot would make
the immediate prompt path work while preserving the structural limitations
that prevent multiple independently configured instances and richer provider
resume cursors.

OpenCode also has behavior that cannot be safely approximated as terminal
output. T3 Code starts or connects to an HTTP server, sends commands over its
API, and consumes a directory-scoped SSE event stream. Current OpenCode sends
both true text deltas and cumulative part updates, can accept a steer during an
active turn, and exposes sessions whose history and canonical working directory
must be verified during recovery. Treating a transport, authentication, or
cursor error as a missing session would silently replace durable context with
an empty conversation.

## Solution

OpenCode becomes a complete implementation of Laplus's existing driver seam.
A developer can configure one or more OpenCode provider instances, use either a
Laplus-owned loopback server or an operator-owned external HTTP endpoint,
discover the models and agents available to each instance, select an instance
and model, and hold a conversation through the same thread and session surface
used by Claude and Codex.

Each owned conversation gets an `opencode serve` process. Each external
conversation connects to its configured endpoint without claiming ownership of
that endpoint's lifetime or transport security. A narrow, handwritten HTTP/SSE
client owns only the OpenCode routes and event shapes Laplus uses. The OpenCode
driver translates those shapes into the shared conversation changes, including
streaming text and reasoning, tools, permissions, questions, status, errors,
titles, steering, interruption and durable continuation.

The provider-instance registry becomes genuinely generic before OpenCode is
registered. Claude and Codex migrate into compatible default instances, while
OpenCode may have several instances with independent configuration,
credentials, catalogues, maintenance state and continuation namespaces.
Continuation becomes an opaque, versioned provider resume cursor without
stranding existing Claude and Codex threads.

The solution also covers the T3-compatible surrounding behavior needed for
full support: shared-filesystem chat attachments, owned-server MCP registration
once the platform MCP surface exists, checkpoint rollback of provider history,
short structured text generation, and explicit provider maintenance. The
highest automated acceptance seam is the existing WebSocket/session boundary
driven against a scripted OpenCode HTTP/SSE peer; protocol fixtures sit below
that seam.

## User Stories

1. As a developer with OpenCode installed, I want to enable it in Laplus, so
   that I can use an agent I already configured and authenticated.
2. As a developer, I want OpenCode installation failures distinguished from
   authentication, connection and version failures, so that I know what to fix.
3. As a developer, I want Laplus to use a configured OpenCode binary path, so
   that a non-standard installation works.
4. As a developer, I want Laplus to reject an unsupported local OpenCode
   version clearly, so that protocol incompatibility is not presented as a
   broken conversation.
5. As a developer, I want an owned OpenCode server bound to loopback, so that
   Laplus does not expose it to the network by default.
6. As a developer, I want Laplus to stop the owned server when its conversation
   ends, so that abandoned agent processes do not accumulate.
7. As a developer, I want startup failure and timeout details surfaced, so that
   a server that never becomes ready does not leave a session stuck at starting.
8. As an operator, I want to configure an external OpenCode server URL, so that
   Laplus can use infrastructure I manage.
9. As an operator, I want external URLs to support HTTP and HTTPS, so that LAN,
   VPN and reverse-proxy deployments remain possible.
10. As an operator, I want an optional external-server password sent using
    OpenCode Basic authentication, so that protected endpoints work.
11. As an operator, I want Laplus to leave an external server running when a
    conversation closes, so that client lifetime never becomes endpoint
    ownership.
12. As an operator, I want transport security for external endpoints to remain
    my responsibility, so that Laplus does not reject a network topology it
    cannot evaluate.
13. As a developer, I want several OpenCode provider instances, so that I can
    keep different endpoints, accounts or environments independently
    selectable.
14. As a developer, I want each provider instance to have a stable identity and
    display name, so that threads and model choices route to the intended
    configuration.
15. As a developer, I want Claude and Codex to remain available as compatible
    default instances after the registry migration, so that existing workflows
    do not change.
16. As a developer, I want existing threads to retain their provider routing
    after the registry migration, so that reopening them does not silently
    select another agent.
17. As a developer, I want settings changes validated at save time, so that an
    unusable provider instance is not accepted as working configuration.
18. As a developer, I want disabling one instance to leave other instances of
    the same driver unaffected, so that configuration is truly per instance.
19. As a developer, I want a refresh targeted at one provider instance, so that
    checking one endpoint does not disturb every agent.
20. As a developer, I want each OpenCode instance to show its actual health and
    version, so that the picker reflects the endpoint Laplus will use.
21. As a developer, I want models discovered from connected upstream providers,
    so that I select only models the OpenCode instance can use.
22. As a developer, I want model slugs to retain both upstream provider and
    model identity, so that similarly named models are unambiguous.
23. As a developer, I want configured custom models kept as fallback choices,
    so that discovery gaps do not erase an intentional configuration.
24. As a developer, I want visible primary OpenCode agents offered as model
    options, so that agent selection is available where OpenCode supports it.
25. As a developer, I want OpenCode model variants offered as options, so that
    I can select the behavior exposed by that model.
26. As a developer, I want local catalogue discovery to work without keeping a
    conversation server alive, so that settings and the composer can populate
    before I send a prompt.
27. As a developer, I want external catalogue discovery to use the configured
    HTTP endpoint, so that no local CLI is required merely to inspect it.
28. As a developer, I want to select an OpenCode instance and model in the
    composer, so that my next thread or turn uses that exact provider identity.
29. As a developer, I want an OpenCode conversation to run in my project's
    working directory, so that relative paths and tool work affect the expected
    workspace.
30. As a developer, I want my prompt sent as text parts through OpenCode's
    asynchronous prompt API, so that the interface stays responsive while the
    turn runs.
31. As a developer, I want assistant text to stream without duplication, so
    that receiving both delta and cumulative updates does not repeat content.
32. As a developer, I want reasoning to stream separately from assistant text,
    so that the transcript preserves the distinction OpenCode provides.
33. As a developer, I want late or out-of-order message role events handled, so
    that valid assistant content is not lost because events arrived in a
    different order.
34. As a developer, I want an older cumulative part update ignored when it
    would shorten rendered text, so that the transcript never moves backward.
35. As a developer, I want either OpenCode idle signal to settle a turn exactly
    once, so that current and pinned server variants cannot double-complete it.
36. As a developer, I want OpenCode title updates reflected on my thread, so
    that generated upstream titles remain synchronized with T3-compatible
    behavior.
37. As a developer, I want a busy, retrying, idle or failed OpenCode session to
    produce the corresponding Laplus status and activity, so that I can tell
    what the agent is doing.
38. As a developer, I want retry information visible as a warning, so that a
    slow recovery is not mistaken for a frozen turn.
39. As a developer, I want a structured OpenCode session error to fail the
    active turn with its reason, so that failures are actionable.
40. As a developer, I want to interrupt an OpenCode turn, so that I can stop
    unwanted work without destroying the thread.
41. As a developer, I want interruption to call OpenCode's abort operation and
    preserve already streamed content, so that the transcript matches what I
    saw.
42. As a developer, I want stopping a session to abort active work and release
    its owned resources, so that stop has a complete lifetime meaning.
43. As a developer, I want to send a steer while OpenCode is busy, so that the
    agent incorporates my correction immediately.
44. As a developer steering an active turn, I want the prompt to retain the
    active turn id, so that the transcript does not invent a second exchange.
45. As a developer, I want Claude and Codex follow-up prompts to retain their
    existing queued-turn behavior, so that OpenCode's steering capability does
    not redefine other drivers.
46. As a developer, I want command, file, web, MCP, image and collaboration tool
    activity represented in the shared work log, so that I can understand what
    OpenCode did.
47. As a developer, I want tool start, progress, completion and failure states
    preserved, so that long-running work remains intelligible.
48. As a developer, I want unknown tools to remain visible as generic dynamic
    tools, so that a new OpenCode tool is not silently discarded.
49. As a developer, I want raw upstream tool state retained where the shared
    vocabulary cannot express every detail, so that diagnostics do not lose the
    evidence.
50. As a developer in full-access mode, I want OpenCode permissions allowed, so
    that the selected mode does not stop for approval.
51. As a developer in any other runtime mode, I want OpenCode to ask before
    sensitive work, so that T3-compatible supervision is enforced.
52. As a developer, I want OpenCode's separate question capability allowed in
    supervised modes, so that questions reach the dedicated answer interface
    instead of becoming permission prompts.
53. As a developer, I want command, read and edit permission requests mapped to
    their specific shared request kinds, so that an approval has useful
    context.
54. As a developer, I want an unknown permission kind shown rather than
    dropped, so that I can still decide whether work proceeds.
55. As a developer, I want accept, accept-for-session, decline and cancel
    translated to valid OpenCode replies, so that every offered decision has
    the intended effect.
56. As a developer, I want resolved permission requests closed when OpenCode
    reports their reply, so that stale approval controls disappear.
57. As a developer, I want multi-question OpenCode requests rendered in their
    original order, so that my answers stay attached to the correct prompts.
58. As a developer, I want question answers and rejection sent through the
    OpenCode question API, so that the active turn can continue or stop waiting.
59. As a developer, I want pending request identity tracked explicitly, so that
    permission and question replies cannot be confused by naming conventions.
60. As a developer, I want chat attachments resolved from Laplus's local store
    and sent as file URLs, so that OpenCode can inspect files I attach.
61. As a developer, I want unresolved attachment references omitted safely, so
    that one missing file does not corrupt the whole prompt.
62. As an operator of an external server, I want the shared-filesystem
    requirement documented, so that I understand why a Laplus-local path may
    not be readable remotely.
63. As a developer, I want an owned OpenCode session registered with Laplus's
    per-thread MCP endpoint when that platform surface is available, so that
    the agent can call back into Laplus tools.
64. As an operator, I want Laplus not to register its MCP endpoint into an
    external OpenCode server, so that it does not mutate infrastructure it does
    not own.
65. As a developer, I want an OpenCode conversation to survive a Laplus restart,
    so that closing the application does not discard agent context.
66. As a developer, I want continuation stored as an opaque versioned provider
    resume cursor, so that driver-specific recovery can evolve without another
    storage redesign.
67. As a developer with an existing Claude or Codex thread, I want its legacy
    string continuation read as that driver's v0 cursor, so that migration does
    not strand it.
68. As a developer, I want a malformed or unsupported cursor reported as
    incompatible, so that lost context cannot masquerade as a successful fresh
    start.
69. As a developer, I want OpenCode to start fresh only after a structured
    missing-session response, so that authentication and network failures do
    not erase durable context.
70. As a developer, I want the stored cursor preserved when resume fails for any
    other reason, so that a temporary outage can be retried later.
71. As a developer whose upstream session was genuinely removed, I want Laplus
    to create a new session and replace the cursor, so that the thread can
    continue honestly.
72. As a developer reopening a session in the same directory, I want current
    permissions re-applied, so that recovery does not inherit stale access.
73. As a developer whose thread moved to another working directory, I want
    OpenCode history forked into the new directory, so that changing worktrees
    does not lose context.
74. As a developer, I want the fork's canonical directory verified, so that a
    successful response cannot hide an ineffective move.
75. As a developer using OpenCode 1.18.10 behavior, I want Laplus to follow an
    unmoved fork with move-session and verify again, so that recovery actually
    reaches the requested workspace.
76. As a developer, I want checkpoint revert to restore the working tree before
    rolling back OpenCode history, so that the visible files follow the chosen
    checkpoint even across the irreducible two-system boundary.
77. As a developer, I want provider history rolled back by the number of removed
    turns, so that OpenCode's context matches the retained transcript.
78. As a developer, I want later checkpoint references pruned only after
    provider rollback succeeds, so that a partial failure remains recoverable
    and is not reported as completion.
79. As a developer, I want rollback failure reported while the restored tree and
    later checkpoint references remain explicit, so that the partial state is
    not concealed.
80. As a developer, I want OpenCode to generate commit messages, pull-request
    text, branch names and thread titles, so that provider-backed naming works
    consistently outside conversations.
81. As a developer, I want each background generation request isolated in a
    temporary session with tools denied, so that naming work cannot mutate my
    project.
82. As a developer, I want structured generation output validated and sanitized
    for its destination, so that malformed model output cannot become an invalid
    branch name or title.
83. As a developer, I want local generation requests to share an idle-reaped
    OpenCode server, so that short operations do not pay process startup every
    time.
84. As a developer, I want the shared generation server stopped after thirty
    idle seconds, so that optimization does not become a permanent process.
85. As an operator, I want external OpenCode text generation to use the
    configured endpoint, so that it follows the same instance identity as
    conversations.
86. As a developer, I want available provider update actions shown for my
    OpenCode installation, so that I can maintain it from Laplus deliberately.
87. As a developer, I want update strategies derived from the resolved native,
    npm, pnpm, Bun, Vite+ or Homebrew installation, so that Laplus runs the
    appropriate command.
88. As a developer, I want provider updates to run only after an explicit
    request, so that probing never mutates my installation.
89. As a developer, I want updates serialized by provider instance and package
    manager, so that overlapping maintenance commands cannot corrupt an
    installation.
90. As a developer, I want the provider refreshed after maintenance and the
    detected version compared, so that success describes the observed result
    rather than the command's exit code alone.
91. As an operator using an external endpoint with a local binary path, I want
    maintenance to follow the configured local binary while the refreshed
    external snapshot remains authoritative, so that T3-compatible behavior is
    explicit even when the two versions differ.
92. As a maintainer, I want protocol changes exposed by captured golden
    fixtures, so that OpenCode drift is reviewed as a wire diff.
93. As a maintainer, I want unknown OpenCode event kinds observable but
    non-fatal, so that compatible server additions do not end conversations.
94. As a maintainer, I want malformed wire records distinguished from unknown
    valid events, so that transport corruption and forward compatibility are
    not conflated.
95. As a maintainer, I want HTTP status and structured error decoding
    centralized, so that resume, authentication and command failures use the
    same evidence.
96. As a maintainer, I want SSE cancellation to release the response and pump
    task promptly, so that session shutdown does not leak work.
97. As a maintainer, I want event delivery cancel-safe after dequeue, so that a
    cancellation cannot consume and lose a normalized event.
98. As a maintainer, I want OpenCode behavior tested at the existing
    WebSocket/session boundary against a scripted peer, so that tests prove
    externally visible behavior rather than internal call sequences.
99. As a maintainer, I want protocol tests and behavioral tests to run without a
    live OpenCode installation or network, so that CI remains hermetic.
100. As a maintainer, I want OpenCode to reuse the shared session loop, so that
     baselines, checkpoints, epochs, settling and published session events stay
     consistent across drivers.

## Implementation Decisions

### Provider instances and routing

- Implement the contract's generic provider-instance registry before
  registering OpenCode. A provider instance id is the durable routing key; the
  driver kind selects the implementation.
- Migrate Claude and Codex into compatible built-in default instances without
  changing the identity stored by existing threads.
- Permit multiple independently configured OpenCode instances. Configuration,
  credentials, environment, catalogue, maintenance state and continuation
  namespace belong to the instance.
- Provider snapshots, settings patches, refresh, model selection and thread
  creation route through instance identity. Disabling or refreshing one
  instance does not act on its siblings.
- OpenCode settings include enabled state, binary path, optional external server
  URL and password, and custom model fallback entries. Secret values are never
  echoed in snapshots or diagnostics.

### Discovery and catalogue

- A local instance resolves and probes its configured binary, enforces the
  accepted minimum compatible version, and discovers models and agents through
  the CLI when no conversation server exists.
- An external instance obtains health, version, connected upstream providers,
  models and agents from its configured HTTP API. It does not require a local
  CLI for ordinary use unless a separately configured maintenance action needs
  one.
- Only connected upstream providers contribute discovered models. Model slugs
  preserve `provider/model` identity. Visible primary agents and model variants
  become selection options.
- Configured custom models remain available as fallback entries rather than
  being erased by an incomplete discovery response.

### HTTP/SSE protocol ownership

- Own a narrow handwritten client over the repository's Rust HTTP stack and
  serde. Model only the routes, responses, errors and SSE events required by
  this specification.
- Treat OpenCode's pinned OpenAPI document and captured traffic as conformance
  evidence and upgrade inputs, not as a build-time code-generation dependency.
- Centralize base URL construction, directory binding, Basic authentication,
  JSON request handling, HTTP status classification, structured errors and SSE
  framing behind the protocol module's small operation-oriented interface.
- Preserve valid unknown event payloads and record them as drift without ending
  a session. Keep malformed SSE/JSON distinguishable from a well-formed unknown
  event.
- Run a dedicated event pump that filters events to the adopted OpenCode session
  and sends normalized driver events over a channel. Driver event retrieval
  performs no cancellable await after removing an event from that channel.
- Closing a session cancels the SSE request, stops the pump and releases every
  response and child-process resource it owns.

### Server lifetimes and transport ownership

- Without an external URL, start one owned OpenCode server per conversation on
  loopback with an ephemeral port and an empty injected OpenCode configuration.
  Wait for an explicit readiness result before opening the session.
- Session closure terminates and reaps the owned server and its process group,
  including escalation when graceful shutdown does not complete.
- With an external URL, connect using HTTP or HTTPS and optional OpenCode Basic
  authentication. Never start, stop, reconfigure or claim transport-security
  responsibility for that endpoint.
- Owned and external lifetimes share the same driver behavior after a client is
  acquired; ownership differences remain confined to acquisition and cleanup.

### Conversation opening and continuation

- Replace the provider-specific stored session string concept with an opaque
  versioned provider resume cursor. The persistence and orchestration layers do
  not interpret cursor JSON.
- Claude and Codex continue to read existing stored strings as their v0 cursor.
  Drivers reject malformed cursors and unsupported future versions as
  incompatible rather than treating them as absent.
- OpenCode's v1 cursor contains the upstream session id and enough versioning to
  validate ownership. A successful create or adoption writes the cursor back
  through the common continuation boundary.
- When a cursor is present, fetch that exact session. Start fresh only after a
  structured OpenCode missing-session response. Preserve the cursor and fail
  visibly on transport, authentication, authorization, decoding and other
  server errors.
- If the recovered session's canonical directory matches the thread, reapply
  current permissions before use.
- If its canonical directory differs, fork the session to preserve history,
  inspect the returned canonical directory, and adopt it only after it matches.
  When fork does not move it, use the current move-session operation and verify
  again. Never infer a move from HTTP success alone.

### Driver and turn behavior

- Implement OpenCode behind the existing driver seam. The shared session loop
  continues to own baselines, checkpoints, epochs, settling and contract event
  publication.
- Send prompts through asynchronous session prompting with the selected
  `provider/model`, optional agent and variant, text parts and resolved chat
  attachment parts.
- A prompt received during an active OpenCode turn is a steer: send it
  immediately to the busy session and retain the active turn id. Do not change
  Claude or Codex queued-follow-up behavior.
- Interrupt calls session abort and retains content already published. Stop
  aborts active work, closes the session scope and releases owned resources.
- Treat both status-idle and the standalone idle event as idempotent terminal
  signals. Busy begins/runs the turn; retry publishes a runtime warning;
  session error fails the active turn with the structured reason.
- Every non-empty OpenCode session title update becomes the thread title,
  matching T3 behavior even when it overwrites a manual rename.

### Event and work normalization

- Keep message-role, part and emitted-text state keyed by upstream ids because
  role and part events may arrive in either order.
- Handle true `message.part.delta` records directly. Reconcile cumulative text
  and reasoning updates by common prefix, emit only unseen suffixes, and refuse
  to shorten already emitted content when an older update arrives.
- Translate OpenCode tool lifecycle into the shared work-log vocabulary for
  commands, file work, web search, MCP calls, image viewing and collaboration.
  Preserve raw tool state and use the generic dynamic-tool representation for
  unclassified tools.
- Cache pending permissions and questions by upstream request id together with
  their explicit kind. Never infer the kind from an id prefix.
- Map bash, read and edit permissions to the corresponding shared request kinds;
  retain other permission kinds as visible unknown requests.
- Translate accept to a one-time permission, accept-for-session to an always
  permission, and decline or cancel to rejection. Resolve the shared request
  when OpenCode reports its reply.
- Preserve multi-question ordering and stable derived question identity when
  mapping to the existing user-input surface. Reply or reject through the
  corresponding OpenCode question operation.
- Support the original T3-compatible question event family. Treat newer
  question-v2 events as observable unknown events until their reply semantics
  are deliberately implemented and fixture-backed.

### Runtime modes, attachments and MCP

- Translate full access to allow every OpenCode permission. Translate the other
  three runtime modes to default-ask rules for sensitive operations while
  allowing the separate question capability. The middle modes intentionally
  remain equivalent for OpenCode.
- Reapply permissions on adoption and when the shared driver retune boundary
  moves the runtime mode between turns.
- Resolve chat attachments through Laplus's attachment store and send existing
  files as local `file://` URLs. Omit unresolved files. Do not add a byte-upload
  protocol for external servers.
- Register the per-thread Laplus MCP endpoint only with owned OpenCode servers
  and only through the generic MCP platform surface established separately.
  External servers receive no automatic MCP mutation.
- The OpenCode driver may land its basic conversation path before the generic
  MCP platform effort, but full MCP-backed behavior retains that explicit
  dependency rather than embedding a private MCP server in the driver.

### Checkpoint rollback

- Extend the shared checkpoint-revert flow with provider-history rollback for
  OpenCode. Restore the filesystem, refresh the workspace index, revert the
  OpenCode session by the removed turn count, prune later checkpoint references,
  then publish completion.
- The Git and provider operations are not claimed to be atomic. If provider
  rollback fails after filesystem restoration, report failure, leave the tree
  restored, retain later checkpoint references and do not publish false
  completion.

### Text generation

- Register OpenCode for the shared provider text-generation operations used by
  commit messages, pull requests, branch names and thread titles.
- Every operation uses a temporary OpenCode session with tool permissions
  denied, validates structured output and applies destination-specific
  sanitization.
- Local OpenCode instances share a server outside conversation lifetimes for
  this work and reap it after thirty idle seconds. This server never shares
  arbitrary conversation history.
- External instances use their configured server and the same isolated
  temporary-session behavior.

### Provider maintenance

- Implement provider maintenance as a generic instance-addressed contract
  surface, then register OpenCode's native, npm, pnpm, Bun, Vite+ and Homebrew
  strategies.
- Derive commands from the resolved installation and run them only after an
  explicit developer request. Serialize overlapping operations by instance and
  package manager.
- Refresh the provider after the command and report the observed before/after
  version. A successful command is not assumed to have changed the provider.
- Preserve T3-compatible external-instance behavior: a configured local binary
  may advertise and receive maintenance even though the authoritative refreshed
  snapshot still describes the external endpoint.

## Testing Decisions

- Good tests assert externally visible behavior and durable state, not concrete
  helper calls, task layouts, channel counts or private wire structs.
- The highest automated seam is the existing WebSocket/session boundary driven
  against a scripted OpenCode HTTP/SSE peer. Through that one seam, test provider
  routing, text/reasoning output, tools, permissions, questions, steering,
  interruption, status and errors, continuation, CWD migration, rollback and
  cleanup.
- Reuse the repository's socket harness and session/orchestration integration
  style as prior art. Assertions should inspect the snapshots, events, stored
  thread state and requests visible at the boundary a real client and OpenCode
  peer use.
- Give the scripted peer deterministic control of HTTP responses, SSE ordering,
  connection closure and cancellation. It must reproduce out-of-order role and
  part events, delta-plus-cumulative text, duplicate idle signals, structured
  missing-session errors, auth failures and fork-without-move behavior.
- Add golden protocol tests over redacted captured fixtures for every used
  request, response, structured error and event family. These tests pin framing
  and normalization and make upstream drift reviewable.
- Test valid unknown events as non-fatal and observable, and malformed SSE or
  JSON as distinct protocol drift/failure according to the settled decoder
  boundary.
- Test the provider-instance registry and settings migration through public
  settings, snapshot, refresh and thread-routing behavior. Include multiple
  instances of one driver and preservation of Claude/Codex default identities.
- Test cursor migration and storage with legacy Claude/Codex strings, OpenCode
  v1 JSON, malformed cursors, unsupported versions, structured missing sessions
  and transient failures that must retain the cursor.
- Test owned-process lifetime with controllable fake children: readiness,
  graceful stop, forced reap and isolation between conversations. Test external
  mode without asserting ownership actions.
- Test text generation through the generic operation boundary, including
  deny-all sessions, schema rejection, sanitization, shared-server reuse and
  idle reaping decisions without elapsed-wall-clock assertions.
- Test maintenance through the generic instance-addressed command boundary with
  fake resolved installations and commands. Assert serialization decisions,
  explicit-request gating, refresh and observed version outcomes.
- Keep all automated tests hermetic: no installed OpenCode, live model,
  authentication, internet access or external server is required.
- Focus verification per ticket on the smallest relevant suites, formatting,
  lint and type checks. A full workspace run remains CI's responsibility.
- Automated UI-driver verification is not required by this specification; the
  contract-facing WebSocket/session tests are its acceptance seam.

## Out of Scope

- Generating or vendoring a complete Rust SDK from OpenCode's OpenAPI document.
- Depending on an unofficial community OpenCode Rust SDK or adding a JavaScript
  sidecar solely to reuse the official TypeScript SDK.
- Driving OpenCode through terminal-output parsing or `opencode run --format
json`.
- Restricting operator-configured external HTTP endpoints to loopback or HTTPS,
  managing their certificates, or owning their lifecycle.
- Uploading chat attachment bytes to an external OpenCode server or defining a
  remote filesystem transfer protocol.
- Mutating an external OpenCode server to add Laplus's MCP endpoint.
- Building the generic MCP platform surface itself; OpenCode integrates with it
  when that separately specified prerequisite exists.
- Implementing the newer `question.v2.*` family before its reply behavior is
  researched, captured and explicitly specified.
- Reproducing every OpenCode API route, event or administration feature beyond
  the operations required here.
- Making OpenCode's steer semantics apply to Claude or Codex.
- Claiming atomic rollback across Git and the OpenCode server.
- Requiring automated UI-driver verification as part of this feature's
  acceptance.

## Further Notes

- T3 Code at the pinned commit is the behavioral reference whenever it has an
  answer. New factual or behavioral uncertainty should return to that source
  first; current OpenCode sources and live captures are used to identify and
  record compatibility divergences.
- Two live 1.18.10 findings are load-bearing: streaming must consume true part
  deltas without duplicating the final cumulative update, and CWD migration
  must verify the fork result and use move-session when the fork remains in its
  original directory.
- The implementation should be split next into dependency-ordered tracer-bullet
  tickets. Likely boundaries include the provider-instance registry and cursor
  migration, contract/settings prerequisites, attachment and MCP dependencies,
  the narrow protocol, the first text-turn slice, event/request normalization,
  durable recovery and rollback, and discovery/text-generation/maintenance.
  The ticketing pass must derive exact blocking edges from this specification
  rather than treating that list as fixed.
- Existing documentation changes and unrelated `.scratch/codex-skills/`
  content predate this specification and must be preserved.
