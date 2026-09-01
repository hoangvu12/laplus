# ADR-0058 — OpenCode stop is verified and owned runaways are reaped

Date: 2026-08-22
Status: Accepted; the ladder's missing bottom superseded by [ADR-0059](0059-a-stop-that-cannot-be-proved-still-ends-the-turn.md)

> **Superseded in part on 2026-09-01.** "A failed sample leaves the conversation
> loop alive and visibly reports that verification is continuing" describes a
> ladder with no bottom: a history that answers nothing but errors can never
> close the quiet interval this ADR proves quiescence with, and can never show
> the changed output its escalation rests on, so verification returns _pending_
> for ever and the stopped turn is immortal. ADR-0059 ends the ladder, and says
> what ending it costs — a queued prompt released into a conversation whose
> provider may still be producing, and the rule about retired provider messages
> that makes that release safe. Everything else below stands, including the
> whole of the proof and the owned reap.

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
