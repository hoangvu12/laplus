# ADR-0017 — A refusal borrows a tag it does not mean, because the contract offers no other

Date: 2026-07-28
Status: Accepted

## Context

Thirty-nine of the seventy methods `packages/contracts/src/rpc.ts` declares are
not implemented here, and each one has to be _declined_ rather than answered.
Ticket 03 settled the envelope: an `Exit`/`Failure` carrying a typed error, so a
refusal costs one request and does not drop the connection.

Ticket 39 found that it cost more than that. An error on this wire is
`_tag`-discriminated, and the client decodes each one against the union **that
method** declares. Every refusal carried `ServerMethodNotImplementedError`, a tag
that is not in the contract at all — so no union contains it, `decodeExit` fails,
and `/settings/diagnostics` and `/settings/source-control` showed the schema
decoder's complaint about the shape of the refusal instead of anything about the
feature. The developer read five lines of `Expected { readonly "_tag": … }` where
the news was "this server has not built that yet".

This is ADR-0009's rule one layer out. There the decision was that a value the
client cannot decode is refused before it is stored; here the thing the client
cannot decode _is the refusal_.

The contract then removed the choice. Every one of the seventy methods has
`EnvironmentAuthorizationError` in its error union — the union of one for fifteen
of them, including all four the surface walk found, and the last member for the
rest. There is no tag available to every method that means "unimplemented", and
none that means nothing at all.

## Decision

**A refusal carries a tag the method it names declares, even when that tag is not
what happened.** Concretely, `crate::refusals` pairs each of the seventy contract
methods with `EnvironmentAuthorizationError` and `requiredScope:
"orchestration:read"`, and the sentence — `Method not implemented by this server:
{method}` — carries the truth. The message is the only part the client renders:
`useEnvironmentQuery` squashes the cause and shows it.

`ServerMethodNotImplementedError` survives for a tag the contract does not name
at all. That has no declared union, so there is nothing for it to fail to decode
against; it is what `no.such.method` gets, and it keeps naming the method,
because that is the only thing that says which of the seventy a developer
mistyped.

`orchestration:read` rather than something narrower because it is the scope every
connected client already holds, so the refusal does not read as a permission
somebody could go and grant.

## Considered options

- **Keep an invented tag.** This is the bug. A tag outside the union is not a
  refusal the client can read, whatever it says.
- **Invent a tag per method that says "unimplemented".** Same failure — the
  problem is not the word, it is membership of a closed union that upstream owns.
  `packages/contracts` is upstream's and stays schema-only.
- **Answer successfully with an empty payload.** Ruled out for two reasons: most
  of these methods have success shapes with required fields no empty value
  satisfies, and a server that reports success and does nothing is the failure
  ADR-0009 calls "the worse one to debug".
- **Send a bare `Defect`.** What the reference server does, and what ticket 03
  deliberately diverged from: laplus refuses most of the vocabulary while it is
  being built, so `Defect` would be the normal answer to the UI's own boot
  sequence.

## Consequences

- **The tag is not inert, and on two methods the client acts on it.**
  `packages/client-runtime/src/rpc/session.ts` maps
  `EnvironmentAuthorizationError` from `server.getConfig` or `server.probe` to
  `ConnectionBlockedError({reason: "permission"})` — a connection refused, not
  retried, rather than an empty state on a page. `server.probe` is in the refused
  set, so this is live code one step away: it is dormant only because this server
  does not advertise `capabilities.connectionProbe`, which makes the client probe
  with `server.getConfig` instead. **Advertising that capability before
  implementing `server.probe` turns every probe into a blocked connection.** Both
  rows in the table say so; this is the sharpest cost of the decision and the
  reason it is written down rather than left in a module doc.
- **A refusal says "authorization" where the truth is "unimplemented".** Anyone
  reading a socket capture, or a log, will see a permission error that is not
  one. The sentence beside it is the correction, and it is the part on screen.
- **The table is all seventy methods, and the test asserts set equality with
  `rpc.ts`.** So a purely additive `upstream` merge — a routine event here — goes
  red until somebody adds a row. That is deliberate. An unlisted contract method
  is still refused, and refused with `ServerMethodNotImplementedError`, which
  puts the original bug back on the screen; the merge that adds a method is
  exactly when its union should be read. The alternative, a subset assertion,
  makes the row forgettable and defers the failure to a test that says less about
  why.
- **`requiredScope` has to stay a real scope.** It is required by the class and
  is one of eight literals in `packages/contracts/src/auth.ts`. A ninth would
  fail the decode as surely as a wrong `_tag`. Nothing reads its value; that is
  not a licence to invent one.
- **Reversing this is cheap in code and expensive in evidence.** The tag lives in
  one table behind one function, so changing it is a small edit — but the reason
  it is this tag is a reading of all seventy unions in another language, which is
  why `refusals::contract` parses them back out rather than trusting the table.
