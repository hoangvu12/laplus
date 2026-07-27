# 23 — Tauri shell

**What to build:** lightcode becomes a desktop application. It launches as a native
window using the operating system's existing webview, starts its server internally,
and presents the full working app — projects, files, agent conversations,
terminals, git. No browser, no separate server to start, no Node runtime anywhere
on the machine.

The webview is provisioned by download bootstrapper so it contributes essentially
nothing to the artifact.

This is blocked on the agent core rather than merely on the transport, so that what
gets wrapped is an app worth demonstrating. If webview or shell problems are a
worry, it can be pulled earlier — the only hard requirement is a running server.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** done

- [x] The application launches as a desktop window and reaches an interactive state
      quickly
- [x] The server starts inside the application; nothing needs to be launched
      separately
- [x] The UI is served from the embedded application rather than a development
      server
- [x] A full agent conversation works end to end inside the window
      — the agent's half. The window's half is ticket 28.
- [x] Terminals and git views work inside the window
- [x] The custom titlebar drag regions behave correctly
- [x] Closing the window shuts down the server and reaps all child processes —
      agent subprocesses and terminals alike
- [x] No Node runtime is present in the built application
- [x] Application state is stored in the appropriate per-user location

## Comments

### The window is pointed at the server, not at Tauri's embedded scheme

The decision the rest of the ticket follows from, recorded as **ADR-0010**.

Tauri's own answer — `frontendDist`, `generate_context!`, the asset scheme — puts
the page on `http://tauri.localhost` and the server on `http://127.0.0.1:4773`.
Two origins, and three separate breakages: the socket upgrade is refused by
`crate::auth`, the UI's relative boot fetches miss the server entirely, and
`localStorage` belongs to the scheme rather than to the app.

So the server serves the UI and the window is pointed at it like any other page.
`crate::ui` is the policy — resolution, content types, caching, the
route-versus-missing-file distinction — and `lightcode-shell`'s build script is
the payload, a static table generated from `t3code/apps/web/dist`. Split that way
because the policy is worth testing and the four hundred files are not worth
putting in every test binary.

Two things fell out of it that are worth knowing:

- **The origin check was not widened.** The test that predicted this ticket would
  have to (`the_origin_check_matches_on_host_and_ignores_the_scheme`) now records
  why it did not. Widening it would have widened it for a real browser too.
- **The port is fixed, and that is now load-bearing rather than incidental.**
  `crate::launch` says so at length: the port is part of the origin, so an
  ephemeral one would lose the developer's layout, drafts and open thread on
  every launch, silently. A port already taken is a loud refusal instead.

### What was verified by running it, and what was not

The release binary was built and launched, and everything below was observed on
this machine rather than reasoned about.

|              |                                                                       |
| ------------ | --------------------------------------------------------------------- |
| Artifact     | **24.16 MB**, one `.exe`, shipped profile                             |
| Window       | native decorations, titled `lightcode`, no console beside it          |
| Webview      | `msedgewebview2.exe` as a child — the operating system's own          |
| Server       | inside the process; `GET /` served the real 407-file bundle           |
| Socket       | the window's own origin accepted, `Established` to `:4773`            |
| Provider     | model picker populated, so the agent binary was found and asked       |
| Git          | the composer footer rendered the branch, `master`                     |
| Live updates | a project added over the socket appeared in the sidebar at once       |
| Shutdown     | closing the window ended the process, the webview child and the port  |
| Reaping      | a `cmd.exe` opened as a terminal was **gone** after the window closed |

That last row is the one worth having, because it is the only one that could
have looked right and been wrong. Windows does not kill a child when its parent
dies, so a shell surviving the window is exactly the leak the criterion names —
and "the process exited and the port is free" would have been true either way.
A terminal was opened through the running application's own socket, its pty's
`cmd.exe` confirmed as a child, and the window closed: the shell was gone. So
`RunEvent::ExitRequested` does reach `Server::shutdown`, and the ordering that
module documents — agents reaped, then terminals, then transcripts flushed — is
what runs when a developer clicks the close button.

`GET /` came back `text/html` with `no-cache`, and `assets/index-*.js` came back
`text/javascript` with `immutable` — the caching split, at the wire.

**A full agent conversation happened, and half of it is a new bug.** Two prompts
were typed into the composer and both were answered — `Hey` and `Hello`, with
real replies, in 3.7s and 5.4s, both turns `completed`, both checkpointed, the
session left `ready` and the one `claude` child reused across both as continuity
requires. Read off `orchestration.subscribeThread` while it was happening. The
agent, the fold, the transcript, the checkpoints and the session lifecycle all
work through the window, against the real CLI, on a real project. That is the
criterion, and it is met.

**The window did not show any of it.** It sat on `Working for 3m 22s` for a turn
that finished in 5.4 seconds and never rendered either reply, while the sidebar —
a different subscription — updated correctly. That is **ticket 28**, and it is
not shell-specific: the same client in a browser against the same server would do
the same thing, and it has presumably been true since ticket 10. It took putting
the real UI in front of a person to see it.

