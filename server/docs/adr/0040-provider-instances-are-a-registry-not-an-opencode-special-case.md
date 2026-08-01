# ADR-0040 — Provider instances are a registry, not an OpenCode special case

Date: 2026-08-01
Status: Accepted

Laplus will implement the contract's generic provider-instance registry before
adding OpenCode, migrating the built-in Claude and Codex settings into
compatible default instances. OpenCode may then have several independently
configured instances, each with its own server connection, credentials,
environment, catalogue and continuation identity, matching T3 Code. Adding one
hard-coded OpenCode slot to the existing closed Rust registry would ship the
immediate provider but deepen the routing and settings divergence every future
driver would have to undo.
