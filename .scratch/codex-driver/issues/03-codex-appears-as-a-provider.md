# 03 — Codex appears as a provider: version, account, models, skills

**What to build:** A developer who has `codex` installed and authenticated opens
the model picker and sees Codex there — with the models their account can
actually use, each carrying the reasoning efforts that model supports, and with
the OpenAI account it will spend the quota of. A developer who has _not_ run
`codex login` is told that specifically, rather than being left to debug an
install that is fine.

A provider refresh starts one `codex app-server`, asks it four things, and kills
it: the version from the handshake, the account, the model list paged to
exhaustion, and the skills for the workspace. This is the shape the catalogue
already uses for `claude` — a session opened for one question — and for Codex one
process answers both the provider snapshot and the composer's `$` menu.

**Models come from the agent, never from a compiled table.** OpenAI's slugs churn
faster than laplus ships, and the proof is already in the tree: the contract's
alias table points at a model the capture's account cannot use, while the live
list is correct by construction. Reasoning efforts are carried **per model** —
the capture shows six for two models, five for one, four for the rest — so they
are not a property of the provider.

**This ticket brings the transport with it**, and three shapes of it that no
schema tells you, all confirmed by capture:

- Responses carry **no `jsonrpc` member**. `{"id":1,"result":{…}}` is the whole
  envelope, and a decoder requiring the member fails on every message.
- **Responses arrive out of order.** Requests are correlated by id through a
  pending map, never assumed FIFO.
- **The server's own requests use a separate id space beginning at 0.** One map
  keyed by id across both directions collides.

The version is read from the `initialize` response's user agent, which begins
with _our own_ client name — upstream's "text after the first `/`" parse works
only by the accident that a client name contains no slash.

**It also brings the noise.** Codex emits configuration warnings, remote-control
status and per-thread MCP startup notices before anything has been asked of it,
and writes `ERROR`-level lines to stderr that it then shrugs off — a missing
optional sandbox dependency is one. None of these make a provider broken. stderr
is classified rather than trusted; a driver that reads `ERROR` as fatal calls a
working Codex broken.

`capabilities: {}` on `initialize` — deliberately, and ticket 04 is where that
matters. The captures were recorded under it.

**Blocked by:** 02.

**Status:** ready-for-agent

- [ ] Codex appears as a provider instance with a version read from the
      handshake's user agent, parsed without assuming the client name is
      slash-free.
- [ ] The model list is fetched from the agent and paged to exhaustion, and each
      model carries its own supported reasoning efforts.
- [ ] A logged-in account is reported with what it is; a logged-out one is
      reported as **not logged in**, distinct from broken. The model list is
      still offered in that state, because the agent still answers it.
- [ ] Skills for the workspace populate the composer's `$` menu.
- [ ] The app-server started for the probe is killed when it has answered.
- [ ] A configured binary path is used; a configured `CODEX_HOME` is honoured.
- [ ] `configWarning`, `remoteControl/status/changed` and
      `mcpServer/startupStatus/updated` arriving before anything is asked do not
      make the provider broken.
- [ ] `ERROR`-level stderr from a healthy codex does not make the provider
      broken.
- [ ] Responses are correlated by id, tolerate out-of-order arrival, and decode
      without a `jsonrpc` member. The server's own request ids do not collide
      with ours.
- [ ] An ADR records **one app-server per conversation** and why, because the
      protocol permitting the opposite makes the choice surprising to a reader
      who knows it.
- [ ] `server/CONTEXT.md` gains an **app-server** entry.
- [ ] The suite runs offline on a machine that has never had `codex` installed.
