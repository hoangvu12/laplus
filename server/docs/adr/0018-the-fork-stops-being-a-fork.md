# ADR-0018 — The fork stops being a fork

Date: 2026-07-28
Status: Accepted

Supersedes the merge-preservation reasoning in ADR-0012 ("deleting paths
upstream still maintains is what turns every future merge into a fight") and
narrows ADR-0014, which brought the server into a tree shaped around syncs that
are no longer taken.

## Context

ADR-0012 forked `pingdotgg/t3code` rather than copying it, and paid for that
choice in layout: four applications and an `infra/` directory nothing here
builds were kept in the tree so that a `git merge` would not conflict; the
`@t3tools/*` scope was left unrenamed for the same reason; nine of upstream's
workflows were left firing because turning them off was "a decision about this
fork". Every one of those costs bought exactly one thing — the ability to take
upstream's changes.

That ability was used once, and ADR-0012's own comment records what it cost:
22 commits, and an application that came up unable to talk to its own server
because `effect` had changed its request-id encoding underneath the contract
(ticket 33). It was never used again. At the time of this decision the
`upstream` remote had not been fetched since the fork.

The reason it stopped being worth using is not fatigue. **laplus answers a
minority of the methods `packages/contracts` declares.** Upstream shipping a
feature adds UI that calls methods this server does not implement, so a sync
widens the parity gap rather than closing it. The tickets 39–70 exist because of
the gap that is already there; importing more of it has negative value until
that gap is nearly closed.

> **Amended 2026-07-30.** This paragraph read "26 of the 71 methods" and "much
> closer to 71". Both halves were wrong by then and had been for a while, which
> is the whole argument for the rule that now holds: a parity figure lives in
> `.scratch/contract-parity/ledger.md` and nowhere else, because a count written
> into prose is a claim nothing re-checks. The decision below does not depend on
> the number — it depends on the direction, and the direction has not changed.

## Decision

**Stop merging from upstream, and take the layout that follows from it.**

The repository is re-founded from its working tree, with no history before the
first commit and no `upstream` remote:

- `apps/{server,desktop,mobile,marketing}`, `infra/`, and the 118 MB of vendored
  reference checkouts under `.repos/` are gone rather than sparse-checked-out.
- Nine of upstream's eleven workflows are gone. Two remain and both are ours,
  which closes ticket 70 by construction.
- Root configuration — `package.json`, `pnpm-workspace.yaml`, `vite.config.ts` —
  is written for this project rather than inherited and trimmed. The catalog
  keeps only what the four surviving packages resolve, with every version
  carried across unchanged.
- **`packages/ssh` and `packages/tailscale` are dropped.** Nothing in `apps/web`
  or the three contract packages imports them; they served the relay.

Two things are deliberately _not_ done:

- **`@t3tools/*` is still not renamed.** ADR-0012 declined it to protect merges;
  that reason is gone and the conclusion survives on its own. It appears in over
  a thousand files, and the rename is churn against every import in the tree for
  a nicer name. It stays available as a mechanical change whenever it is worth a
  day.
- **Upstream's server is kept, as `reference/t3code-server/`.** Not built, not
  linted, not imported — see `reference/README.md`. laplus is built to be
  feature-compatible with it, so it is the specification the remaining parity
  tickets are argued against, and PARITY-LEDGER's section 7 is that directory
  read directly. Losing `git show HEAD:apps/server/src/ws.ts` was the one real
  cost of re-founding the repository, and this is what it is traded for.

## Consequences

- **No upstream fixes, and no route back to them.** ADR-0012 accepted this cost
  in the form "paid with `git merge` rather than by never moving". It is now
  paid by hand or not at all. Anything wanted from upstream is read and ported,
  with `reference/` as the model for how that reads.
- **The UI is unambiguously ours.** Ticket 32 made it editable; this makes it
  unshared. The "build `upstream/main` without our commits and confirm it still
  connects" check that ADR-0012 proposed keeping is no longer possible, and user
  story 57 — the unmodified upstream UI against this server — is retired rather
  than deliberately checked.
- **History before 2026-07-28 is not in this repository.** `git blame` on
  `apps/web` answers with the founding commit. The predecessor repository holds
  it, and is the place to look when a line of UI needs its reasoning recovered.
- **Tickets written before this ADR describe a tree that no longer exists.**
  Several — 32 above all — reason from sparse-checkout, the `upstream` remote,
  and the object store. They are correct as history and wrong as instructions.
  `server/CLAUDE.md` has been corrected; the tickets have not, and are not going
  to be.
- **Ticket 70 is closed by construction**, and ticket 24's bundle question is
  now unobstructed: trimming `apps/web` conflicts with nobody.
