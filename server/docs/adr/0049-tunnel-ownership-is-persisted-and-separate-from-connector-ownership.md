# ADR-0049 — Tunnel ownership is persisted, and separate from connector ownership

Date: 2026-08-03
Status: Accepted

Who runs a `cloudflared` connector and who owns the Cloudflare tunnel behind it
are two questions with different answers, and laplus records them separately.
Connector ownership is laplus or an external supervisor. Tunnel ownership is
`external`, `adopted`, or `laplus-created`, and it is the only thing that
authorizes deleting a Cloudflare resource: laplus may delete a tunnel and DNS
record it created, and may never delete one it merely adopted or merely
verifies. A connector-token tunnel is therefore `external` and laplus-run,
because Cloudflare keeps its configuration and allocation.

Tunnel ownership is a persisted column on the one public-exposure endpoint row,
together with the exact resources laplus made — tunnel id, DNS zone, record id
and record name, credential and configuration paths — and a journal of the
mutation steps a multi-step create, adopt or cleanup started and settled. It was
previously a pair of string literals in a snapshot and one write nothing read
back, so every laplus-managed connector looked alike and the row that described
somebody else's hostname was also the row a supervised connector was restored
from. Ownership lives in exactly one place: the connector's own settings file
carries what it needs to run and says nothing about who owns its tunnel, so a
restart cannot have to choose between two records of one fact.

Ownership is not a value a client may supply. Registering a hostname as
externally operated is refused while an adopted or laplus-created tunnel is
recorded, because laundering ownership is how a repeated, stale or forged
request would earn a deletion it is not entitled to. A word outside the
vocabulary is a refused read rather than a default, since every default
available would be a guess about deletion authority.

This costs a schema migration on installs that already hold an endpoint, which
migrate to `external` — true for everything the previous schema could record,
and the ownership that authorizes nothing. It buys stop, forget and delete being
distinguishable at the server rather than by which button a client chose to
draw, and a partial mutation that can say what happened and what remains instead
of claiming a rollback it did not perform.
