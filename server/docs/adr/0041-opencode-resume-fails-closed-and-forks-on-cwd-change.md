# ADR-0041 — OpenCode resume fails closed and forks on CWD change

Date: 2026-08-01
Status: Accepted

Laplus re-adopts an OpenCode session named by its provider resume cursor and
starts fresh only when OpenCode returns a structured missing-session result.
Transport, authentication and other server errors leave the cursor intact and
fail visibly rather than turning unavailable context into an empty successful
conversation. A recovered session whose canonical working directory differs is
forked, preserving history across worktree or path changes, and the result's
canonical directory is verified before it is adopted. OpenCode 1.18.10 leaves
that fork in the source directory despite sending the requested directory on
`session.fork`; for servers with that behavior Laplus follows the fork with the
server's move-session operation and verifies again. It never treats a
successful fork response alone as proof that the CWD changed. An in-place
recovery has the current permissions re-applied. This preserves T3 Code's
intended recovery boundary on both its pinned protocol and current OpenCode,
while retaining ADR-0037's stricter handling of malformed cursor data.
