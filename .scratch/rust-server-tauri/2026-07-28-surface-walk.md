# Surface walk — every control the application offers, and what answers

**Date:** 2026-07-28 · **Build:** `target/release/laplus.exe` (26.7 MB, 10:53)
**Method:** `tools/ui-driver/surface-walk.mjs` and `surface-actions.mjs`, against a
second instance on `:4774` with a copy of the real `state.sqlite`. No agent turn
was spent.

Written because every finding so far came from reading one server against the
other, and that method is blind to the question a user actually has: _of
everything on the screen, what does nothing?_ M1 — still the most severe item in
the ledger — was found by accident while looking for something else. This is the
same method, applied on purpose.

The signal is the socket: a control that reaches an unimplemented method
produces `ServerMethodNotImplementedError`, and one that reaches an unparsed
command produces `Command not implemented by this server: <name>`. Both are
visible in the frame log, so the walk does not have to guess from the DOM whether
a click did anything.

---

## S1 — Archive is a one-click failure on every row of the sidebar

Not buried in a menu. Every thread in the sidebar carries an `Archive <title>`
button; pressing one puts **"Failed to archive thread"** on screen.

```
--- pressed: Archive → Archive Okay, I'm using the laplus itself, here are few th..
  !! REFUSED: thread.archive
  !! ON SCREEN: Failed to archive thread
```

M4 was filed from the contract diff as "both commands refused". Seeing it happen
changes its priority rather than its description: this is the most reachable
broken control in the application, one press from the default screen.

`/settings/archived` is the other half, and it renders **"Could not load archived
threads / Failed to load archived threads."** — `orchestration.getArchivedShellSnapshot`,
refused.

## S2 — The refusal itself does not decode, and the decoder's complaint reaches the user

A new class, and the ugliest thing the walk found. On `/settings/diagnostics`
and `/settings/source-control` the page shows this, verbatim:

```
Expected { readonly "_tag": "EnvironmentAuthorizationError", ... },
got {"_tag":"ServerMethodNotImplementedError","message":"Method not implemented
by this server: server.getProcessDiagnostics","method":"server.getProcessDiagnostics"}
at ["cause"][0]
```

laplus answers with `ServerMethodNotImplementedError`, but that tag is **not a
member of the error union those methods declare**. So the client cannot decode
the error, and what the developer is shown is the schema decoder's complaint
about the shape of a refusal rather than any statement about the feature.

This is the same failure mode `config.rs:266` already reasons about for
`ConfigIssue` — _"its `kind` is one of two literals the contract names, not a
label: an invented kind fails the client's decode of the whole payload"_. The
same rule applies to error tags on a per-method basis, and `crate::rpc`'s single
`ServerMethodNotImplementedError` does not observe it.

Worth fixing independently of ever implementing the methods: answering with a tag
each method actually declares turns four raw decoder errors into four honest
empty states. Affects at minimum `server.getProcessDiagnostics`,
`server.getProcessResourceHistory`, `server.getTraceDiagnostics`,
`server.discoverSourceControl`.

## S3 — Ticket 35 is larger than filed, and on a different transport

Filed as a socket subscription retrying four times a second in a draft pane.
Measured on boot, on the default route, over HTTP:

```
16 × GET /api/orchestration/threads/350ca67b-…  → 404
```

in roughly five seconds. That is ticket 31's own HTTP snapshot path — the
optimisation added so the client would prefer a compressible unwrapped snapshot
over the socket — being asked for a thread the server has never heard of, and
404ing every time.

So the ticket's mechanism section is incomplete: the storm is on both transports,
and the HTTP half fires from the main screen rather than only from a draft pane.
It is also the entire content of the console's 404 noise, which appears on every
route and made the console useless as a signal until it was traced.

## S4 — Three provider rows spin for ever

`/settings/providers` lists **Codex, Claude, Grok, OpenCode**. Claude reports
`v2.1.220 / Available`. The other three sit permanently on:

> Checking provider status — Waiting for the server to report installation and
> authentication details.

They will never resolve, because laplus publishes exactly one provider instance
and the UI has a row per built-in driver. Upstream has a module for precisely
this case — `provider/unavailableProviderSnapshot.ts`, _"when
`providerInstances` references a driver this build does not ship … produces
shadow snapshots that satisfy `ServerProvider`'s wire shape while signalling
unavailability"_.

"Claude Code only in v1" is a settled decision (spec, Implementation Decisions).
Three rows implying the other drivers are _loading_ is not that decision showing
through — it is the absence of the snapshot that would state it. Small fix,
directly visible.

## S5 — Project actions can be imported and never saved (corrects R10)

**R10 in the ledger was wrong in its user-visible half and I am correcting it.**
It claimed `ProjectScriptsControl` is always empty because the server hardcodes
`"scripts": []`. The menu actually opens with:

```
Setup Worktree | Add action
```

`Setup Worktree` is this repository's own `t3.json` script. It appears because
**the client reads `t3.json` itself** — `useT3ProjectFileScripts.ts` calls
`projects.readFile`, which laplus implements — rather than taking the server's
`scripts` array. So the t3.json half works.

What does not work is keeping one. The hook's own docstring says these are
"offered in the scripts menu **for import**"; the saved set is
`OrchestrationProjectShell.scripts`, persisted through `project.meta.update`,
which carries `scripts: Schema.optional(Schema.Array(ProjectScript))` — and is
refused (M8). So the server's `[]` is not merely unpopulated, it is
**unpopulatable**, and importing an action is a dead end one step further in
than R10 said.

The rest of R10 stands: nothing runs `runOnWorktreeCreate`, which needs worktrees
(M12) anyway.

## S6 — Two smaller observations

- **`assets.createUrl` and `subscribeDiscoveredLocalServers` are called at boot**,
  not on demand. M7's "silently dropped attachments" understates it: the method is
  refused on the first screen, before anything is attached.
- **The UI still says "T3 Code"** — `/settings/general` reads "Choose how T3 Code
  looks across the app", and the document title is "Code". Recorded as an
  observation rather than a defect: the UI is upstream's, and whether laplus
  rebrands it is a product decision nobody has taken. `CONTEXT.md`'s rename entry
  covers `lightcode` → `laplus` only.

## What worked

Worth stating, because a walk that only lists breakage misleads. `/settings/general`,
`/settings/keybindings` (41 bindings, edit and when-clause controls per row),
`/settings/providers` for Claude itself, `/settings/beta`, the sidebar's project
and thread lists, the right-panel and terminal-drawer toggles, and the project
actions menu all render and respond. `/settings/connections` degrades honestly —
it says pairing needs a scope this backend does not offer, which is true.

## The tools this leaves behind

- `tools/ui-driver/surface-walk.mjs` — navigates every route, enumerates visible
  controls, and reports refusals, empty renders and console errors per route.
- `tools/ui-driver/surface-actions.mjs` — presses controls and reports what came
  back, plus every failed HTTP request the page made.

Both spend nothing. `repro.mjs` and `first-turn.mjs` remain the two that cost a
turn. Re-run either after implementing any M-item to check the control it was
about now answers.
