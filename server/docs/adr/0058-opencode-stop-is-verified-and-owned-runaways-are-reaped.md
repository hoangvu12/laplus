# ADR-0058 — OpenCode stop is verified and owned runaways are reaped

Date: 2026-08-22
Status: Accepted

An accepted OpenCode abort request begins a stopping phase; it does not prove
the turn stopped. Laplus samples the session's assistant messages and treats
one unbroken quiet interval as quiescence; equal point samples alone prove
nothing, and any changed snapshot restarts the interval. Provider status is
only a hint and cannot settle an interrupted turn. A failed sample leaves the
conversation loop alive and visibly reports that verification is continuing.

If output keeps changing through the bounded verification window, Laplus
settles the interrupted turn and reaps a server process it launched, allowing
the next prompt to resume by durable session id under ADR-0041. An external
server remains operator-owned under ADR-0036: Laplus reports that it ignored
the stop and continues supervision, but never kills it.
