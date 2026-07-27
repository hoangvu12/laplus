# ADR-0014 — The server moves into the fork, as `server/`

Date: 2026-07-28
Status: Accepted

Supersedes ADR-0012's layout — "cloned **beside** this repository" — and with it
the `../laplus/apps/web/dist` path that `build.rs` reached through. Everything
else in 0012 stands: why the UI is a fork, why upstream's `apps/server` and
`apps/desktop` are not deleted, why `@t3tools/*` is not renamed.

## Context

ADR-0012 put the UI in a fork we own and cloned it beside the server, giving a
layout that depended on two directory names being right on disk. 0012 listed the
consequence itself: _"`build.rs` reads `../laplus/apps/web/dist`, so it assumes
a layout rather than describing one."_

Two repositories also split work that is not split. Tickets 26, 31 and 33 were
each a disagreement between the client and the server, and the interesting ones
can be fixed on either side — so the change, the test that proves it, and the
ticket that describes it landed in two places or waited.

The counter-argument was merge cost, and it was overstated. Git conflicts arise
on files **modified or deleted** on both sides. 0012's expensive example was
renaming `@t3tools/*` across 1,069 files — touching what upstream owns.
_Adding_ a path upstream has never had is the cheap kind of divergence.

The legibility objection was the real one: a repository whose root is someone
else's product, with ours bolted on, does not read as this project. That is
answered separately and reversibly by `git sparse-checkout`, which hides
`apps/{server,desktop,mobile,marketing}` and `infra/` from the working tree
without touching the index — so merges behave exactly as before, and a fresh
clone still gets everything.

## Decision

**One repository. The Rust workspace becomes `server/`; the tickets become
`.scratch/` at the root.**

```
laplus/
├── apps/web        the UI the shell embeds
├── packages/       @t3tools/{contracts,client-runtime,shared}
├── server/         the Rust server, the Tauri shell, xtask, fixtures, tools
└── .scratch/       the tickets, for both halves
```

Three properties of the shape are load-bearing:

- **No upstream file is modified.** Not `pnpm-workspace.yaml` with its catalog,
  overrides and patches; not the ~40 root scripts; not `tsconfig.base.json`. The
  merge surface is unchanged, which is the whole point of the subdirectory.
- **`server/` is not under `apps/`.** Upstream's `apps/server` still exists in
  the index — we hid it, we did not delete it — so that name is taken, and a
  collision there would be a permanent fight. Top-level also keeps a Rust crate
  out of `pnpm-workspace.yaml`'s `apps/*` glob.
- **`.scratch/` is at the root, `CONTEXT.md` and `docs/adr/` are not.** Tickets
  cover both halves — 31 is a client fix, 26 was one, 24's open question is the
  web bundle. The glossary and these records are the server's, and stay with it.

History came across with `git subtree add`, not a copy, so `git log` and
`git blame` still reach the commits that explain the code.

## Consequences

- **`server/` is the Cargo workspace root.** `Cargo.toml`, `.cargo/config.toml`
  and `target/` live there, so cargo commands are run from that directory rather
  than from the repository root. `cargo xtask release` included.
- **`build.rs` describes a layout instead of assuming one.** Three `..` from
  `server/crates/laplus-shell` reaches the root, and `apps/web` is a sibling
  directory rather than a second checkout. Its failure message now says
  `pnpm --filter @t3tools/web build` rather than telling anyone to clone.
- **A protocol disagreement is one commit.** Client, server, test and ticket
  land together. That is the win, and it sharpens 0012's stated risk — _"the
  cheapest fix is now to change the client, which is how a fork stops being able
  to merge"_ — because the cheap fix is now cheaper still. The discipline that
  answers it is unchanged: fix it where it is wrong, not where it is nearest.
- **The unredacted captures moved out from under their ignore rule.**
  `.scratch/wire-capture/raw/` holds live session tokens. Its rule was in the
  server's `.gitignore`, which no longer sits above `.scratch/`, so it is now in
  the repository root's — and it matters more than it did, because unlike the
  repository it came from, **this one is public**.
- **`.scratch/` becomes public on the first push.** Every ticket and every
  triage comment. That is a change in kind from a repository with no remote at
  all, and it is the reason this move is a decision rather than a chore.
- **`docs/agents/issue-tracker.md` stops saying there is no remote.** There is;
  `gh` works. Tickets are still markdown in the tree, because a ticket and the
  commit that closes it moving together is the property that was wanted.
