# Running `laplus-server` on Linux

laplus without a window: the server on a Linux box, driven from a phone's
browser. This page is what an operator needs — what to install, what to run, how
to get a device paired, and what turning this on actually costs.

**The desktop application is not this.** `laplus-shell` is Tauri and would need
WebKitGTK; it is excluded from the workspace's `default-members` and stays
excluded. What runs here is the server.

## Build prerequisites

A Rust toolchain, and a C compiler.

```
sudo apt install build-essential            # Debian, Ubuntu
sudo dnf groupinstall "Development Tools"   # Fedora, RHEL
sudo pacman -S base-devel                   # Arch
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
  `crate::provider` resolves it by walking `PATH`. **Read the next section
  before running this under a service manager**, because that `PATH` is not the
  one you get in a terminal.
- **A shell**, for the terminal feature. `crate::terminal` tries `$SHELL`, then
  `/bin/zsh`, `/bin/bash`, `/bin/sh`.

### Nothing user-local is on a non-interactive `PATH`

This costs an hour if you meet it the other way round, so it is worth stating
before you meet it. It was written here as a `claude` problem and it is not one
— it is a property of every user-local toolchain on a box like this. Setting one
up for the 2026-07-30 drive hit it **three separate times**: `claude` in
`~/.local/bin`, `node`/`corepack` under `~/.nvm`, and `cargo` under `~/.cargo`.
Each is wired in through `~/.profile` or `~/.bashrc`, and neither of those is
read by `ssh host cmd`, by `nohup bash`, or by systemd.

`~/.bashrc` is the sharper edge of the two, because it looks like it would work.
Ubuntu's `~/.profile` does source it for a bash login shell — but `~/.bashrc`
opens with

```sh
case $- in
    *i*) ;;
      *) return;;
esac
```

so it returns immediately unless the shell is **interactive**. `ssh box` then
typing works; `ssh box 'node -v'` and `bash -lc 'node -v'` both find nothing.

Taking `claude` as the example, since it is the one that breaks laplus rather
than the build: the installer puts it in `~/.local/bin`, which `~/.profile` adds
for **login shells only** — so:

```
ssh box                       # login shell: `which claude` answers
ssh box 'which claude'        # non-login shell: answers nothing
systemd, cron, a container    # answers nothing
```

`crate::provider` walks `PATH` and finds nothing, so laplus reports no provider
and every turn fails — on a machine where `claude` is installed, authenticated
and working when you check it by hand. Nothing about the failure points at
`PATH`.

Two ways out. Give the process the path explicitly:

```
Environment=PATH=/home/ubuntu/.local/bin:/usr/local/bin:/usr/bin:/bin
```

or put the binaries somewhere already on every `PATH`, which is what the test
box does — one symlink each into `/usr/local/bin`:

```
sudo ln -sfn ~/.local/bin/claude /usr/local/bin/claude
```

The second makes the machine behave the same however it is invoked, which is
worth a lot when the failure it prevents is silent. Its cost is that a symlink
pins a particular install: `nvm install 26` or a `rustup` toolchain change will
not be picked up until the link is refreshed, and `node -v` can then disagree
with itself depending on how you ask.

`ssh box 'echo $PATH'` — with the quotes, so the remote shell expands it — is
the check worth running before blaming laplus. `laplus: agent binary <path>` in
the startup output is the confirmation that it worked; its absence is the
symptom.

## Running it

Build the release binary from `server/`, which is the Cargo workspace root:

```
cargo build --release -p laplus-server
./target/release/laplus-server --ui ../apps/web/dist --network
```

### The flags

| Flag         | Environment      | Default              | What it does                                           |
| ------------ | ---------------- | -------------------- | ------------------------------------------------------ |
| `--port <n>` | `LAPLUS_PORT`    | `4773`               | The port to listen on. `0` asks the OS for a free one. |
| `--ui <dir>` | `LAPLUS_UI`      | none                 | Serve the web bundle from this directory.              |
| `--network`  | `LAPLUS_NETWORK` | `remote-access.json` | Bind `0.0.0.0` instead of `127.0.0.1`.                 |

An argument beats the environment, which beats the default. An unrecognised flag
or an unparseable value is a refusal with a sentence rather than a silent
fallback — a server that started on a port you did not ask for is a server you
then cannot find.

**`--ui` is not optional for a phone.** Without it this binary answers `404` at
`/`: it is a socket endpoint, and the UI is expected to come from somewhere
else. A browser has no application to start, so the page and the API have to
come from the same place. Point it at a built `apps/web/dist` — built by `pnpm
install && pnpm --filter @t3tools/web build` from the repository root, which
`cargo build` will not do for you. A bundle that will not load stops the server
rather than starting one that serves nothing.

**`--network` decides this run only.** It does not write
`remote-access.json`; the next start reads that file as though the flag had
never been passed. That is deliberate — one `laplus-server --network` run that
rewrote the file would silently change what the desktop application does on its
next launch on that machine. `docs/adr/0023` is the full record, including why
the flag can also turn exposure _off_ (`--network=false`, over a file that says
otherwise). `true`, `1`, `on`, `yes` and their opposites are all accepted, for
the sake of unit files and `docker run -e`.

### What it says at startup

Everything a Settings panel would have shown you, because there is no Settings
panel:

```
laplus: serving the UI from ../apps/web/dist (0.0.28)
laplus: listening on ws://127.0.0.1:4773/ws
laplus: network access is on, from --network — this server is on your network
laplus: open http://192.168.1.42:4773/#token=ABCD2345WXYZ
laplus: or open http://192.168.1.42:4773/ and pair with ABCD2345WXYZ
laplus: on this machine, http://127.0.0.1:4773/#token=ABCD2345WXYZ
```

The `listening on` line is the socket for a development server on this machine,
and it names loopback on purpose — `0.0.0.0` is an address to bind and not one
to connect to. The URLs below it are the ones to use.

The `network access is` line names **which** of `--network`, `LAPLUS_NETWORK`
and `remote-access.json` decided the mode, because the flag does not persist and
the three can therefore disagree. `by default` means none of them did — there is
no `remote-access.json` on this machine. It reports what the listener actually
bound, not what was asked for.

The two after it are the same address twice, and the second is usually the
easier one to type: a bare `http://192.168.1.42:4773/` followed by twelve
characters into the box on the pairing screen beats getting `/#token=` right in
the middle of a URL on a phone keyboard. Both are built from a routing-table
lookup — `crate::endpoints::lan_address` `connect`s a UDP socket at TEST-NET-3
and reads back the local address, which sends no packets — so the host is the
one this machine would send from, and the same one your other traffic takes.

