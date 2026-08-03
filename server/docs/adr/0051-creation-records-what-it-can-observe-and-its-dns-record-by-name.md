# ADR-0051 — Creation records what it can observe, and its DNS record by name alone

Date: 2026-08-03
Status: Accepted

Creating a stable tunnel is three mutations at two different places — allocate
the tunnel, route a DNS name to it, write laplus's own configuration — and any
of them can be the last. So each is journaled before it happens, and each is
skipped when the thing it would produce is already there. What "already there"
means is different for all three, and that difference is why a resume reconciles
rather than replays the log. The allocation is observable, because `cloudflared
tunnel create --credentials-file` writes a `<UUID>.json` into laplus's private
directory and that file names the tunnel Cloudflare made. The configuration is
observable, because it is the connector's own settings file and the manager read
it back at boot. The DNS route is not observable at all from this machine, so for
that one step the journal — and afterwards the endpoint row — is the observation.
Reading a log entry where nothing else can be read is not the hopeful in-memory
list ADR-0050 rejected; it is the only durable evidence that step leaves.

There is no `Credential` step in a creation's journal, because no command
performs one separately: `tunnel create --credentials-file` allocates the tunnel
and writes the credential that runs it in a single call. A fourth entry would put
a boundary in the log that a creation can never be interrupted at, and the whole
value of the log is that every entry names a place this can actually stop.

**Creation rolls nothing back.** A refused route leaves a real tunnel at
Cloudflare and says so, naming the work completed and the work outstanding; it
does not attempt a `tunnel delete`, because a rollback that can itself fail
leaves a worse-described state than the one it was tidying, and because removing
a Cloudflare resource is ticket 07's separately confirmed operation rather than
an implicit consequence of a failure. Repeating the command is the recovery, and
repeating it after everything is recorded is a read.

**The DNS record is recorded by name, and the name is all there is.**
`cloudflared tunnel route dns` prints `Added CNAME <hostname> which will route to
this tunnel` and returns no zone id and no record id; the CLI has no structured
output for the verb and no symmetric `route dns delete`. The zone and account
identifiers do exist inside the account certificate, and ADR-0045 forbids laplus
to read its contents — it uses the file in place and never opens it. So the only
thing creation can truthfully write down about the record it made is the name it
asked Cloudflare to create, which is also exactly what a destructive
confirmation has to show a human. `DnsRecord` therefore requires a name and
makes the two identifiers optional, and `addressable()` is how a caller asks
whether the record can be reached through Cloudflare's DNS API as it stands.
A row with a name and neither id is a record laplus made and has not yet
addressed, rather than a half-written one.

This costs ticket 07 a lookup: deleting the record means acquiring DNS authority
of its own and resolving the name to a zone and a record before removing it, and
it must write the identifiers back onto the row rather than assume creation
supplied them. That was already true of the authority — the CLI cannot delete a
DNS record at all — so the cost is a resolution step rather than a new
capability. It buys a creation that never invents an identifier it did not
receive, an endpoint row whose absent fields mean something specific, and a
partial creation that can say which of three resources exist without claiming a
rollback it did not perform.
