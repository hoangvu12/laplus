# laplus

A desktop-grade client for the `claude` CLI, started from a terminal:

```sh
npx laplus@latest
```

That downloads the server for your machine, starts it on `http://127.0.0.1:4773`
and prints a URL with a pairing token in it. Open the URL.

## What it needs

**The `claude` CLI, installed and authenticated on the machine the server runs
on.** laplus drives the agent where the server is, not where your browser is, so
a server on another box needs its own `claude` login rather than inheriting
yours.

Node 20 or newer, to run this launcher. The server itself is a Rust binary and
needs nothing.

## Flags

All of them belong to the server; this package passes them straight through.

| Flag                      | What it does                                               |
| ------------------------- | ---------------------------------------------------------- |
| `--port <n>`              | the port to listen on. Default 4773, or `LAPLUS_PORT`      |
| `--network`               | leave loopback, for this run only, so a phone can reach it |
| `--advertise-host <host>` | the host to print for other machines to reach              |
| `--ui <dir>`              | serve a bundle other than the one in this package          |

```sh
npx laplus@latest --network            # reachable from your phone on this network
npx laplus@latest --port 5000
```

## Keeping it running

On Linux with systemd, laplus can install itself as a background service for
your user — it starts at boot and survives you logging out of SSH.

```sh
npx laplus@latest service install --network
npx laplus@latest service status
npx laplus@latest service uninstall
```

The flags you pass to `service install` are the ones the service runs with.

## Pairing another device

A server started at boot has no terminal to print a pairing URL to, and any
server has only one startup credential. Mint a code whenever you need one:

```sh
npx laplus@latest auth pairing create
npx laplus@latest auth pairing create --ttl 1h --label "ipad"
npx laplus@latest auth pairing list
npx laplus@latest auth pairing revoke <id>
```

`create` prints the code, a URL, and **a QR code** — point a phone's camera at
it and there is nothing to type. Codes are single-use and last five minutes
unless `--ttl` says otherwise. Add `--base-url` when the address a device
reaches you at is a tunnel or a tailnet name this machine cannot discover, and
`--json` for a script.

This works against a server that is already running, and never has to find it:
a pairing code is a row in the database rather than server state.

`--network` turns the listener off loopback **for one run** and does not change
what the desktop app does next time it starts. Anything it can reach can drive
an agent on that machine, so the credential printed at startup is the whole of
the boundary — treat it accordingly, and read
[remote access](https://github.com/hoangvu12/laplus/blob/main/server/docs/running-headless.md)
before leaving it running on a network you do not control.

## Platforms

Binaries are published for Windows x64, Linux x64 and arm64, and macOS on both
architectures. They arrive as `optionalDependencies`, so npm downloads exactly
one of them and skips the rest.

The Linux binaries are statically linked against musl, so they carry no libc
dependency and there is no distribution too old for them — Ubuntu 20.04, RHEL 8
and Alpine all run the same file.

On any platform not in that list, build the server yourself:

```sh
git clone https://github.com/hoangvu12/laplus
cd laplus/server
cargo build -p laplus-server --release
```

## The desktop app is a different thing

This is the server and its web UI. The Windows desktop application is a Tauri
window with the same server inside it, published as an installer on
[GitHub Releases](https://github.com/hoangvu12/laplus/releases).

MIT. Source at [hoangvu12/laplus](https://github.com/hoangvu12/laplus); this
package is built from `apps/cli`.
