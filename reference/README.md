# reference/

Read-only. Nothing here is built, linted, typechecked, tested, or shipped.

## `t3code-server/`

Upstream t3code's TypeScript WebSocket server — the one laplus replaced with
`server/crates/laplus-server`. It is here as a **specification**, not as code.

laplus is built to be feature-compatible with it: the contract in
`packages/contracts` declares 71 socket methods, laplus answers a subset, and
the question every parity ticket eventually asks is "what does the real server
do here?" This is the only thing that answers it. `PARITY-LEDGER.md` was derived
by reading it, and section 7 of that ledger — the part that found gaps the
tickets did not know about — is this directory read directly.

It was lifted out of `pingdotgg/t3code` at the commit this project forked from,
and it does not track upstream. It will drift, and that is fine: it is evidence
of an implementation, not a dependency.

### Rules

- **Do not edit it.** A change here is a change to the evidence.
- **Do not import from it.** It is excluded from the pnpm workspace and from
  the lint, format and test configuration, and its `@t3tools/*` imports will not
  resolve. That is deliberate.
- Read it when a ticket needs a behaviour pinned down. `src/ws.ts` is the method
  table; `src/` is the rest.

MIT, Copyright (c) T3 Tools, Inc. See `LICENSE` at the repository root.
