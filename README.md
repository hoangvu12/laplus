# laplus

A desktop client for the `claude` CLI: a Rust server that drives the agent, a
Tauri window that hosts it, and a React UI.

The UI derives from [t3code](https://github.com/pingdotgg/t3code) (MIT, T3 Tools,
Inc.) and is maintained here. The server and shell are this project's own.

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

## Layout

| Path       | What it is                                    |
| ---------- | --------------------------------------------- |
| `apps/web` | the UI                                        |
| `packages` | `@t3tools/{contracts,client-runtime,shared}`  |
| `server`   | the Rust server, the Tauri shell, the release |
| `.scratch` | the issue tracker and the parity ledger       |

Start with `AGENTS.md`, then `server/CLAUDE.md` for the Rust half.
`.scratch/rust-server-tauri/PARITY-LEDGER.md` is what the server does and does
not yet answer.

## Licence

MIT. See `LICENSE`, and `server/THIRD_PARTY_NOTICES.md` for what ships inside
the binary.
