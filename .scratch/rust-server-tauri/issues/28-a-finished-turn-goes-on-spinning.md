# 28 — A finished turn goes on spinning in the window

**What to build:** a composer that stops saying "Working" when the work is done.

The agent answers, the server records the answer, and the thread view keeps
showing `Working for 3m 22s` with the reply never rendered. Observed at 3m 22s
against a turn the server finished in **5.4 seconds**.

**Status:** needs-triage

**Found by:** ticket 23, the first time a person sent a message through the real
UI. Nothing about it is shell-specific; the same server and the same client in a
browser would do the same thing, and it has presumably been true since ticket 10.

## What the server has

Everything, and it is all correct. Read straight off `orchestration.subscribeThread`
while the window was still spinning:

| | |
|---|---|
| user | `Hey` |
| assistant | `Hey! What can I help you with in lightcode today?` |
| user | `Hello` |
| assistant | `Hi again! I'm ready when you are — …` |

Both messages `streaming: false`. Both turns `state: "completed"` —
`turn-…-1` in 3.7s, `turn-…-10` in 5.4s, `stopReason: end_turn`, no parse errors,
no unknown events. `session.status: "ready"`, `session.activeTurnId: null`.
Two checkpoints written, both `status: "ready"`. The `claude` child was alive and
idle, having been reused across both turns as continuity requires.

So the agent, the fold, the transcript, the checkpoints and the session lifecycle
all worked. **What failed is the client's picture of them**, and this ticket is
about finding out why.

## What the window showed

- The user's `Hello` bubble, and nothing else — neither assistant reply, and not
  the earlier `Hey` exchange, though both were in the same thread.
- `Working for 3m 22s`, counting from a `requestedAt` the client clearly had.
- Breadcrumb `lightcode / New thread`, while the sidebar — fed by
  `orchestration.subscribeShell` — correctly showed the thread under its
  generated title.

That split is the shape of the bug: **the shell subscription updated and the
thread subscription did not.** The client is watching a thread id that is not
receiving, or is receiving and not folding.

## What was ruled out

**"A subscription opened on a thread that does not exist yet never wakes up."**
This was the first theory and it is **wrong**. It looked confirmed —
`orchestration.subscribeThread` on an unknown id opens with only
`{"kind":"synchronized"}` (correctly: an empty snapshot would be a positive claim
that the conversation is empty and would wipe what the composer is optimistically
showing), and creating that thread afterwards produced nothing.

The probe was at fault, not the server. `Ack` is real back-pressure — the server
sends **one** unacknowledged chunk and stops, which `crate::subscriptions` says in
its first rule. A probe that never acknowledges sees exactly one chunk and
concludes the feed is dead. Acknowledging it, the created event arrives
immediately. Anyone re-opening this should read that module before writing a
client.

## What is still on this machine

**The conversation above is still in the registry** — thread
`0ea19ef1-d1d2-4745-875b-a4c0ef996950`, project `lightcode`, in
`%LOCALAPPDATA%\lightcode\state.sqlite`. Both turns, both replies, both
checkpoints. So the first experiments cost **no agent turn at all**: the data
that should render is already there to be rendered.

**Start with the cheapest discriminator, which is free:** open that thread from
the sidebar in a fresh window and see whether it draws the four messages.

- If it **renders**, the bug is specific to the draft-becomes-real transition —
  the pane the composer was on never followed the thread the turn created, and
  the question is which id it is watching.
- If it **stays blank**, the thread subscription is broken generally, which is a
  much bigger fish and easier to chase.

This was attempted here and not answered: the sidebar's project was collapsed,
so the click landed on nothing. It is one click away for whoever picks this up.

**A socket probe needs two things**, both learned the hard way above. It must
send `{"_tag":"Ack","requestId":"<id>"}` after every chunk or the feed stalls
after one. And in .NET it must not cancel a pending `ReceiveAsync` to implement a
timeout — that aborts the whole WebSocket; poll the task instead.

## Where to look next

The client's own view, which nothing above has. Two ways in, and the second is
free because of how ticket 23 serves the UI:

- Enable Tauri's `devtools` feature on `lightcode-shell` and attach to the
  webview.
- **Open `http://127.0.0.1:4773/` in Edge or Chrome.** It is the same origin and
  the same application — that is the whole point of ADR-0010 — so the ordinary
  browser devtools work on it, with no rebuild.

The question to answer first: **which thread id did the client subscribe to, and
does it match the one the turn created?** The server will tell you, since
`Threads::entry` makes an entry for any id subscribed to; the client will tell you
faster over the network panel.
