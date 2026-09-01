# ADR-0058 — OpenCode stop is verified and owned runaways are reaped

Date: 2026-08-22
Status: Accepted; a last rung added on 2026-09-01

> **Amended 2026-09-01.** "A failed sample leaves the conversation loop alive
> and visibly reports that verification is continuing" describes a ladder with
> no bottom. A history that answers nothing but errors can never close a quiet
> interval and can never show the changed output the escalation below rests on,
> so verification returns _pending_ forever and the stopped turn is immortal:
> the conversation stays alive and useless, which is worse than either outcome
> the paragraphs below choose between. The ladder now ends. An unbroken run of
> unreadable history snapshots lasting the same bounded window that escalates a
> proven runaway abandons verification: the turn settles as interrupted, the
> failure already reported once stands as the only report, and the conversation
> stays open for the next prompt. A readable snapshot restarts that window, so
> one transient error only delays the proof rather than ending a turn. Nothing
> is killed on this rung in either ownership mode — a history that cannot be
> read is not the proof of a runaway the reap below is built on, and an
> external server is never ours to kill regardless. Ending the whole
> conversation remains the exclusive job of explicit stop-session.

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
