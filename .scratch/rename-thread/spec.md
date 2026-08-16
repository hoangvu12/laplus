Status: ready-for-agent

# Automatic thread titles

Evidence and provenance: `.scratch/rename-thread/research.md`, T3 Code upstream
commit `f5fce74169a5629f701aeb8c4535cab6f7bd3c92`, and Laplus commit
`6dcb58b6c55a30d07aa1b79863c3f7f407f5ef7f`.

## Problem Statement

Laplus gives a new thread a useful but literal title derived from the first
message. That seed remains the title for Claude, Codex, Cursor, and Grok unless
the developer edits it manually. Long prompts are truncated rather than
summarized, and threads that begin with similar instructions are hard to tell
apart later.

T3 Code treats the first-message title as provisional. Its server asks the
configured text-generation provider for a short descriptive title, commits the
result without blocking the agent turn, and protects a title the developer has
changed while generation was running. It can repeat the same operation over an
existing conversation on request. It also adopts native title notifications
from Codex and OpenCode.

Laplus currently implements only the OpenCode notification path. The configured
text-generation model already declares thread titles as an intended use but is
not connected to thread orchestration, and Codex title notifications are
ignored.

## Solution

Match T3 Code's observable automatic-title behavior in vertical slices.

After the first user turn begins, Laplus asynchronously generates a concise
title through the configured text-generation provider and publishes it through
the existing thread metadata stream. Generation must not delay or fail the
conversation. Before committing, the server verifies that the provisional title
has not changed; a manual rename or newer title therefore wins over a stale
generation result.

The developer can later request title regeneration. Laplus generates from the
existing conversation and current title, exposes pending state, prevents
overlapping requests from racing, and commits only the result that still owns
the request. Failure leaves the current title intact and is reported through the
normal command and activity surfaces.

Provider-native names are normalized into the same metadata update. OpenCode's
existing `session.updated` behavior remains unchanged; Codex
`thread/name/updated` gains equivalent ingestion.

## User Stories

1. As a developer starting a Claude thread, I want its literal first-message
   seed replaced by a concise generated title, so that I can find it later.
2. As a developer starting a Codex thread, I want the same automatic-title
   behavior, so that thread naming does not depend on the conversation provider.
3. As a developer using any configured conversation provider, I want title
   generation to use my configured text-generation provider, so that naming and
   conversation execution can be configured independently.
4. As a developer, I want the first turn to start immediately while its title is
   generated, so that cosmetic work does not delay the agent.
5. As a developer, I want title-generation failure to leave the conversation
   usable with its provisional title, so that naming cannot break my work.
6. As a developer who manually renames a thread while generation is pending, I
   want my title preserved, so that a late background result cannot overwrite an
   explicit choice.
7. As a developer with two title requests close together, I want only the newest
   valid result committed, so that completion order cannot decide the title.
8. As a developer with multiple windows open, I want an automatic title to
   appear in every window, so that all views converge on the same thread.
9. As a developer reopening Laplus, I want the automatic title retained, so that
   it is durable metadata rather than local presentation state.
10. As a developer viewing thread history, I want an automatic title update to
    use the existing metadata event, so that lists, headers, search, and command
    palette agree.
11. As a developer, I want to regenerate an unhelpful title from the current
    conversation, so that the name can reflect how the work evolved.
12. As a developer requesting regeneration, I want visible pending state, so
    that repeated clicks do not create ambiguous requests.
13. As a developer whose regeneration fails, I want the old title preserved and
    the failure explained, so that I lose neither context nor control.
14. As a developer who manually renames during regeneration, I want that edit to
    supersede the generated result, so that explicit intent remains strongest.
15. As a Codex user, I want a native `thread/name/updated` notification reflected
    in Laplus, so that the provider and application use the same name.
16. As an OpenCode user, I want existing native session-title adoption to keep
    working, so that this feature does not regress supported behavior.
17. As a developer using an older or incapable text-generation provider, I want
    the provisional title retained without disrupting the turn, so that version
    skew degrades safely.
