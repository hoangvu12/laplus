# Automatic thread-title research

Date: 2026-08-11

## Corrected scope and verdict

This report concerns a title changing **without the user manually editing it**.
It distinguishes four mechanisms because “the agent renamed the thread” can
otherwise describe very different behavior.

**T3 upstream does automatically rename threads after conversation starts.** It
has two independent paths:

1. its server generates a better title from the first user message, using the
   configured text-generation provider; and
2. provider-native title events (currently Codex `thread/name/updated` and
   OpenCode `session.updated`) are normalized into a thread metadata update.

Upstream also offers a user-triggered **Regenerate title** action which uses the
server's generation path over existing thread history. None of these is an
agent-visible Laplus/T3 tool for explicitly calling “rename this thread.”

**Laplus is only partially equivalent.** It seeds a title from the first prompt
and it accepts OpenCode's native `session.updated` title. It does not implement
upstream's server-generated first-turn replacement or regeneration workflow,
does not consume Codex `thread/name/updated`, and has no equivalent automatic
title path for Claude. In particular, a Claude conversation in Laplus keeps its
client seed unless the user manually renames it.

## Provenance

Laplus's configured remote is
[`hoangvu12/laplus`](https://github.com/hoangvu12/laplus). The upstream project
identified by this repository is
[`pingdotgg/t3code`](https://github.com/pingdotgg/t3code), so upstream was read
directly rather than inferred from `origin`.

Compared snapshots:

- T3 upstream
  [`f5fce74169a5629f701aeb8c4535cab6f7bd3c92`](https://github.com/pingdotgg/t3code/tree/f5fce74169a5629f701aeb8c4535cab6f7bd3c92),
  fetched 2026-08-11.
- Laplus
  [`6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f`](https://github.com/hoangvu12/laplus/tree/6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f),
  local `HEAD` and `origin/main` during research.

## The four mechanisms

### 1. Initial title seed

This is not agent renaming. Before the provider has replied, the UI derives a
title from the submitted text (or image/terminal/element context), truncates it,
and sends it as the thread title and `titleSeed`. Laplus does this in
[`ChatView.tsx`](https://github.com/hoangvu12/laplus/blob/6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f/apps/web/src/components/ChatView.tsx#L4661-L4676)
and carries the seed into the first turn. If no usable title reaches the Rust
server, it falls back to the project title in
[`orchestration.rs`](https://github.com/hoangvu12/laplus/blob/6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f/server/crates/laplus-server/src/orchestration.rs#L2099-L2115).

Upstream receives the same seed, but treats it as provisional for the first-turn
generation described next.

### 2. Server-generated title

On the first user-message turn, upstream checks that the current title is still
replaceable (so it does not overwrite a later manual rename), asynchronously
calls `generateThreadTitle`, rechecks the title, and dispatches a server-owned
`thread.meta.update` with the generated result. See the
[first-turn trigger](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L1097-L1125)
and
[generation/update function](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L856-L898).

This generation is provider-independent orchestration backed by the configured
text-generation instance. Upstream implements `generateThreadTitle` adapters
for
[Claude](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/textGeneration/ClaudeTextGeneration.ts#L341-L366),
[Codex](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/textGeneration/CodexTextGeneration.ts#L382-L412),
[OpenCode](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/textGeneration/OpenCodeTextGeneration.ts#L597-L622),
[Cursor](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/textGeneration/CursorTextGeneration.ts#L241-L266),
and
[Grok](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/textGeneration/GrokTextGeneration.ts#L233-L258).
Thus the title-writing model may be distinct from the provider handling the
conversation.

Upstream's user-triggered regeneration is the same server synthesis over
existing history: `thread.meta.update { regenerateTitle: true }` records pending
state, the reactor generates against recent messages and the previous title,
then an internal `thread.title.regeneration.complete` conditionally commits the
result. The contract and concurrency guard are in
[`orchestration.ts`](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/packages/contracts/src/orchestration.ts#L751-L767)
and the
[reactor](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/orchestration/Layers/ProviderCommandReactor.ts#L901-L966).

Laplus has none of this path. Its settings explicitly say the configured text
generation model is stored but “nothing reads it yet” for thread titles in
[`config.rs`](https://github.com/hoangvu12/laplus/blob/6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f/server/crates/laplus-server/src/config.rs#L430-L447).
Its contract has neither `regenerateTitle`, title-regeneration pending state,
nor the internal completion command.

### 3. Provider-native title event

This is the provider deciding or generating its own session/thread name, then
the host adopting it.

- Upstream Codex maps `thread/name/updated` to
  `thread.metadata.updated` in
  [`CodexAdapter.ts`](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/provider/Layers/CodexAdapter.ts#L980-L999).
- Upstream OpenCode maps a non-empty `session.updated` `info.title` to the same
  normalized event in
  [`OpenCodeAdapter.ts`](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L807-L824).
- The shared upstream ingestion layer turns either normalized event into
  `thread.meta.update`, which makes the provider's name the application title:
  [`ProviderRuntimeIngestion.ts`](https://github.com/pingdotgg/t3code/blob/f5fce74169a5629f701aeb8c4535cab6f7bd3c92/apps/server/src/orchestration/Layers/ProviderRuntimeIngestion.ts#L1894-L1901).

Laplus implements only the OpenCode variant. Its driver folds
`session.updated.info.title` directly into `Change::MetaUpdated` in
[`opencode.rs`](https://github.com/hoangvu12/laplus/blob/6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f/server/crates/laplus-server/src/opencode.rs#L1637-L1655),
and its socket test sends a real-shaped `session.updated` title and asserts the
resulting thread event in
[`socket_opencode_turn.rs`](https://github.com/hoangvu12/laplus/blob/6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f/server/crates/laplus-server/tests/socket_opencode_turn.rs#L811-L823).

Laplus's Codex driver has no `thread/name/updated` handling, and its Claude
driver has no provider title-event mapping. Repository-wide, the only provider
that constructs `Change::MetaUpdated` is OpenCode; the other occurrences are
the user/client command fold and tests. Therefore:

| Conversation provider | Upstream automatic title routes                                                 | Laplus automatic title routes   |
| --------------------- | ------------------------------------------------------------------------------- | ------------------------------- |
| Claude                | Server first-turn generation; manual regeneration                               | None beyond initial client seed |
| Codex                 | Server first-turn generation; manual regeneration; native `thread/name/updated` | None beyond initial client seed |
| OpenCode              | Server first-turn generation; manual regeneration; native `session.updated`     | Native `session.updated` only   |
| Cursor                | Server first-turn generation; manual regeneration                               | None beyond initial client seed |
| Grok                  | Server first-turn generation; manual regeneration                               | None beyond initial client seed |

### 4. Agent explicitly invokes rename

Neither source tree exposes a provider/agent tool whose semantic operation is
“rename this application thread.” `thread.meta.update` is an orchestration
command used by the UI/server and provider-ingestion layers, not a tool supplied
inside the Claude/Codex/OpenCode agent session. A provider may autonomously emit
its own native title notification (mechanism 3), but that is not the assistant
choosing to call a Laplus rename tool.

## Port assessment

This is a real Laplus gap, especially for the requested Claude case. It is not a
1:1 UI copy: the upstream implementation crosses contract, settings capability,
text-generation adapters, orchestration reactor, durable pending state,
concurrency/supersession behavior, and UI status/action policy.

The smallest valuable tracer bullet is **automatic first-turn generation for
Claude using Laplus's configured text-generation model**, guarded so a slow
generation cannot overwrite a user rename. Reusing upstream's full design would
then add explicit regeneration and the remaining provider adapters. Codex native
`thread/name/updated` ingestion is a separate, smaller provider-parity slice;
OpenCode native title adoption already exists.
