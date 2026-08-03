# ADR-0053 — Connector log redaction is a server boundary, applied twice

Date: 2026-08-03
Status: Accepted

A laplus-managed connector's own output is the only actionable thing a developer
has when `cloudflared` will not start, and it is also the one place a run
credential can be quoted back verbatim. Ticket 02 requires both: logs that are
redacted, and logs that are worth reading. Dropping a line that mentions a secret
satisfies the first rule by abolishing the second, so redaction has to be a
substitution rather than a filter — the sentence survives with `[REDACTED]` where
the secret was.

**The redaction happens on the server, and the client renders what it is given.**
`packages/contracts/src/remoteAccess.ts` types `logs` as an array of strings, and
that is deliberate rather than an omission. The alternative considered was to mask
the pane in the client the way `RedactedSensitiveText` masks a single value behind
a click-to-reveal blur. That component is right for one field whose whole content
is a secret — an API token echoed back beside the instance that holds it — and
wrong here for two reasons. Blurring a multi-line log blurs the diagnosis as well
as the secret, which is the failure mode this feature exists to avoid; and a
client-side mask is not a security boundary at all, because the value it hides has
already crossed the wire to a browser that can be read with devtools. A secret
that reached the client is a secret that leaked. So the client gets no affordance
to reveal what the server would not send, and the server sends nothing to reveal.

**It is applied at two points, and the second is not ceremony.** The first is at
capture, as the child's output is read. The second is as the snapshot is built,
over `logs` and `failureMessage` together. They share one function and differ in
timing: the first keeps a secret out of laplus's own memory, and the second
answers for anything that reached that memory by a route the first does not
cover. Checkbox 2 is a claim about everything crossing this wire, not a claim
about one function, and a rule enforced only where today's single caller happens
to sit is a rule the next caller will not know about.

**What made this necessary was a real gap rather than a theory.** Redaction used
to open the run-credential file at the moment a log line arrived, and match
against what it found. A file it could not read yielded no secrets and therefore
redacted nothing — and the arrival of a log line is not an arbitrary moment. A
connector's stderr is drained when its child exits, and Forget stops the connector
and _then_ removes its credential (ADR-0052). The one moment `cloudflared` is most
likely to be complaining about its token is the one moment laplus could no longer
recognise it. Secrets are therefore learned while the file is certainly readable —
at boot, when a configuration writes one, and before each launch — remembered for
the life of the process, and merged rather than replaced, so that a later read
finding nothing takes nothing away.

The secret set is still built from the credential file's _shape_ rather than from
a remembered literal: a connector-token file is the secret, and a tunnel
credential is a JSON document whose `TunnelSecret` is, so redacting either whole
document would leave the inner value readable in any sentence that quoted only it.
Secret-shaped fields only — an account tag and a tunnel id are in the snapshot
already, and blanking them turns "the connector for tunnel 2222 failed" into a
sentence nobody can act on.

This costs a secret held in process memory for as long as the manager lives,
which is the same lifetime as the credential path it would otherwise re-read, and
one substitution pass per snapshot over at most fifty short lines. It buys a
redaction that does not depend on the filesystem agreeing to cooperate at the
least convenient moment, and a boundary that a future field on this snapshot is
answered by without its author having to know the rule exists.
