# Driving the real UI

A headless browser pointed at a running laplus, over the Chrome DevTools
Protocol. Written for **ticket 28**, which could not be diagnosed any other way:
the server was correct, every event was correct, and the only place the bug
existed was in what the client did with them.

Sibling of `tools/wire-capture/`, and the opposite end of the same wire. That
proxy records what the *reference server* answers; this drives what the *real
client* does. Neither is a test — both are ways of finding out.

## Why a browser at all

`crate::ui` serves the UI from the same origin as the socket (ADR-0010), so
`http://127.0.0.1:4773/` in an ordinary browser is the whole application, with no
webview and no rebuild. That is what makes this possible, and it is worth knowing
before reaching for Tauri's devtools feature.

## Running it

Build and launch the shell — it is what carries the bundle:

```
cargo build -p laplus-shell --release
./target/release/laplus.exe &
node tools/ui-driver/repro.mjs 40
```

| File | What it does |
| --- | --- |
| `cdp.mjs` | Launches headless Chrome, attaches, and exposes the DOM, the console, and **every WebSocket frame in both directions** |
| `probe-boot.mjs` | Boots the UI and dumps what it says on the socket. Start here. Takes the URL as an argument, for the second-instance recipe below |
| `probe-open-thread.mjs` | Clicks a conversation in the sidebar and prints the pane. Ticket 28's free discriminator |
| `repro.mjs` | Types a prompt into the composer of a new thread and watches the pane. Exit code 1 if the reply never renders |

`repro.mjs` spends a **real agent turn** against the configured `claude` binary
and the project the sidebar opens on. The other two spend nothing.

Set `CHROME` if yours is not at the Windows default. `probe-open-thread.mjs` is
the one file here that is *not* general: it names the thread id and the sidebar
row from the machine ticket 28 was found on, and wants both changed before it
means anything elsewhere.

## Looking at a change without closing the laplus already open

Start a second one somewhere else, and point the probe at it:

```
LOCALAPPDATA=/tmp/lc-probe LAPLUS_PORT=4774 ./target/release/laplus.exe &
node tools/ui-driver/probe-boot.mjs http://127.0.0.1:4774/
```

`LOCALAPPDATA` gives it a profile of its own — an empty registry, and no share of
the running instance's SQLite file. Copy `state.sqlite` in from the real one if
the screen you are looking at needs a project to exist. The port is what makes it
a **fresh browser profile** too, since `localStorage` is scoped per origin: on a
new port the UI has forgotten every banner that was ever dismissed, which is the
state ticket 26 was about.

Ticket 26 also found the reason this recipe is needed at all: a running
`laplus.exe` holds a **lock on its own file**, so `cargo build -p
laplus-shell` cannot relink while one is up — it fails with `Access is
denied. (os error 5)`. `--release` writes a different file, which is the way past
it that does not involve closing somebody's window.

## The frame log is the point

`frameLog()` is what solved ticket 28. It shows the client's own traffic — which
thread id it subscribed to, what the server sent back, and what the client then
failed to do with it. Reading the server's logs would have shown a correct
server forever, because that is what it was.

`repro.mjs` writes every frame to `frames.log` beside these files, marking the
one the prompt was submitted at. It is rewritten on every run and gitignored —
it is output, and it is a transcript of a live session.

## Two things that will waste an hour

- **A subscription needs `Ack`.** The server sends one unacknowledged chunk and
  stops (`crate::subscriptions`, first rule). This is not a problem for the
  driver — the real client acknowledges everything — but it is the trap any
  hand-written probe falls into, and ticket 28 records an earlier one that did.
- **Assert on the symptom, not on a substring.** The first version of
  `repro.mjs` looked for the reply text anywhere on the page and went green
  immediately: the sidebar shows the thread's title, and the title is the
  prompt. It now reads the pane only, and asserts the spinner has stopped.
