# ADR-0032 - One Codex app-server belongs to one conversation

Date: 2026-07-31
Status: Accepted

## Context

`codex app-server` can host several threads in one process. A reader who knows
the protocol could therefore reasonably expect laplus to start one process per
provider instance and multiplex every Codex conversation through it.

That is not the lifetime laplus already owns. A session is the process behind
one conversation. Session stop, epochs, shutdown and reaping all rely on that
boundary, and the shared session loop deliberately keeps those rules outside a
driver. Upstream also keeps one Codex runtime per thread, in part because its
per-thread MCP wiring is supplied when the process starts.

The provider probe is different work with the same executable: it starts an
app-server, asks for the handshake, account, models and workspace skills, then
reaps it without creating a conversation.

## Decision

**Each Codex conversation gets one app-server process, and a provider probe gets
one short-lived app-server of its own.**

The conversation process stays alive across its turns and is reaped when the
session ends. Continuity across a laplus restart is the Codex thread id and its
rollout under `CODEX_HOME`, not a shared process that survives conversations.

The probe never joins a conversation process. It has no thread whose lifetime it
could share, and combining it with one would make provider refresh depend on an
arbitrary open conversation.

## Consequences

- Session lifetime, epochs, settling and shutdown remain one shared mechanism
  for every driver rather than gaining a Codex-only process registry.
- Several open Codex conversations mean several app-servers. Codex currently
  starts an MCP child per thread, so this topology has a measurable process cost.
- A future shared-process design is possible, but it must replace this decision
  explicitly and provide an equivalent owner for per-conversation stop, failure
  isolation and MCP configuration.
- The provider probe always terminates and waits for its child after the four
  answers, including when notifications or alarming stderr were harmless noise.
