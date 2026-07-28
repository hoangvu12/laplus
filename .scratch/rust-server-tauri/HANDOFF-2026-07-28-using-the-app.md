# Handoff — four things found by using laplus, and what is left

**Date:** 2026-07-28
**Branch:** merged to `main` (`62c71b1c1`)
**Found by:** a session spent using laplus to work on laplus, rather than
reading it.

That provenance is the point. Every item below was invisible to the suite —
which was green throughout — and obvious within a minute of driving the
application. Three of the four had a written ticket saying they were fine.

---

## What shipped

| Commit      | What it does                                                                  |
| ----------- | ----------------------------------------------------------------------------- |
| `f10e062a7` | `AskUserQuestion` renders as a question instead of an Allow/Deny prompt       |
| `a89885662` | The `/` and `$` menus are filled, from the CLI's handshake and the filesystem |
| `1053a3862` | The agent's announcement no longer opens every conversation with a row        |
| `0ba121111` | Pressing send shows something immediately, on a repository of any size        |

Two of these are worth reading for the method rather than the change.

**The handshake.** The obvious source for the slash commands is the
`system/init` line, which lists them. It cannot be used: the CLI writes no
`init` until it has been given a prompt — verified against the real binary, with
stdin open and closed, rather than assumed. What answers immediately is the
`initialize` control request, and it answers _better_, with the descriptions and
argument hints the menu shows. `crate::catalogue` has the whole finding.

**The empty pane.** Reported as "the first message doesn't show anything". Two
hypotheses were chased and both were wrong, both client-side, and both would
have been believed on a one-second-per-sample probe. `tools/ui-driver/first-turn.mjs`
samples four times a second, and the answer was in the server: `running` was
published _after_ the pre-turn checkpoint, which is a `git add -A` over the whole
project. 0.2s on a scratch repo, 2.1s on this one.

---

## What is left

### 1. The titlebar — ticket 27, reopened

> **Done, later the same day.** The window is frameless, the UI draws its own
> controls, and the topbars drag. Ticket 27's last comment has what was built
> and what was found; `27-titlebar/` has the photograph. The two findings below
> both held, and the second one — `isDesktopShell` rather than `isElectron` — is
> what the fix is built on. What follows is the brief as it stood.

The one thing asked for first and finished last. The window still has the
operating system's frame above the application's own topbar, where upstream has
one bar with the window controls drawn into it.

Ticket 27 was `wontfix` and is now `needs-triage`, because the maintainer has
asked for it. **Read that ticket before starting** — its three-things-together
argument still holds and this session did not weaken it. What this session adds
is one finding that makes the second bullet worse, and one that makes it easier:

- **WebView2 exposes no Window Controls Overlay.** Upstream's Windows path is
  `titleBarStyle: "hidden"` plus `titleBarOverlay`, and the UI reserves space for
  the OS-drawn buttons through the `.wco` class and `env(titlebar-area-*)`
  (`apps/web/src/index.css`, `apps/web/src/lib/windowControlsOverlay.ts`). None
  of that can fire here, so the buttons have to be drawn by the web app itself —
  upstream's own titlebar path cannot simply be switched on.
- **`isElectron` is the wrong gate, and there is a right one.** Faking
  `window.desktopBridge` is what ticket 27 rejects, correctly: it also flips the
  router to hash history (`apps/web/src/main.tsx:25`) and turns on every
  Electron-only feature behind that gate. A separate `isDesktopShell` flag —
  keyed on Tauri's own injected global — costs one module and leaves
  `isElectron` alone. That is a much smaller diff than the ticket assumes,
  because `apps/web` is ours outright now (ADR-0014) and it was not when 27 was
  written.

Still true: the drag regions need `data-tauri-drag-region` and a capability
granting `core:window:allow-start-dragging` to the loopback origin, since the
page is served over `http://127.0.0.1:4773` rather than from a Tauri scheme.
`-webkit-app-region: drag` is inert in WebView2.

### 2. The draft pane's request storm — ticket 35

Unchanged in mechanism, changed in character, and there is a comment on it
saying so. It was filed as console noise on the grounds that nothing was
user-visible. It _was_ user-visible: the pane goes blank for one 250ms retry tick
on the first message of a conversation. `0ba121111` masks that — the session is
now `running` before the client's subscription lands, so whatever the pane draws
next already has something to draw — but the four-requests-a-second is still
there and the choice the ticket asks for is still unmade.

### 3. Ticket 37 — the mode the agent reports is no longer visible

The cost of `1053a3862`, filed against itself so it is a decision with a record
rather than a thing that quietly went missing.

### 4. Ticket 38 — a project's own slash commands are missing

`catalogue`'s known gap. Skills are scanned per project; commands are not,
because a command costs a process and a skill costs a `readdir`.

---

## Two things deliberately not done

**Streaming stays.** Upstream drives Claude through the Agent SDK without
partial messages, so replies land whole; laplus passes
`--include-partial-messages` and streams. This was raised twice as a difference
from upstream and kept both times. It is the only thing filling the first two
seconds of a turn, and matching upstream here would make the application worse
to use in exactly the window item 4 above was about.

**The composer draws no spinner.** There is a "Working for 3s" text row and no
animated affordance anywhere in it. That is upstream's design, unchanged here,
and is noted only so the next person to look does not file it as a laplus
regression.

---

## The tool this session leaves behind

`server/tools/ui-driver/first-turn.mjs` — types a prompt into a fresh
conversation and samples the pane four times a second. It is the answer to "the
first message doesn't show anything", which is otherwise a report nobody can act
on, and it is how the two-second figure above is a measurement rather than an
impression. `repro.mjs` beside it polls once a second and steps straight over
the thing being asked about.