Two states that are not that:

- **`network access is off`** and one loopback URL. Nothing is wrong; the switch
  is not on. Pass `--network`.
- **`no network address was found: this machine has no route off itself`.** The
  port is open and nothing can reach it: no default route, or an interface that
  is down. This is a machine problem, not a laplus one.

## Pairing a phone

1. Put the phone on the same network as the box.
2. Either type the whole `open http://<lan-address>:4773/#token=…` line into it,
   or type the bare address from the `or open …` line and put the twelve
   characters into the pairing screen. The credential is not optional in either
   case: the address on its own lands on a pairing screen with nothing to type
   into it.
3. That is it. The page trades the token at `POST /oauth/token` for a bearer,
   stores a connection profile, and opens the socket. There is no code to read
   off a second screen, because on a headless box there is no second screen to
   read it off.

**The fragment is why this is safe to print.** A URL fragment is never sent to
the server — the browser keeps it and hands it to the page's JavaScript — so the
credential reaches the page without travelling over HTTP.

**This credential is reusable and lives 24 hours.** Unlike a pairing code minted
in Settings, which is single-use and expires in five minutes, the boot grant
survives being spent so that a page reload does not lock you out of your own
window. On a headless box that also means it pairs a second device, and a third.
Treat the startup output as a secret: anyone who can read your terminal
scrollback, your `journalctl`, or the file you redirected stdout into can pair
with this server for the next day. Restarting the server retires the old grant
and mints a new one.

Once paired, the phone holds a session that is good for thirty days.

## What this costs

Not softened, because it is the whole of the security model. ADR-0022 wrote the
honest version and it applies more here, not less:

> turning this on puts a process that runs `claude` as you, with your terminals
> and your filesystem behind it, on your network.

What makes it defensible is that reaching the port is not the same as being let
in: an absent credential is refused, a pairing code is twelve characters and
single-use with a five-minute life, and a session is a row that can be revoked.
A stranger who finds the port gets a pairing screen.

Three things are specifically worse on a box you do not sit in front of.

**HTTP, not HTTPS.** This server speaks plain HTTP and will not be growing a
certificate loader. Everything on the path — every switch, every access point,
anyone on the same Wi-Fi — sees the traffic, the bearer token and the pairing
credential in that startup URL. On a home network behind one router that is a
considered risk. On anything else it is not acceptable, and the answer is to put
**Tailscale or a tunnel with TLS in front of it and leave the listener on
loopback**. That is the recommended deployment for anything reachable from
outside your own flat, not a footnote to it; a tailnet name reaches this server
with nothing written down, because laplus keeps no list of the origins it will
hear from.

