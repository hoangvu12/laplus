# ADR-0035 — OpenCode HTTP is a narrow owned protocol

Date: 2026-08-01
Status: Accepted

T3 Code drives OpenCode through its official generated TypeScript SDK, but
OpenCode publishes no corresponding official Rust SDK. Laplus will instead own
a narrow `reqwest`/serde client for the HTTP routes and SSE events its OpenCode
driver uses, checked against a pinned OpenAPI document and captured wire
fixtures. It will not generate a full client during the build or depend on an
unofficial Rust SDK: generation brings a large unrelated surface and still
requires upstream-specific corrections, while community clients add another
compatibility and Windows-support boundary. Unknown event kinds remain
observable but non-fatal so a compatible newer server can extend its stream
without ending a conversation.
