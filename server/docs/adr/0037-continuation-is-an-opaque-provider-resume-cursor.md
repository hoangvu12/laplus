# ADR-0037 — Continuation is an opaque provider resume cursor

Date: 2026-08-01
Status: Accepted

Laplus persists continuation as opaque, versioned JSON interpreted only by the
driver that produced it, replacing the Claude/Codex-specific
`agent_session_id` string concept. Existing strings remain readable as each
current driver's v0 cursor, so the migration does not strand conversations.
Unlike T3 Code, an established cursor with a malformed or unsupported shape is
reported as incompatible instead of being treated as absent: silently starting
fresh would make durable context loss look like a successful resume. This wider
storage boundary costs a migration across the existing drivers now, but avoids
redesigning persistence whenever a provider's continuation needs more than one
identifier.
