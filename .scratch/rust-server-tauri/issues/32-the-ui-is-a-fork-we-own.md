# 32 — The UI stops being someone else's and becomes ours

**What to build:** the shell embedding `laplus`, a fork of t3code that this
project owns, in place of the read-only vendored checkout.

**Status:** done

**Found by:** ticket 26. Not a defect — a decision that ticket forced into the
open and could not itself resolve.

## Why

Three problems in this tracker are all the same problem: the client does
something lightcode cannot change.

- **26** — the version-skew banner. Silenced by making the server report the
  bundle's version, which works but leaves `serverVersion` naming the UI rather
  than the server. With an editable UI the check is three lines to delete.
- **31** — every snapshot is asked for over HTTP first, so every one 404s
  before the client falls back to the socket. Fixable at the client instead of
  by implementing two routes this server does not want.
- **24's open question** — trimming the bundle. Parked there as "a change to
  vendored code that needs its own decision". This is that decision, and it is
  the one with the artifact size behind it.

Each on its own is a poor reason to fork 184,082 lines of TypeScript. Three,
plus a rename the project wanted anyway, is a different arithmetic.

## What was decided

`hoangvu12/laplus`, a public fork of `pingdotgg/t3code`, cloned as a **sibling
of this repository** rather than inside it. The frontend packages come across
whole — `apps/web`, `packages/{contracts,client-runtime,shared}` — and
`apps/server` (171k lines of Effect TypeScript) and `apps/desktop` (Electron)
are the parts lightcode already replaced with 51k lines of Rust and one window.

Not deleted, though: they sit there untouched, because deleting paths upstream
still maintains is what turns every future merge into a fight.

The app is renamed to **laplus** in one constant — `APP_BASE_NAME` — because
every other name in the running UI derives from it. The `@t3tools/*` package
scope is **not** renamed: it appears in 1,069 files, and renaming it would
conflict with upstream on nearly every merge, which is precisely what forking
rather than copying was supposed to buy.

## What this costs, said plainly

- **The spec's non-goals move.** "The UI is reused unmodified and is upstream's"
  stops being true, and user story 57 — the _unmodified_ upstream UI connecting
  to this server — becomes a thing to check deliberately rather than a property
  held by construction. Worth keeping as a check: build `upstream/main` without
  our commits and confirm it still connects, and any divergence in the protocol
  shows up there rather than in a user's window.
- **`pnpm` is now load-bearing.** It always was, informally — `dist/` was built
  by hand — but it was building _someone else's_ code. A stale `dist/` is now a
  lightcode bug rather than a vendoring detail.
- **No free upstream fixes, by merge instead of by pinning.** The spec already
  accepted this cost. What changes is that it is now paid with `git merge`
  rather than by never moving.

## Acceptance

- `cargo build -p lightcode-shell` embeds `laplus/apps/web/dist`, and says what
  to clone and where if it is not there.
- The window's title and sidebar say **laplus**.
- The server suite is unchanged and green: none of this is server behaviour.
- The UI is driven headless and boots against the new bundle, since 22 upstream
  commits — four of them touching `packages/contracts` — came with the move.

## Comments

### 2026-07-28 — agent. Done, and it cost one ticket on the way

The fork is `hoangvu12/laplus`, cloned beside this repository with `upstream`
pointing at `pingdotgg/t3code` and its push URL disabled. The rename came to
three lines — `APP_BASE_NAME` and the two tests that pinned the old default —
because every displayed name derives from that constant and `index.html` has no
title of its own. The window now says **laplus (Alpha)**; the suffix is
upstream's release-channel notion (`APP_STAGE_LABEL`, "Alpha" in a production
build) and is left alone rather than quietly dropped.

**What taking HEAD cost.** The fork's `main` was 22 commits past the pinned
checkout, and the app came up unable to talk to its own server: `effect` had
moved `4.0.0-beta.78 → 4.0.0-beta.102` and its RPC client now sends numeric
request ids, which this server dropped as malformed. Ticket 33 has the whole of
it. Worth recording here because _nothing in `packages/contracts` showed it_ —
the change was in a library underneath the contract, where the conformance suite
cannot see it, and the only instrument that caught it was opening the window.

That is the shape of the risk this ticket accepted, demonstrated on day one: a
sync is not a fact that arrives in the background, it is a thing to do and then
verify against the real client.

**Verified** with `tools/ui-driver/probe-boot.mjs` against a release build on
port 4774 with its own profile: configuration arrives, the sidebar renders the
project and its threads, the composer is there, no version-skew banner, and
`document.title` is `laplus (Alpha)`. The 404s in the console are ticket 31,
which this ticket now makes fixable and does not fix.