18. As a developer resuming a historical thread, I do not want first-turn
    generation to run again, so that reopening work cannot unexpectedly rename
    it.
19. As a developer starting an attachment-only thread, I want its provisional
    fallback to remain useful until generation succeeds, so that it is never
    blank.
20. As a developer, I want generated and provider-native titles to obey the
    existing trimmed, non-empty title rules, so that automatic updates cannot
    create an unfindable thread.

## Implementation Decisions

- T3 Code commit `f5fce74169a5629f701aeb8c4535cab6f7bd3c92` is authoritative
  for observable first-turn generation, regeneration, supersession, and native
  provider-title behavior. Laplus re-expresses the server behavior in Rust.
- “Automatic title” means either server synthesis or a provider-native title
  notification. The first-message title seed is provisional input, not an
  automatically generated result.
- First-turn generation runs asynchronously after the first user turn is
  accepted. It cannot gate prompt dispatch, streaming, settlement, or resumption.
- Generation uses the configured text-generation provider rather than assuming
  that the conversation provider can or should name the thread.
- The existing thread metadata command, event, fold, projection, and persistence
  remain the single write path for titles. Automatic sources do not introduce a
  second title store or client-only override.
- A generated result carries enough expected-state identity to prove that the
  provisional title or regeneration request still owns the write. A manual
  rename, native provider update, or newer generation supersedes an older result.
- First-turn generation runs once for a newly started conversation, not when a
  historical conversation is resumed or replayed.
- Regeneration is an explicit metadata operation over existing thread history.
  Its pending state is part of canonical thread state so all connected clients
  agree whether a request is active.
- Only one regeneration owns completion at a time. Starting a newer request
  invalidates an older result; a failure clears only the request it belongs to.
- Codex `thread/name/updated` and OpenCode `session.updated` are normalized to
  the same title semantics. OpenCode's already implemented path is preserved.
- Blank or whitespace-only generated/provider titles are ignored or refused at
  the existing validation boundary and never replace a usable title.
- The action to regenerate uses the existing shared thread-action policy so its
  availability and pending state do not drift between sidebar and chat header.
- No agent-visible tool for explicitly renaming the Laplus thread is added.
  Current upstream has no such tool; provider-native autonomous naming is a
  protocol event rather than a tool call.

## Testing Decisions

- The primary seam is a real socket conversation: start a first turn, observe
  the title event and shell/thread projections, then reconnect and observe the
  persisted title. Tests assert external behavior rather than reactor, fold, or
  database internals.
- The first tracer bullet covers a successful generated title, generator
  failure that does not block the turn, another connection receiving the title,
  persistence after restart, and a manual rename winning while generation is
  delayed.
- Regeneration tests drive the public command over the socket and cover pending
  state, success, failure, overlapping requests, and a manual rename
  superseding an in-flight result.
- Codex coverage replays the provider's real `thread/name/updated` wire shape and
  observes the normal thread metadata event and durable projection.
- Existing socket-renaming tests are prior art for title validation,
  cross-connection publication, fresh subscribers, and restart persistence.
  Existing OpenCode socket tests are prior art for provider-native title
  ingestion.
- Focused contract/client tests cover new wire fields and UI action policy, but
  do not duplicate behavior already proven at the socket seam.
- Completion includes driving a real window: start a Claude conversation and
  watch its title improve without manual editing; regenerate it; then manually
  rename while a generation is pending and confirm the manual title remains.

## Out of Scope

- Giving Claude, Codex, OpenCode, or another agent an explicit application tool
  whose operation is “rename this thread.”
- Replacing or removing manual thread rename.
- Generating project titles, branch names, commit messages, or pull-request
  titles beyond existing behavior.
- Changing title search, sorting, truncation, or display presentation except for
  regeneration action and pending/error state.
- Rewriting OpenCode native title ingestion that already works.

## Further Notes

The research found three independently useful slices. Automatic first-turn
generation provides the requested Claude behavior. Regeneration reuses that
capability but adds canonical pending and concurrency semantics. Codex native
event ingestion is smaller and independent, while OpenCode parity already
exists.
