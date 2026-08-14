# ADR-0045 — OpenCode queues messages during active work

Date: 2026-08-14
Status: Accepted

When a developer sends one or more messages during an active OpenCode turn,
Laplus stores them as a queued turn. It starts that turn after the active turn
settles. An interrupt stops only the active turn and does not discard the queued
messages. This matches Claude and Codex, preserves each user message across
navigation, and prevents an interrupt race from steering new text into a turn
that is stopping. It supersedes ADR-0038.
