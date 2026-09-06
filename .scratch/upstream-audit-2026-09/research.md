# Upstream audit, September 2026

Status: ready-for-human
Date: 2026-09-06
Upstream read at: [`pingdotgg/t3code`](https://github.com/pingdotgg/t3code), pushed 2026-09-06

Supersedes nothing. The previous scan is
[`../upstream-feature-scout/research.md`](../upstream-feature-scout/research.md),
taken 2026-08-17 against upstream `9a1472d9558e`; this one covers what has
landed since and is scoped to the three questions it was asked, rather than to a
feature sweep.

**Upstream is public and readable with `gh`.** `gh api repos/pingdotgg/t3code`
answers, and `gh search commits --repo pingdotgg/t3code` indexes it. That is
worth writing down because `server/CLAUDE.md` says there is no upstream remote
and no more syncs, which is a policy about merging rather than a claim that the
source cannot be read. It can, and it is MIT.

Rate of change, measured rather than supposed: two `per_page=100` pages of
commits covered 2026-09-05 to 2026-09-06 alone. A feature-by-feature sweep is
not a repeatable exercise at that rate; a question-by-question one is.

## What the audit was asked

1. How a phone attaches a file.
2. Whether upstream has fixed provider process accumulation in a way laplus has not.
3. What else is worth taking.

## 1. Attachments

Upstream's web composer carries a hidden `<input type="file" multiple>` and a
paperclip button in the **right-hand actions group beside send**
(`apps/web/src/components/chat/ChatComposer.tsx`, at the `showComposerAttachAction`
branch), with `onPointerDown` prevented so the picker does not take the
composer's focus. That is the whole of the affordance a phone needs, and it is
what laplus was missing: paste and drop were the only two ways in, and a phone
has neither.

Ported, narrowly. Upstream has since gone much further and laplus deliberately
has not followed:

| Upstream                                                                                                                                                                                                           | Laplus                                                                                                         |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------- |
| [`#8048`](https://github.com/pingdotgg/t3code/pull/8048) uploads over signed HTTP URLs before send, with a queue, progress and retry                                                                               | inline data URLs on the turn-start message, per [`../image-attachments/spec.md`](../image-attachments/spec.md) |
| [`#8236`](https://github.com/pingdotgg/t3code/pull/8236) attaches PDFs, ZIPs and other files                                                                                                                       | images only                                                                                                    |
| [`#8161`](https://github.com/pingdotgg/t3code/pull/8161) converts HEIC photos to JPEG                                                                                                                              | PNG, JPEG, GIF, WebP                                                                                           |
| [`#10198`](https://github.com/pingdotgg/t3code/pull/10198)–[`#10200`](https://github.com/pingdotgg/t3code/pull/10200) report image dimensions with the asset URL so the chat slot is sized before the bytes arrive | the slot resizes when the image loads                                                                          |

`apps/mobile/src/components/ComposerAttachmentButton.tsx` is upstream's React
Native app and has no counterpart here. Laplus's mobile UI is the responsive web
composer, so upstream's **web** affordance is the one that transfers.

## 2. Provider process accumulation

Two upstream changes looked relevant. Neither turned out to describe a gap
laplus still has, and saying so is the finding:

- [`#7719`](https://github.com/pingdotgg/t3code/pull/7719) `fix(server): reconcile orphaned provider sessions` repairs projected
  sessions left `starting`/`running` with no live process, on restart, before
  the command gate opens. **Laplus cannot reach that state**: it never persists
  a live session at all. `Thread::restored`
  ([`fold.rs`](../../server/crates/laplus-server/src/threads/fold.rs)) returns
  `session: None` and moves a `running` turn to `interrupted`, and
  `a_turn_the_app_closed_during_does_not_come_back_running`
  ([`socket_continuity.rs`](../../server/crates/laplus-server/tests/socket_continuity.rs))
  has pinned that since before this audit. One neighbouring case _was_ open and
  is ticket [01](issues/01-a-queued-prompt-survives-the-app-being-killed.md).
- [`#8187`](https://github.com/pingdotgg/t3code/pull/8187) `perf(server): cut idle CPU use and stop provider event leaks` bounds
  the Codex and ACP protocol readers to a sliding 32, caps concurrent request
  handlers at 32, and stops termination blocking on handler cleanup. **Laplus's
  Codex transport is already bounded** — `OUTPUT_QUEUE` is 256 on a
  `tokio::sync::mpsc` channel, which applies backpressure rather than dropping —
  and it answers every inbound app-server request with `-32601`
  (`protocol::unsupported_request`), so it has no handler pool to cap. What is
  unbounded is one queue behind that channel: ticket
  [03](issues/03-bound-the-deferred-codex-line-queue.md).

The leak laplus actually had was diagnosed and fixed here rather than upstream —
see [`../provider-process-leaks/`](../provider-process-leaks/). Its one
remaining Windows gap is ticket
[02](issues/02-a-provider-process-is-supervised-from-creation.md), now closed.

## 3. Worth taking

Ranked by what it is worth to laplus, which is not upstream's ranking: laplus has
no Electron desktop app, no mobile app, no relay and no marketing site, so whole
categories of upstream's traffic do not apply.

| Upstream                                                                                                           | What it is                                                         | Note                                                                             |
| ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| [`#8605`](https://github.com/pingdotgg/t3code/pull/8605)                                                           | `fix(codex): avoid quadratic app-server input buffering`           | Same protocol laplus decodes. Check `codex_protocol.rs` against it.              |
| [`#8480`](https://github.com/pingdotgg/t3code/pull/8480)                                                           | `fix(opencode): handle child approvals, stops, and model catalogs` | Overlaps [`../opencode-correctness/`](../opencode-correctness/).                 |
| [`#8808`](https://github.com/pingdotgg/t3code/pull/8808), [`#9293`](https://github.com/pingdotgg/t3code/pull/9293) | context compaction across harnesses                                | A feature laplus does not have.                                                  |
| [`#9173`](https://github.com/pingdotgg/t3code/pull/9173)                                                           | recall sent prompts with the up arrow                              | Composer-local; no server work.                                                  |
| [`#9924`](https://github.com/pingdotgg/t3code/pull/9924)                                                           | `fix(opencode): revert from the first removed assistant message`   | Revert semantics laplus also implements.                                         |
| [`#8897`](https://github.com/pingdotgg/t3code/pull/8897)                                                           | `fix(codex): accept rate limit errors on thread resume`            | Resume path laplus shares.                                                       |
| [`#9167`](https://github.com/pingdotgg/t3code/pull/9167)                                                           | continue active threads across server restarts                     | The constructive counterpart to `#7719`. Needs a product decision before design. |

Not taken, and why: everything under `feat(mobile)`, `feat(desktop)` and
`feat(marketing)`; the Knip export-pruning campaign, which is upstream's
TypeScript hygiene and does not describe this tree; and the Antigravity, Cursor
and Grok drivers, which remain inherited contract vocabulary here rather than
runtime drivers.

## How to re-run this

```
gh api "repos/pingdotgg/t3code/commits?per_page=100&since=<ISO date>" \
  --jq '.[] | "\(.commit.author.date[0:10]) \(.sha[0:8]) \(.commit.message | split("\n")[0])"'
gh search commits --repo pingdotgg/t3code '<term>' --author-date '><ISO date>'
gh api repos/pingdotgg/t3code/pulls/<n> --jq '.body'
```

Read a file without cloning:

```
gh api repos/pingdotgg/t3code/contents/<path> --jq '.content' | base64 -d
```
