# ADR-0052 — A destructive deletion is authorized by a fresh confirmation of the exact recorded resources

Date: 2026-08-03
Status: Accepted

Removing a Cloudflare tunnel and its DNS record is the one laplus operation that
destroys something outside this machine and cannot be undone. ADR-0049 already
decides _who_ may ask for it — only a `laplus-created` tunnel, read from the
persisted endpoint row and never from the request — and that answer is necessary
but not sufficient. A scope says which sessions may issue a command; it cannot
say what the person holding that session was shown, and the specification
requires the destructive path to be a separate confirmation naming the exact
tunnel and DNS resources laplus recorded.

So laplus mints the confirmation itself. `POST
/api/access/cloudflare/account/deletion` reads the endpoint row, refuses for any
ownership but `laplus-created`, and answers with the tunnel id, the DNS record
name, the endpoint, the steps it will take, and a one-time value. The deletion
spends that value: it is removed from memory when read, refused once it is older
than five minutes, and refused when the tunnel, hostname or record it names is
not what the row records at the moment the command runs. That is the whole of
"including through repeated, stale, or forged client requests" — a repeat finds
it spent, a stale one finds it expired or naming something else, a forged one was
never minted, and a restart invalidates every outstanding offer because the
offers are held in memory rather than persisted. An offer left on a screen
yesterday is not authority today.

**Deleting the DNS record needs authority laplus does not hold and will not
take.** `cloudflared` has no `route dns delete`; removing a record is a
Cloudflare DNS API call. The account certificate does contain a token that could
make it, and ADR-0045 forbids reading the certificate's contents — it is used in
place, by pointing `cloudflared` at it, and opening it to extract a token is
exactly the copying that ADR rules out. The developer therefore supplies a
Cloudflare API token with DNS edit permission for the one request that needs it.
It is never persisted, never logged, never put in a snapshot and never passed as
a process argument, and it is redacted out of every refusal the route can answer
with. Missing or insufficient DNS authority is refused _before_ the first step is
journaled: the tempting alternative is to delete the tunnel anyway and report
success, which leaves a hostname answering with a Cloudflare error page and is a
weaker operation rather than a recoverable state.

**Forget removes laplus's own setup and no executable, including the one laplus
installed.** Ticket 02 says laplus never removes a system or user-selected
executable, and an app-managed `cloudflared` (ADR-0046) is neither — it lives in
the same private directory as the configuration and credential this removes, so
the question had to be decided rather than left to which files happened to be
named. It stays. Forget is about _this exposure's_ setup; the executable is a
tool any future setup on this machine would use, removing it costs a network
download and a second explicit approval to undo, and nothing about a forgotten
tunnel makes a verified copy of `cloudflared` unwanted. Forget also stops the
connector before it removes anything, because removing a running connector's
configuration leaves a `cloudflared` serving a public hostname that nothing
records — which is the state the previous row-only Forget actually produced.

This costs a Cloudflare API token the developer has to create, a round trip to
resolve a record laplus could only name (ADR-0051), and a confirmation that
expires while a developer is reading it. It buys a deletion that cannot be
reached by an adopted or external tunnel through any request, cannot be replayed,
cannot silently target a resource other than the one confirmed, and cannot half
succeed without saying exactly what remains.
