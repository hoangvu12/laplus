# ADR-0038 — OpenCode can steer a running turn

Date: 2026-08-01
Status: Superseded by ADR-0045

When another prompt arrives during an active OpenCode turn, Laplus sends it
immediately into the busy OpenCode session and retains the active turn id,
matching T3 Code and OpenCode's native steering behavior. Claude and Codex keep
Laplus's existing queued-follow-up semantics. This makes prompt timing
provider-dependent, but preserves the capability and event meaning of each
protocol; representing an OpenCode steer as a later independent turn would
misstate both when the agent received it and which exchange produced the work.
