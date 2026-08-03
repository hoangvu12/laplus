# ADR-0050 — Adoption rechecks activity at the last moment and resumes from observed state

Date: 2026-08-03
Status: Accepted

Dedicating an inactive existing tunnel to a laplus environment is confirmed
against a listing, and a listing is evidence about the past. A connector can
start between the screen that says "no connector is serving it" and the button
that says yes, and ADR-0045 makes an active tunnel externally managed — so
laplus re-reads the tunnel's activity immediately before its first mutation
rather than trusting the answer the confirmation was drawn from. `tunnel list`
mutates nothing at Cloudflare, so the recheck costs a read and buys the race.
A tunnel that has become active is not a failed adoption: the hostname is
registered as an external tunnel endpoint, verified and advertised, and laplus
operates nothing. The refusal says which mutations remain rather than implying
a rollback of work that never started.

The same rule read the other way is why a repeated confirmation answers with
what it already recorded instead of starting again. After a successful
adoption the account's listing shows connections — laplus's own — so a second
confirmation that re-ran the recheck would disown a tunnel this environment is
correctly running. An adoption already recorded on the endpoint row for the same
tunnel is therefore a read.

Between those two, adoption is two mutations, and both are journaled before they
happen and settled after. A resume reconciles against what is observably there
rather than replaying the log: a run credential already in laplus's private
directory is the retrieval having already happened, and fetching it again would
spend the account certificate at Cloudflare for a file that is already on disk.
The journal is what lets a refusal name both halves — the work completed and the
work outstanding — which is the first thing `public_exposure_journal` and
`Refusal::after` have been used for since ADR-0049 created them.

An adopted tunnel is `adopted` on the endpoint row and nothing else may say
otherwise. Registering a hostname as externally operated, selecting a different
tunnel, and configuring a connector-token connector are all refused while an
adopted or laplus-created tunnel is recorded, because each of them writes that
row and each would launder ownership into a word that authorizes a deletion.
The deletion verdict itself crosses the wire beside the ownership rather than
being derived by the client, so "Delete everywhere is never offered for an
adopted tunnel" is one answer the offer and the refusal share instead of two
that can come apart.

This costs a Cloudflare read on every confirmation and a `cloudflared tunnel
token` that only works for tunnels created since 2022.3.0 — older ones cannot
supply a run credential and are refused with that reason. It buys an adoption
that cannot half-happen invisibly, cannot claim a rollback it did not perform,
and cannot be turned into deletion authority by any repeated, stale or forged
request.
