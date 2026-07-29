# Running `laplus-server` on Linux

**What this page is, today.** Ticket 05 of the headless-Linux effort put
`laplus-server` on a Linux runner and had one thing to hand on: what a machine
needs installed before `cargo build` will work. That list is below. Ticket 04
writes the rest — which binary to run, `--ui`, `--port`, `--network`, how to
pair a phone, and the security posture — and this file is where it goes.

## Build prerequisites

A Rust toolchain, and a C compiler.

```
sudo apt install build-essential      # Debian, Ubuntu
sudo dnf groupinstall "Development Tools"   # Fedora, RHEL
sudo pacman -S base-devel             # Arch
```

The C compiler is the non-obvious one, and it is not optional:
`rusqlite` is taken with its `bundled` feature, which compiles SQLite from
source through the `cc` crate. Without a compiler the build fails at that
dependency with

```
error: failed to run custom build command for `libsqlite3-sys`
  ... failed to find tool "cc"
```

which reads like a missing Rust component and is not one. `ubuntu-latest` in CI
already carries a toolchain, so `.github/workflows/rust.yml` installs nothing
extra — a bare box, a minimal container image, or a distroless base does not.

Nothing else in the dependency set needs a system library. `portable-pty` uses
`openpty(3)` from libc, `notify` uses inotify, and the rest is pure Rust.

## What the server needs at runtime

- **The `claude` CLI, installed and authenticated on this box.** laplus drives
  the CLI where the server runs, which is the whole point of running it here —
  so the machine needs its own provider setup rather than inheriting yours.
  `crate::provider` resolves it by walking `PATH`.
- **A shell**, for the terminal feature. `crate::terminal` tries `$SHELL`, then
  `/bin/zsh`, `/bin/bash`, `/bin/sh`.

## Where laplus keeps its files

`$XDG_DATA_HOME/laplus` if that is set, otherwise `$HOME/.laplus`
(`config.rs`, `data_dir`). The SQLite database, the logs, `keybindings.json`,
`settings.json` and `remote-access.json` are all in there.

## What the machine calls itself

The `environment.label` a paired client shows comes from `COMPUTERNAME`, then
`HOSTNAME`, then `/etc/hostname`. The third was added by ticket 05: `HOSTNAME`
is a _shell_ variable on most distributions and is not exported, so a server
started by systemd, by Docker, or over `ssh host cmd` sees neither of the first
two — and every headless laplus answered `"laplus"`, which distinguishes no
machine from any other at exactly the moment there is more than one.

A box with no `/etc/hostname` still answers `"laplus"`. That is the honest
fallback rather than a bug.

## What has and has not been checked on Linux

**Checked, in CI:** `cargo build -p laplus-server` and
`cargo test -p laplus-server --no-fail-fast` on `ubuntu-latest`, every push and
pull request touching `server/`.

**Not yet checked:** a hand-driven session — starting the server on a real Linux
box, pairing a browser, opening a terminal, and running a turn. `AGENTS.md` is
blunt that a green suite is not evidence the application works, and the pty and
the provider are exactly the two things a suite speaks for least. Ticket 05
leaves this open; write what it finds here.