**A session handed to a device cannot be revoked.** `auth.clients` is not
implemented. Pairing _links_ can be revoked — `/api/auth/pairing-links/revoke`
withdraws a code that has not been spent — but a session already issued to a
device is good for its thirty days and there is no UI that will take it back. On
a desktop machine ticket 73 called that "nice, not load-bearing". On a server it
is a gap, and it is stated as one: if a paired phone is lost, the recourse is to
delete the rows from `auth_sessions` in the SQLite database and restart.

**Nobody is watching.** Every failure here is a line on a box with no window,
and `laplus-server` **writes no log file**. It prints to stdout and stderr and
nothing else; the `logs/` directory that `observability.logsDirectoryPath`
advertises to the UI is written only by the desktop shell, and only when the
shell fails to start. So the log is wherever you put it, and putting it
somewhere is your job:

```
ExecStart=…                          # systemd: journalctl -u laplus -f
laplus-server … >> ~/laplus.log 2>&1 # or redirect both streams yourself
```

Both streams, not just one. The startup announcement, a bundle that would not
load, a `remote-access.json` that would not parse, and a socket that stopped are
split across `stdout` and `stderr` on purpose — the ordinary output and the
things that went wrong — and capturing only the first loses exactly the half you
will want.

## Where laplus keeps its files

`$XDG_DATA_HOME/laplus` if that is set, otherwise `$HOME/.laplus`
(`config.rs`, `data_dir`). The SQLite database, the `logs` directory,
`keybindings.json`, `settings.json` and `remote-access.json` are all in there.

`remote-access.json` is what the desktop application's Settings switch writes,
and what `--network` overrides without touching. Hand-writing it is still an
option and is the only way to make network access the machine's default:

```json
{ "mode": "network-accessible" }
```

A mode this server does not recognise, or a file that will not parse, is a
complaint on stderr and a fall back to loopback — the failure mode of a typo is
a phone that cannot connect rather than a port open to the network.

## What the machine calls itself

The `environment.label` a paired client shows comes from `COMPUTERNAME`, then
`HOSTNAME`, then `/etc/hostname`. The third was added by ticket 05: `HOSTNAME`
is a _shell_ variable on most distributions and is not exported, so a server
started by systemd or over `ssh host cmd` sees neither of the first two — and a
headless laplus answered `"laplus"`, which distinguishes no machine from any
other at exactly the moment there is more than one.

A container is the exception worth knowing: Docker exports `HOSTNAME`, so the
second source answers there and the file is never read.

A box with no `/etc/hostname` still answers `"laplus"`. That is the honest
fallback rather than a bug.

## Known gaps

**No systemd unit ships with this.** The two things such a unit has to get right
are the explicit `PATH` above and capturing both streams.

## What has and has not been checked

**In CI, on Linux:** `cargo build -p laplus-server` and
`cargo test -p laplus-server --no-fail-fast` on `ubuntu-latest`, every push and
pull request touching `server/`.

**By hand, on Windows**, driving the real binary against the real
`apps/web/dist` — tickets 03 and 04. All four sources of the exposure mode
(`--network`, `--network=false`, `LAPLUS_NETWORK`, `remote-access.json`, and its
absence) announce themselves correctly; a malformed value is refused; and over
the machine's own LAN address, `GET /` served the page, the printed credential
exchanged at `POST /oauth/token` for a thirty-day bearer, that bearer was
accepted at `/api/auth/session`, and the descriptor reported
`policy: remote-reachable`. The boot grant was confirmed reusable by spending it
twice.

**By hand, on Linux, from a phone** — 2026-07-30, and this is the one that
matters. An Oracle Ampere instance: aarch64, Ubuntu 20.04, 3 cores. The server
bound to loopback with `--ui`, a `cloudflared` quick tunnel in front of it, and
a phone's browser on the far end of the public internet.

The page loaded and paired from the URL's fragment with nothing typed, the
descriptor answered `"os":"linux","arch":"arm64"`, an uncredentialed
`/api/orchestration/shell` was still refused `401`, and **a turn ran to
completion**. That last one is the whole chain on hardware that had never
executed any of it: the socket upgraded through the tunnel, `crate::provider`
resolved `claude` on arm64, the CLI streamed back, and the settling pipeline
rendered it.

**Still not checked, and worth being exact about:**

- **The terminal on Linux.** A turn is the agent path; the pty is separate code
  (`portable-pty` on `openpty(3)`) and no hand-driven session has opened one on
  Linux. This is the single largest thing a green suite is not speaking for.
- **A phone against `--network` on the same LAN.** The drive above went through
  a tunnel, so the loopback path and HTTPS were exercised and the wildcard bind
  was not.
- **The URL the server printed.** It was not the URL used. On a cloud instance
  the printed host is the private VCN address (`10.0.0.136` on that box), which
  no phone can reach, so the working URL was assembled by hand against the
  tunnel hostname. Nothing the server can inspect will find a NATed public
  address — see the note on advertising a host below.
