# Driving the real UI

A headless browser pointed at a running laplus, over the Chrome DevTools
Protocol. Written for **ticket 28**, which could not be diagnosed any other way:
the server was correct, every event was correct, and the only place the bug
existed was in what the client did with them.

Sibling of `tools/wire-capture/`, and the opposite end of the same wire. That
proxy records what the _reference server_ answers; this drives what the _real
client_ does. Neither is a test — both are ways of finding out.

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

| File                         | What it does                                                                                                                                                                                                                                                            |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cdp.mjs`                    | Launches headless Chrome, attaches, and exposes the DOM, the console, and **every WebSocket frame in both directions**                                                                                                                                                  |
| `probe-boot.mjs`             | Boots the UI and dumps what it says on the socket. Start here. Takes the URL as an argument, for the second-instance recipe below                                                                                                                                       |
| `probe-open-thread.mjs`      | Clicks a conversation in the sidebar and prints the pane. Ticket 28's free discriminator                                                                                                                                                                                |
| `repro.mjs`                  | Types a prompt into the composer of a new thread and watches the pane. Exit code 1 if the reply never renders                                                                                                                                                           |
| `first-turn.mjs`             | The same, sampling four times a second. Ticket 35's "the first message doesn't show anything"                                                                                                                                                                           |
| `probe-context-meter.mjs`    | Runs a turn and reads the composer's context meter. Exit code 1 if no meter is drawn — which is what a server that never emits `context-window.updated` looks like from the outside                                                                                     |
| `titlebar-boxes.mjs`         | Where everything in the topbar's right-hand corner is, in pixels in from the window edge. `--plain` measures the browser layout for comparison                                                                                                                          |
| `remote-pairing.mjs`         | Pairs a page served by one laplus with a **second laplus on another origin**, and reports every preflight Chrome sent. Ticket 02 of the headless-Linux effort; wants two servers                                                                                        |
| `add-remote-environment.mjs` | The same, through the real Settings form rather than `fetch`. Takes **one or more** `<remote-url> <code>` pairs and insists the list grows by one each time — ticket 06's acceptance check, which was two servers colliding on `environmentId: "local"` until it landed |
| `surface-walk.mjs`           | Navigates every route, enumerates the visible controls, and reports refusals, empty renders and console errors per route                                                                                                                                                |
| `surface-actions.mjs`        | Presses controls and reports what the server answered, plus every failed HTTP request the page made                                                                                                                                                                     |

`repro.mjs`, `first-turn.mjs` and `probe-context-meter.mjs` each spend a **real
agent turn** against the configured `claude` binary and the project the sidebar
opens on. Everything else here spends nothing.

The two `surface-*` files answer a different question from the rest: not "why is
this one thing wrong" but "of everything the application offers, what does
nothing?" They read the **socket** rather than the DOM for the answer — a control
that reaches an unimplemented method produces "Method not implemented by this
server: …" and one that reaches an unparsed command names it — so neither has to
guess from the page whether a click did anything. Both key on that **sentence**
rather than on the error's `_tag`, which since ticket 39 is whatever the method
in question declares.
The surface walk that first run produced was written up under `.scratch/` and
deleted on 2026-07-29.

Set `CHROME` if yours is not at the Windows default. `probe-open-thread.mjs` is
the one file here that is _not_ general: it names the thread id and the sidebar
row from the machine ticket 28 was found on, and wants both changed before it
means anything elsewhere.

## The window, as opposed to the page

Everything above drives a headless Chrome, which is the right tool for what the
_page_ does and blind to everything ticket 27 was about: a browser tab has no
frame to remove and no caption buttons to get wrong. The four PowerShell scripts
act on laplus's real window through Win32.

| File               | What it does                                                                                  |
| ------------------ | --------------------------------------------------------------------------------------------- |
| `window-find.ps1`  | Finds the window. Dot-sourced by the other three; **read its header before writing a fourth** |
| `window-shot.ps1`  | Photographs the window, frame and all, to a PNG                                               |
| `window-drag.ps1`  | Drags the window by a point on the topbar and reports how far it moved                        |
| `window-click.ps1` | Presses a point in from the right-hand edge and reports what the window did                   |

```
powershell -File tools/ui-driver/window-shot.ps1 -Out ../.scratch/window.png
powershell -File tools/ui-driver/window-drag.ps1 -X 600 -Y 26
powershell -File tools/ui-driver/window-click.ps1 -FromRight 69 -Y 20   # maximise
```

`-FromRight` is in from the right edge because that is where the controls are:
23 is close, 69 maximise, 115 minimise, and the two panel toggles are further
left again. All of them read the answer back from Windows (`IsZoomed`,
`IsIconic`, `DwmGetWindowAttribute`) rather than from the page, so the thing
being tested is not also the thing reporting.

**`Get-Process().MainWindowHandle` is the trap here**, and it produced a run of
green results against a window sixteen pixels wide. `window-find.ps1` has the
whole story; the short version is that .NET's idea of a main window is a guess,
and once laplus is minimised the guess lands on tao's helper window.

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
