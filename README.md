# laplus

A desktop client for the `claude` CLI: a Rust server that drives the agent, a
Tauri window that hosts it, and a React UI.

The UI derives from [t3code](https://github.com/pingdotgg/t3code) (MIT, T3 Tools,
Inc.) and is maintained here. The server and shell are this project's own.

## Running it without building it

```sh
npx laplus@latest
```

Downloads the server for your platform, starts it on `http://127.0.0.1:4773` and
prints a URL with a pairing token in it. `--network` makes it reachable from a
phone on the same network; `npx laplus@latest --help` lists the rest.

The machine the server runs on needs its own `claude` installed and
authenticated — laplus drives the agent where the server is, not where the
browser is. `server/docs/running-headless.md` is the long version, and
`apps/cli/` is the launcher itself.

This is the server and the page, not the window. The desktop application is the
installer on [Releases](https://github.com/hoangvu12/laplus/releases).

## Building it

The two halves are built by different tools, and the shell needs the UI's output
baked into it:

```sh
pnpm install
pnpm build:web                  # produces apps/web/dist
pnpm app                        # cargo run -p laplus-shell — the window
```

`cargo build -p laplus-shell` will not build the UI for you; it embeds
`apps/web/dist` from a fixed path and says so if it is missing.

## Working on it

```sh
pnpm dev:server                 # the Rust server, no window
pnpm dev                        # the UI against it, with HMR

pnpm test                       # the TypeScript suites
pnpm test:server                # the Rust suite (--no-fail-fast)
```

`server/` is the Cargo workspace root — `Cargo.toml` and `target/` are there
rather than at the repository root, so cargo commands run from that directory
unless you pass `--manifest-path`.

## Releasing it

```sh
cd server
TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/laplus.key)" cargo xtask release
```

Builds the Windows installer, signs it for the updater, and weighs the result.
**The key is not optional**: an installed laplus only accepts an update signed by
the key whose public half is in `tauri.conf.json`, so a build without it fails
rather than producing an installer nobody can update from.

Pushing a `v*.*.*` tag runs the same thing on CI, signs it with the repository's
`TAURI_SIGNING_PRIVATE_KEY` secret, and attaches the installer and `latest.json`
to a GitHub Release — which is the feed an installed copy checks. The tag must
match the version in `server/crates/laplus-shell/tauri.conf.json`.

The installer is not code-signed for Windows, which is a different signature
entirely, so SmartScreen will warn about an unknown publisher.
`server/docs/adr/0020` records both, and ticket 74 the update path.

The same tag also publishes `laplus` to npm — the launcher behind `npx laplus`,
and a `@laplus/server-*` package per platform carrying a `laplus-server` binary.
That half needs no signing key and an `NPM_TOKEN` secret instead;
`server/docs/adr/0026` is the shape and why.

A `workflow_dispatch` run builds the same six packages and versions them
`<version>-rc.<run>`, which sorts below the release that number will name later.
Two switches decide what happens to them:

```sh
gh workflow run release.yml --ref main                        # pack only
gh workflow run release.yml --ref main -f publish_npm=true    # publish as latest
```

Without `publish_npm` the tarballs are left on the run as an `npm-tarballs`
artifact, to be installed from files. With it they go to npm and `latest` moves,
so `npx laplus` answers — a prerelease under `latest` is still what that command
resolves. Either way no installer is built and no release is cut; the installer
needs `-f build_installer=true`, because forty minutes of Tauri answers nothing
about the npm half.

**A published version is permanent.** npm does not let a version number be
reused, so the run number in `-rc.<run>` is doing real work.

## Layout

| Path       | What it is                                    |
| ---------- | --------------------------------------------- |
| `apps/web` | the UI                                        |
| `apps/cli` | `npx laplus` — the launcher published to npm  |
| `packages` | `@t3tools/{contracts,client-runtime,shared}`  |
| `server`   | the Rust server, the Tauri shell, the release |
| `.scratch` | the issue tracker                             |

Start with `AGENTS.md`, then `server/CLAUDE.md` for the Rust half. The server
answers 33 of the 61 socket methods `packages/contracts` declares; the rest are
not implemented yet. `.scratch/contract-parity/ledger.md` is the count.

## Licence

MIT. See `LICENSE`, and `server/THIRD_PARTY_NOTICES.md` for what ships inside
the binary.
