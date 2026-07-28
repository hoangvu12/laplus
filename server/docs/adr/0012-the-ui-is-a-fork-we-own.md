# ADR-0012 — The UI is a fork we own, cloned beside this repository

Date: 2026-07-28
Status: Accepted

Supersedes the vendoring position in ADR-0010's consequences ("upstream's bundle
ships exactly as upstream built it") and the spec's "reference material only".

## Context

lightcode reuses t3code's `apps/web` rather than writing a UI. Until ticket 32
that reuse was **read-only**: a depth-1 checkout at `t3code/`, gitignored, built
by hand, never modified. The spec said so twice — "reference material only, never
a build dependency", and a non-goal of "the UI is reused unmodified".

Three tickets then turned out to be the same ticket, and none of them was a
server problem:

- **26** — the client compares its `APP_VERSION` against `serverVersion` by
  string equality and warns on every launch. Silenced by reporting the bundle's
  version, which works, but leaves the field naming the UI rather than the
  server. With an editable client the check is three lines to delete.
- **31** — the client asks for every snapshot over HTTP before falling back to
  the socket, so every one 404s first. It is fixable in one client function, or
  by implementing two routes this server does not want to own.
- **24's open question** — trimming the bundle. Parked as "a change to vendored
  code that needs its own decision", on a project whose reason to exist is the
  size of the artifact.

The costed alternatives were: leave it (three known problems stay unfixable and
the fourth is already queued); a maintained patch series applied at build time
(cheap, but drifts, and still cannot rebrand); or a fork.

## Decision

**`hoangvu12/laplus`, a public fork of `pingdotgg/t3code`, cloned as a sibling
of this repository. The frontend comes across whole; the server and the Electron
shell are the parts lightcode already replaced.**

```
nguyenvu/
├── lightcode/   the Rust server (50,992 lines), the Tauri shell, the tickets
└── laplus/      apps/web + packages/{contracts,client-runtime,shared}
                 (184,082 lines), and upstream as a remote
```

Four things about the shape are load-bearing:

- **`apps/server` (171k lines) and `apps/desktop` (32k) are not deleted.** They
  are what lightcode replaced, and deleting paths upstream still maintains is
  what turns a merge into a fight. They cost nothing: nothing builds them.
- **The `@t3tools/*` package scope is not renamed.** It appears in 1,069 files.
  Renaming it would conflict with upstream across most of the tree on every
  merge — throwing away the one thing a fork has over a copy.
- **The app is renamed in one constant.** `APP_BASE_NAME` in `branding.ts`, from
  which the window title and every displayed name derive at runtime. Three lines
  including the two tests that pinned the old default. The other 58 "T3 Code"
  strings live in features this fork does not ship.
- **`upstream`'s push URL is disabled** in that clone, so a `git push` cannot
  reach `pingdotgg/t3code` by accident.

## Consequences

- **"The client does that" stops being an answer.** Tickets 26, 31 and 24's
  bundle question all become ordinary work. That is the whole point, and it is
  also the risk: the cheapest fix for a protocol disagreement is now to change
  the client, which is how a fork stops being able to merge.
- **The spec's non-goals move, and user story 57 changes character.** "The
  unmodified upstream UI connects to this server" was true by construction and
  is now a thing to _check_ — build `upstream/main` without our commits and
  confirm it still boots. Worth keeping precisely because it is the tripwire for
  the previous bullet.
- **`pnpm` is load-bearing.** It always was — `dist/` was built by hand — but it
  was building someone else's code. A stale `dist/` is now a lightcode bug.
- **The bundle path leaves the repository.** `build.rs` reads
  `../laplus/apps/web/dist`, so it assumes a layout rather than describing one.
  The failure prints the two commands that produce it, which is the mitigation.
- **The artifact is unaffected.** Same bundle, same embedding, same measurement
  in `docs/artifact-size.md`. What changed is who may edit the input.
- **Attribution does not change.** The UI is still derived from t3code and still
  MIT; `THIRD_PARTY_NOTICES.md` and the `bundle.copyright` string still name
  T3 Tools, Inc., and `xtask::notice` still enforces both.
- **Upstream arrives by merge now.** The move took 22 commits with it — four
  touching `packages/contracts` — which is the first evidence that syncing is
  something to do deliberately and verify, not a background fact.
