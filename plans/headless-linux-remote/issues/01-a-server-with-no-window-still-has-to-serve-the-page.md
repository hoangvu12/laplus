# 01 — A server with no window still has to serve the page

**What to build:** a way for `laplus-server` to serve `apps/web/dist` from a
directory given at runtime, so a browser pointed at a headless laplus gets the
application instead of a 404.

**Status:** ready-for-human — built and driven against the real bundle on
Windows; nobody has yet loaded it from a phone. See **What landed**.

**Depends on:** nothing. This is the first ticket of the effort.

## Why

`laplus-server/src/main.rs` binds with `Assets::none()`, and says why:

```rust
// No assets: this binary answers calls, it does not serve pages. The bundle
// belongs to the shell — see `laplus_server::ui`.
let server = match Server::bind(port, Assets::none()).await {
```

That was right when the only client was a window on the same machine and the
only other caller was a Vite dev server. It is the whole of what stops a phone
working: `Assets::none()` leaves `files` empty, `Assets::resolve` therefore
declines every path, and the `asset` fallback in `crate::server` answers
`StatusCode::NOT_FOUND`. The phone does not land on a pairing screen — it lands
on nothing. `PairingRouteSurface.tsx` and `routes/pair.tsx` exist and are never
reached.

Upstream has no such gap because its server has always served the client:
`ServerConfig.staticDir`, resolved by `resolveStaticDir()`
(`pingdotgg/t3code:apps/server/src/config.ts`), which looks for `client/index.html`
beside the server bundle and then `../../web/dist`. `t3 serve` is a server that
serves pages.

## Do not embed it

The obvious move — copy `laplus-shell/build.rs` into `laplus-server` — is wrong,
and the reason is written in the workspace manifest:

```
default-members = ["crates/laplus-server", "xtask"]
```

with a comment saying the shell is excluded because it embeds `apps/web/dist`,
which needs a `pnpm` build, and leaving it in the default set "would mean a
fresh clone could not run the suite at all". Compile-time embedding in
`laplus-server` would inflict exactly that on the crate that comment exists to
protect: `cargo test` would start requiring a web build.

So the bundle is found at runtime. This is also what makes the two binaries
honest about what they are — the shell ships an application, the server points
at one.

## What makes this cheap

`crate::ui` was already built for owned bytes:

```rust
pub struct Assets {
    files: BTreeMap<String, Cow<'static, [u8]>>,
    version: Option<&'static str>,
}
```

`Cow` means runtime-read bytes need no new type and no lifetime work. The one
thing that has to widen is `version`, which is `&'static str` because the
shell's build script hands it a literal. A bundle read from a directory has a
version too — `apps/web/package.json` — and it must travel with the bytes for
the same reason the comment on that field already gives.

`Assets::resolve` needs no change at all: its three rules (file, 404 for a
missing file, entry point for a client route) and `SERVER_SURFACE` are about
paths, not about where the bytes came from.

## What to build

1. **`Assets::from_directory(path) -> io::Result<Assets>`** — walk the
   directory, key each file by its path relative to the root with `/`
   separators, read the bytes as `Cow::Owned`. Read the version from
   `package.json` beside it if there is one; a bundle without one is not an
   error, it is `None`, and `serving_ui_version` already handles that case.
2. **Widen `version` to `Option<String>`**, and follow it through
   `Assets::version` and `crate::config::ServerConfig::serving_ui_version`.
   Nothing else reads it.
3. **A `--ui <dir>` argument on `laplus-server`**, parsed in `crate::launch`
   beside `--port`, with a `LAPLUS_UI` environment variable behind it in the
   same order the port uses (argument beats environment beats default).
   `launch::port_from` is the shape to copy, including that a malformed value is
   a refusal with a sentence rather than a silent fallback.
4. **A missing or unreadable directory is a startup refusal**, not a warning.
   A server that came up with no UI because a path was misspelled is a 404 the
   user will blame on the feature. `crate::launch`'s own comment argues this
   for the port and it applies unchanged.

## Acceptance criteria

- `laplus-server --ui apps/web/dist` answers `GET /` with `index.html`.
- The same server answers a client route (`/settings`) with the entry point and
  a missing file (`/assets/nope.js`) with 404 — `Assets::resolve`'s existing
  rules, now exercised through the plain binary.
- `laplus-server` with no `--ui` behaves exactly as it does today: 404 at `/`,
  every API route unchanged. `tests/http_boot.rs` must still pass untouched.
- `--ui` naming a directory that does not exist, or that has no `index.html`,
  refuses to start and says which path it tried.
- The server reports the bundle's version as its own — a directory-loaded
  bundle and an embedded one produce the same `serverVersion`.
- `cargo test` still runs on a clone with no `apps/web/dist` present.

## Out of scope

- Changing what the **shell** does. It keeps its build script and its embedded
  table; ADR-0010 and ADR-0011 are untouched.
- Compression, `ETag`, range requests. `Caching` already decides
  `immutable` versus `no-cache` and that is enough.
- Watching the directory for changes. A dev loop points Vite at the server, it
  does not point the server at Vite.

## What landed

`--ui <dir>` on `laplus-server`, with `LAPLUS_UI` behind it in the same
argument-beats-environment order `--port` uses. Without it the binary behaves
exactly as before: 404 at `/`, every route unchanged, `tests/http_boot.rs`
untouched.

`Assets::from_directory` walks the directory and keys each file by its path from
the root **with forward slashes** — the spelling `resolve` matches a URL
against, so a walk that kept the platform's separator would have built a table
answering nothing on Windows. `resolve` itself is unchanged, as this ticket
predicted: its rules are about paths, not about where the bytes came from. Only
`version` widened, to `Option<String>`.

`launch` now has two entry points rather than one. `requested_port` is the
shell's and accepts only `--port`; `requested` is the server's and accepts
`--ui` too. A shared parser that ignored an unknown flag would be a shell
silently disregarding a directory it was told to serve, so `flags_from` takes
the set each binary accepts and refuses anything else.

### The bug that only the real bundle showed

The version is looked for **inside the bundle and then in the directory above
it**, and the second is the case that actually ships. Vite copies no
`package.json` into `dist/`, so the real number lives in `apps/web/package.json`
— the same file `laplus-shell/build.rs` reads through `web_directory()`.

The first implementation looked only inside, and every test passed: the unit
tests wrote a manifest beside the files, and the integration test did too. Run
against the real `apps/web/dist` it served every file correctly and reported
`0.1.1`, the crate's own version, instead of `0.0.28`. That is precisely the
skew ticket 26 exists to prevent, and no test in this repository would have
caught it, because they all built the fixture the convenient way. `AGENTS.md`'s
"a green suite is not evidence the application works" earned its place again.

### Driven, not only tested

`laplus-server --ui ../apps/web/dist` answers `/` with the real 3192-byte page,
`/settings` with the entry point, `/assets/nope.js` with 404, and reports
`serverVersion` 0.0.28. A `--ui` naming a directory that does not exist, or one
with no `index.html`, refuses to start and says which path it tried. 879 tests
pass on Windows.

### What is left

**Nobody has loaded this from a phone.** That is the point of the ticket and it
needs a person with a handset on the same network as a box running this — which
also needs tickets 04 and 03, because the server still binds loopback by default
and still prints `127.0.0.1` as the address to open.
