Status: ready-for-human

# Upstream research — the `t3` CLI surface

Written 2026-08-01, before implementation. The question this answers: **what
does the current `pingdotgg/t3code` CLI expose, and which parts should laplus
copy to make its server command coherent?**

Read from upstream commit
[`0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62`](https://github.com/pingdotgg/t3code/tree/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62)
(2026-07-31). Upstream moves quickly; the links below are commit-pinned.

## Summary

Upstream has one real, hierarchical CLI built with Effect's CLI parser. The
published npm executable and the server are the same Node bundle: package `t3`
maps binary `t3` directly to `dist/bin.mjs`. There is no second launcher with a
separate help page.

The strongest design to copy is the split between the default/start behavior
and headless operation:

- `t3 [cwd] [server flags]` and `t3 start ...` run the ordinary server.
- `t3 serve ...` runs without opening a browser and prints pairing details.
- Administrative operations are nouns with nested verbs (`auth pairing
create`, `project add`, `service status`).
- Every command owns generated, contextual `--help`; `--version` comes from the
  package version passed to `Command.run`.

Laplus currently has two conflicting parsers: `npx laplus` recognizes only
help/version and otherwise forwards, while `laplus-server` hand-parses flags and
does not recognize help/version. Bare execution is always headless serving, and
unknown first words fall through as "unrecognised argument" rather than being
diagnosed as an unknown command. That is the confusion to remove.

## Exact upstream command tree

Declared at [`apps/server/src/bin.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/bin.ts):

```text
t3 [cwd] [server flags]          Run the T3 Code server.
├── start [cwd] [server flags]   Run the T3 Code server.
├── serve [cwd] [server flags]   Headless; do not open a browser; print pairing details.
├── pair [pair flags]            Pair with a running server and print a QR code.
├── auth
│   ├── pairing
│   │   ├── create [auth location flags] [--ttl] [--label] [--base-url] [--json]
│   │   ├── list [auth location flags] [--json]
│   │   └── revoke [auth location flags] <id>
│   └── session
│       ├── issue [auth location flags] [--ttl] [--label] [--subject]
│       │         [--token-only] [--json]
│       ├── list [auth location flags] [--json]
│       └── revoke [auth location flags] <session-id>
├── project
│   ├── add [project location flags] <path> [--title]
│   ├── remove [project location flags] <project> [--force]
│   └── rename [project location flags] <project> <title>
├── service
│   ├── install [project location flags]
│   ├── uninstall [project location flags]
│   ├── update [project location flags]
│   └── status [project location flags]
└── connect [connect flags]
    ├── login
    ├── link
    ├── status
    ├── publish
    ├── unlink
    └── logout
```

`connect` is cloud-product-specific and may be hidden/unavailable in builds
without its public configuration. It is not a useful laplus parity target.

The declarations are in upstream
[`server.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/server.ts),
[`pair.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/pair.ts),
[`auth.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/auth.ts),
[`project.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/project.ts),
[`service.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/service.ts), and
[`connect.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/connect.ts).

## Server flags and argument shape

The root, `start`, and `serve` deliberately share one flag definition in
[`cli/config.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/cli/config.ts):

| Input                                        | Meaning                                                           |
| -------------------------------------------- | ----------------------------------------------------------------- | ------------ |
| optional positional `cwd`                    | Provider-session working directory; defaults to current directory |
| `--mode <web                                 | desktop>`                                                         | Runtime mode |
| `--port <port>`                              | HTTP/WebSocket port                                               |
| `--host <host>`                              | Bind host/interface                                               |
| `--base-dir <path>`                          | Data directory (`T3CODE_HOME` equivalent)                         |
| `--dev-url <url>`                            | Development web URL                                               |
| `--no-browser`                               | Do not open the browser                                           |
| `--bootstrap-fd <fd>`                        | Read one-time bootstrap secrets from an fd                        |
| `--auto-bootstrap-project-from-cwd`          | Create the cwd project when missing                               |
| `--log-websocket-events` / `--log-ws-events` | Log outbound WebSocket traffic                                    |
| `--tailscale-serve`                          | Expose through Tailscale Serve HTTPS                              |
| `--tailscale-serve-port <port>`              | HTTPS port for Tailscale Serve                                    |

The key interface choice is `--host`, not a boolean `--network` plus a separate
`--advertise-host`. It expresses the actual bind target directly. Upstream's
remote guide consequently reads `npx t3 serve --host "$(tailscale ip -4)"` and
points readers to `t3 serve --help` for the complete reference
([`REMOTE.md`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/REMOTE.md)).

## Help and error UX

Descriptions are attached at every level and help is generated from the same
command/flag declarations that parse the invocation. This prevents the drift
laplus currently invites by maintaining the server parser in Rust and the only
help text in `apps/cli/src/invocation.ts`.

Useful upstream details to preserve:

- Help is contextual: `t3 --help`, `t3 serve --help`, and `t3 auth pairing
create --help` describe different scopes.
- Required positionals (`revoke <id>`, `rename <project> <title>`) are declared
  as such, so missing values and malformed nesting are parser errors with help.
- Every flag has a one-sentence description beside its type and validation.
- Aliases are declared beside the canonical flag (`--log-ws-events`).
- `Command.run(cli, { version: packageJson.version })` makes version handling
  part of the same CLI rather than a launcher special case.
- Upstream tests the nested help and parse-error presentation, including
  `service --help` and canonical boolean negation such as
  `--no-log-websocket-events`
  ([`bin.test.ts`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/bin.test.ts)).

## Binary/server relationship

Upstream's [`apps/server/package.json`](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/package.json)
publishes `t3 -> ./dist/bin.mjs`; that entrypoint owns parsing and runs the
server. The root handler and `start` handler are identical. `serve` calls the
same server runner with `startupPresentation: "headless"` and disables automatic
project creation from cwd. The normal and headless experiences are modes of one
executable, not separate command implementations.

Laplus needs its npm shim because Rust binaries are platform-specific. That is
fine, but the shim should be transport only: resolve the platform package,
append its bundled UI location when appropriate, spawn, and forward status.
The Rust binary should own the complete command tree, help, version, validation,
and errors so `laplus-server ...` and `npx laplus ...` behave identically.

## Recommended laplus slice

Copy the structure, not all upstream features:

```text
laplus [cwd] [run flags]       normal/default experience (only if laplus wants browser-open)
laplus start ...               explicit alias for the default
laplus serve ...               current headless server behavior
laplus pair ...                ergonomic QR-code operation against a running server
laplus auth pairing {create,list,revoke}
laplus service {install,update,uninstall,status}
```

Then add `auth session` and `project` only when laplus actually supports their
operations. Do not expose placeholder commands for parity.

Implementation priorities:

1. Put one typed hierarchical parser in `laplus-server`; generate contextual
   help from it and make the npm package pass every argument through.
2. Introduce explicit `serve`; keep bare legacy flags as a compatibility alias
   during migration if changing bare behavior would surprise existing scripts.
3. Adopt upstream's `start`/`serve` distinction and command nouns. Preserve
   `auth pairing` because laplus already matches it; consider the `pair`
   convenience command separately.
4. Add upstream's missing `service update` concept: install/update/repair is a
   clearer lifecycle than overloading `install` plus an "up to date" status.
5. Prefer `--host` as the primary bind control. Retain `--network` temporarily
   as a compatibility alias if needed; keep advertised/public URL selection a
   separate concept when the bind address cannot express a tunnel hostname.
6. Test help snapshots, every command path, missing required positionals,
   unknown commands/flags, and equality of direct-binary versus npm-launcher
   behavior.

## Local comparison points

- `server/crates/laplus-server/src/launch.rs`: handwritten flat parser;
  `service` and `auth` are special cases and everything else is treated as
  serve flags.
- `apps/cli/src/invocation.ts`: separate static help and separate
  help/version interception.
- `apps/cli/src/bin.ts`: the necessary platform-binary resolver and process
  shim; this is the part worth retaining.
- `server/docs/running-headless.md`: current public surface and compatibility
  promises.
