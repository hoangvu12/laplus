# ADR-0021 — The page commands the shell through a named list, not through a bridge

Date: 2026-07-29
Status: Accepted

Completes the half [ADR-0020](0020-this-fork-publishes-an-installer-and-does-not-yet-update-itself.md)
deferred: laplus now updates itself, so that ADR's "self-update is deferred, not
refused" is spent. What it says about publishing, signing and SmartScreen still
holds.

## Context

Upstream is Electron, and its renderer talks to its main process through one
object: `window.desktopBridge`, put there by a preload script. Everything the
page cannot do for itself goes through that — update state, window controls,
native dialogs, the lot. Whether the bridge is present is also how upstream's UI
decides what kind of application it is.

laplus has never had one. The window is pointed at `http://127.0.0.1:<port>`
rather than at a Tauri scheme (ADR-0010), there is no preload, and ticket 27
established the rule that the UI is told it is _Tauri_ — a narrow, true claim in
`isDesktopShell` — rather than told it is Electron, which would send every one of
those calls at something absent.

Ticket 74 is the first time the page needed the shell to do something
substantial: replace the application. The update pill upstream wrote reads
`window.desktopBridge.getUpdateState`, and the question was how to feed it.

### What was rejected, and why

**Defining a partial `window.desktopBridge`.** The obvious move, and it breaks
things far from the update code: `ConnectionsSettings`, `branding` and the
in-app browser all branch on that object's presence, and a partial one tells them
a full Electron bridge is there. A pill that works, bought with several features
that silently misbehave, is a bad trade.

**Custom `#[tauri::command]`s of our own.** This is what a Tauri application
normally does, and it does not fit the shape laplus already has. From `tauri`'s
`webview/mod.rs`, with its own comment: a command from a **remote** origin is
refused unless an explicit `remote` capability was configured for it — and this
page _is_ a remote origin, because it is served over loopback. Reaching custom
commands from it means shipping an application ACL manifest, which is a larger
change than the feature that wanted it.

**Making the version-skew banner fire.** The other available surface, and it
answers a different question. Skew means "this client and this server are
different builds"; the two halves here are one executable, so the numbers can
only differ by being made to, and the banner would then be permanently lit — the
exact bug ticket 26 filed and ADR-0011 fixed. Its button also updates the
_server_ toward the _client_, which is backwards for "a newer application exists".

## Decision

**Anything the page may ask the shell to do is a line in a capability file, and
the shell's test suite asserts that line exists.**

The mechanism is Tauri's plugin commands over the remote-origin ACL, which
`capabilities/titlebar.toml` already used for the window buttons. Ticket 74 adds
`capabilities/updater.toml` for `updater:allow-check`, `allow-download` and
`allow-install` — and deliberately not `allow-download-and-install`, which
nothing presses.

Three consequences of stating it that way:

- **The privilege list is one grep.** What a page loaded over loopback can do to
  this machine is two files, each with a `[remote]` section naming the host it
  trusts.
- **Not `updater:default`.** A permission set that grants a command nothing calls
  is a grant nobody is checking.
- **The grant is asserted, not assumed.** A missing permission is a control that
  renders correctly and does nothing, which ticket 27 already learned the
  expensive way. `the_page_may_check_for_download_and_install_an_update` fails
  the build instead.

`window.desktopBridge` stays `undefined`. The two readers of update state ask
`apps/web/src/shellUpdate.ts` instead, which answers over the IPC in the window
and `undefined` anywhere else.

## Consequences

- **A phone gets none of this.** Tauri's IPC exists only in the webview, so a
  paired browser sees no bridge and the pill never draws. That is the right
  answer rather than a limitation: replacing the application on somebody's PC is
  not a thing a phone should offer, and it is the first clean answer to ticket
  73's outstanding `window.desktopBridge` audit — the feature degrades by
  disappearing, which is what that audit was asked to decide per feature.

- **This is the trusted channel ADR-0019 said the page did not have**, and it
  does not weaken that ADR. The argument there was that anything _served over
  HTTP_ is available to anyone who can make an HTTP request, which is why the
  boot grant travels in a URL fragment. The IPC is injected by the webview
  rather than served, and is unreachable from a browser, so it carries a
  different guarantee — one that a phone, correctly, cannot use.

- **Every new shell capability is now two edits and a test**, and that friction
  is the point. The alternative — one bridge object that grows — is how the
  privilege list stops being readable.

- **A release cannot be built without the signing key.**
  `bundle.createUpdaterArtifacts` makes the bundler sign, so `cargo xtask
release` fails without `TAURI_SIGNING_PRIVATE_KEY`. That is a real cost to a
  fresh clone and is recorded in `README.md` and `server/CLAUDE.md` rather than
  left to be discovered.

- **The key is the single point of failure for every install.** An installed
  laplus accepts only an update signed by the key whose public half is compiled
  into it. Losing it strands every copy on the version it has, with no route out
  but a hand-installed download; leaking it is somebody else able to publish an
  update to all of them. `.gitignore` refuses `*.key` in the tree, and the only
  two places it belongs are the maintainer's password manager and the
  repository secret.

- **The artifact grew.** The plugin brings `reqwest`, `rustls` and `zip`, which
  the binary did not carry. `cargo xtask release` weighs every build and writes
  `docs/artifact-size.md`, so the cost against ticket 24's 20–30 MB target is
  measured on every release rather than argued here.
