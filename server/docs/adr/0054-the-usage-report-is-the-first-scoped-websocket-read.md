# ADR-0054 — The usage report is the first scoped WebSocket read

The usage report reconstructs historical provider consumption and estimated
API-equivalent cost from transcripts on the server's machine. Laplus will match
T3 Code by requiring `orchestration:read` for `server.getUsageSummary`, making
this the first WebSocket RPC whose authenticated session is also checked for a
method-specific scope. Authenticating the socket alone would disclose this
financially sensitive summary to a paired client that was deliberately granted
no orchestration read access; leaving all WebSocket scope enforcement for a
future all-at-once migration would therefore weaken the feature at launch.

This introduces an authorization seam at RPC dispatch rather than a Usage-only
credential check. Later scoped RPCs may reuse that seam, but this decision does
not retroactively assign or enforce scopes for unrelated existing methods.