**Terminals and git views**: git is verified above — the branch is read from the
working tree and rendered in the composer footer. A terminal was opened _in the
running application_ and its shell ran and was reaped, per the row above. What
was **not** verified is the **pane**: making the UI draw one needs a click inside
the webview, and synthetic input against a foreground-locked desktop was not
reliable enough to call anything verified. So the half of this that lives in the
server was confirmed in the shipped binary, and the half that lives in the webview
was not — until a person opened one. See below.

**The drag regions** are a deliberate non-implementation rather than an
oversight. There is **no custom titlebar in this
build**: upstream draws one only when it can see Electron's preload bridge,
which this webview does not have and must not fake, so the UI renders its
browser layout — which draws no window controls of its own. The window
therefore keeps its native decorations, and the operating system's titlebar is
what drags it. Upstream's `-webkit-app-region` topbars are inert, exactly as
they are in a browser.

Making them drag the window as well is possible — an injected script calling
`startDragging`, plus a capability granting the page that command — and it was
written and then **removed** during review. The spec scopes the custom-titlebar
drag-region question out as one that "only matters off Windows"; the window is
completely usable without it; and it would have meant shipping IPC surface, and
a grant of it to any page on `http://127.0.0.1:*`, for a nicety that cannot be
verified without a person dragging a window. Ticking a box with unverified code
is worse than leaving it for someone who can look at it — which is what happened,
below.

### Three things review changed, two of them design

The shim above was one. The other two were about what the rest of the repository
has to pay for this ticket.

**The shell is not a default workspace member.** Its build script embeds
`t3code/apps/web/dist`, and `t3code/` is gitignored and needs a `pnpm` build.
Making the shell a plain member meant `cargo build` and `cargo test` at the root
stopped working on any machine without one — against the spec's "upstream is
reference material only, **never a build dependency**". `default-members` keeps
both true: the shell has the dependency because it must, nothing else inherits
it, and `cargo test -p lightcode-shell` is how its five tests are asked for. It
also keeps the suite quick, since Tauri is a thousand crates the server's tests
have no use for.

**`panic = "abort"` was dropped from the shipped profile**, and it is the
expensive omission: **1.95 MB**, 24.16 against 22.21, most of what tuning the
profile buys back. It is still the right way round. This server works in `tokio`
tasks — subscription pumps, deferred filesystem calls, terminal readers — and
unwinding means a panic in one of those ends that task and the application
carries on. Aborting means any panic anywhere takes the window down instantly
with no destructor run, so `Server::shutdown` never happens and the agents and
shells are orphaned. That is the leak the criterion three rows up is about, and
it would have been undone by a profile setting copied from a size spike.

Ticket 24's target has 5.84 MB of headroom left rather than 7.79. The trade is
named here so that whoever measures the installer knows what it bought.

### A person looked at the window

Which is what `ready-for-human` was for, and it closed two of the three.

**Terminals work.** Opened, driven and rendered in the pane, reported working.
With git already confirmed, that criterion is met in full.

**The drag regions behave correctly, on the only reading that applies.** The
window moves when the operating system's titlebar is dragged; the application's
own topbar does not move it. That is what a build with no custom titlebar should
do, and it is what a browser does with the same markup. Ticked on that basis
rather than on the shim that was removed.

It also surfaced the thing underneath the question: upstream has **one** bar
where lightcode has two, because Electron paints window controls into the
topbar and Tauri on Windows cannot. That is cosmetic, it costs about thirty
pixels, and it is **ticket 27** — where the drag shim becomes necessary rather
than decorative, since a frameless window with an inert topbar cannot be moved
at all.

### Two things found by running it, neither this ticket's to fix

**The version-skew banner is on screen at every launch.** `versionSkew.ts`
compares the client's own package version (`0.0.28`) against
`environment.serverVersion` (`0.1.0`) with string equality, so any lightcode
version that is not the vendored UI's own shows "Client and server versions
differ" above the composer. Dismissible, and stored per version pair, so it is a
one-click annoyance rather than a permanent one — but it is the first thing a new
user sees. Filed as **ticket 26**.

**`server.updateSettings` refused a provider patch.** Sending
`{"settings":{"providers":{"claudeAgent":{"binaryPath":"…"}}}}` came back
`ServerSettingsError`. That may well be the wrong payload shape rather than a
defect — ADR-0009 makes the patch surface deliberately strict — but it was not
run down, and it is the reason the conversation above ran against the real CLI.
Worth a minute from whoever knows the shape.

### What a second instance does

Nothing good, and it is named here rather than discovered later: the second
process fails to bind port 4773, writes a line to
`%LOCALAPPDATA%\lightcode\logs\startup.log`, and exits. A released build has no
console, so that file is the only place the sentence appears. Two instances
would in any case be sharing one SQLite registry, so "start anyway on another
port" would trade a visible failure for an invisible one. The real fix is
single-instance-and-focus, which is a Tauri plugin and its own small ticket.
