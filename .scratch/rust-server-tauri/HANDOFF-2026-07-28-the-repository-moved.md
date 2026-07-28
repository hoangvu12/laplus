# Handoff — this repository is new, and three things about it are not obvious

**Date:** 2026-07-28
**Commits:** `2c9487a` (founding), `15060ce` (deleting what nothing imports)
**Decision:** `server/docs/adr/0018-the-fork-stops-being-a-fork.md`

laplus stopped being a fork of `pingdotgg/t3code`. ADR-0018 is the reasoning and
the costs; this file is the operational residue — what someone picking the work
up needs that is not already a comment beside the code it describes.

---

## 1. There is a second copy of this project on disk, and it can still push

The predecessor repository is at `../laplus`. It holds every commit before
2026-07-28, which is the only place `git blame` on `apps/web` will answer
anything useful, and it is where the tickets' references to sparse-checkout and
`git show HEAD:apps/server/...` were true.

**Its `origin` still points at `github.com/hoangvu12/laplus`.** A `git push` run
from that directory would restore the old tree and the 750 branches this one
deleted. If it is not going to be used again, close the hazard the same way the
`upstream` remote was closed:

```sh
git -C ../laplus remote set-url --push origin DISABLED
```

Nothing has been done to it. It is untouched, and deliberately so.

## 2. An export key and its filename can disagree, and a search will miss it

`packages/client-runtime` exports `./state/relay` from `src/state/relayDiscovery.ts`
and `./state/project-grouping` from `src/state/projectGrouping.ts` — kebab-case
keys, camelCase files. Both were nearly deleted as dead in `15060ce` because a
search for `@t3tools/client-runtime/state/relayDiscovery` finds nothing, and both
are imported by `apps/web`.

What caught it was checking every `exports` entry against the file it names, not
a better search. **Do that check before deleting anything from a package**, in
either direction: a module can look dead because its specifier is spelled
differently from its file.

## 3. Two test failures on Windows are the platform, not the code

Both were found by running the suite here rather than trusting CI, which is
Linux and green on both.

- **Fixed and skipping.** `packages/shared/src/logging.test.ts` provoked a
  non-`ENOENT` `stat` failure with a 300-character filename. Windows answers
  `ENOENT` for that, which is exactly the code the sink reads as "no log file
  yet" — so the case the test exists to distinguish cannot be produced there at
  all. It probes the filesystem and skips rather than branching on
  `process.platform`, which `t3code/no-global-process-runtime` forbids.
- **Deleted.** The other four were `relayClient`, which needed `cloudflared.exe`.
  Nothing imported it and it went with the rest of the dead modules.

A third, in the lint plugin, was a real bug: its 35 tests spawned `.bin/oxlint`,
a shell script Windows cannot spawn, and `oxlint.CMD` fails too because Node
refuses `.cmd` without a shell. They run oxlint's entry under `node` now. They
had never passed on this platform.

---

## What was verified, and how

Not a suite — the application, driven. `tools/ui-driver/probe-boot.mjs` against
`target/debug/laplus.exe` on `LAPLUS_PORT=4774` with a throwaway `LOCALAPPDATA`:

- the window opens, `document.title` is `laplus (Alpha)`, the sidebar and
  composer render;
- `server.getConfig` succeeds, `subscribeServerConfig` streams and is acked,
  `orchestration.subscribeShell` is sent, ping/pong is alive;
- **the console is empty** — ticket 31's boot 404s did not appear;
- `subscribeServerLifecycle` is refused as unimplemented, which is ticket 46 and
  was already known.

Also green: `pnpm typecheck`, `pnpm lint`, `pnpm test` (1598 passed, 1 skipped),
`cargo test --no-fail-fast` (224 passed), `cargo build -p laplus-shell`.

## What is next

Unchanged by any of this: tickets **39–70** under `issues/`, and
`PARITY-LEDGER.md` for what the server does not answer. The contract declares 71
methods and laplus implements 26. Ticket **70** is closed by construction — the
nine upstream workflows it was about do not exist here.

Two offers left open and not taken:

- `server/spike-claude-protocol/` and `server/t3code-electron-to-tauri-migration.md`
  are resolved historical documents that `server/CLAUDE.md` still references.
  They can go.
- The `@t3tools/*` scope can now be renamed without conflicting with anyone.
  ADR-0018 argues it is still not worth the churn.
