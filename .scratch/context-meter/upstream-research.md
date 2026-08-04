# The context ring — what moves it, when, and what upstream does

Written 2026-08-04. Two sources: the **live database** at `~/.laplus/state.sqlite`
(read-only, 8,778 `context-window.updated` rows across 32 threads) and
`pingdotgg/t3code` at commit
[`c30a6d9b`](https://github.com/pingdotgg/t3code/tree/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52).

Written because the developer asked three things: does the ring update on
`/clear`, is it real-time, and what does upstream do. The short answers are
**yes it does**, **yes within a turn**, and **upstream moves it at the compact
boundary where laplus waits for the turn to end**.

This corrects `/tmp/laplus-handoff-ui-turn-and-compaction.md`'s problem 2, which
said `Folded::Compacted` leaves the ring "keeping whatever it last read until
some _later_ event moves it". A later event does move it — the question is how
much later.

## What actually fills the ring

`deriveLatestContextWindowSnapshot` (`apps/web/src/lib/contextWindow.ts`) walks
the thread's activities backwards and takes the **last**
`context-window.updated` whose `usedTokens` is a non-negative number. The ring is
`usedTokens / maxTokens`. `totalProcessedTokens` is a lifetime counter carried in
the same payload and is _not_ what the ring shows — so the "a lifetime counter
legitimately does not drop" explanation in the handoff is not the one.

laplus writes those rows from two places:

1. **Every assistant message's `usage`**, folded per event. This is the real-time
   half, and it is genuinely real-time — a sample from one live turn:

   ```
   14:30:10.813  used=25435      14:31:11.709  used=62764
   14:30:26.890  used=32309      14:31:26.231  used=67935
   14:30:38.264  used=57248      14:31:41.451  used=73939
   ```

2. **An out-of-band measurement**, where the server asks the CLI how full its
   window is (`session::measure`, ticket 76). Those rows carry
   `turnId: null, inputTokens: null` and are the ones that can _drop_. Asked at
   exactly two moments, both setting `driving.unmeasured`:
   `Folded::Initialized` (`turn.rs:692`) and the turn's ending (`turn.rs:928`).

Everything below follows from that list: **nothing asks at a compact boundary,
and `/clear` produces no assistant message to fold.**

## `/clear` — it works, with a 1.2-second lag

Every `/clear` in the database is followed by a drop. All eleven of them:

| when             | before  | after  | lag   |
| ---------------- | ------- | ------ | ----- |
| 2026-07-30 18:37 | 325,346 | 20,409 | +1.2s |
| 2026-07-30 19:27 | 323,882 | 20,404 | +1.3s |
| 2026-07-30 19:40 | 119,450 | 20,446 | +1.2s |
| 2026-07-30 20:38 | 341,433 | 20,414 | +1.3s |
| 2026-07-30 21:34 | 245,078 | 20,559 | +1.2s |
| 2026-07-30 22:13 | 187,717 | 20,420 | +1.1s |
| 2026-07-30 22:48 | 156,184 | 20,427 | +1.3s |
| 2026-07-30 23:32 | 212,185 | 20,424 | +1.2s |
| 2026-07-31 00:43 | 304,399 | 20,446 | +1.3s |
| 2026-07-31 01:05 | 109,860 | 20,446 | +1.2s |
| 2026-08-03 06:38 | 412,896 | 21,105 | +1.6s |
| 2026-08-04 14:30 | 342,540 | 20,757 | +1.3s |

(One `/clear`, 2026-08-01 11:19 in thread `bbcb6ca6`, has no row after it at all —
that thread has no further activity of any kind, so it is the conversation
ending rather than the meter failing.)

Two things make this look like nothing happened:

- **The `/clear` turn itself publishes no meter row.** It completes in ~60ms with
  `numTurns: 0` and no assistant `usage` to fold — the CLI handles the command
  locally. The drop arrives only when the turn-end measurement comes back, which
  is the 1.2s. On 2026-08-04 the developer sent their next message **114ms after**
  that row landed, so the cleared reading was on screen for a fraction of a
  second before the next turn pushed it back to 25,435.
- **It does not go to zero, and should not.** ~20.4k is the system prompt, the
  tools, `CLAUDE.md` and the skills a cleared session still carries. The ring
  reading 2% after `/clear` is correct.

## `/compact` — this is the one that is actually broken

| when     | what                                           | used                               |
| -------- | ---------------------------------------------- | ---------------------------------- |
| 11:04:47 | developer sends `/compact`                     | 372,914 (last reading, from 11:02) |
| 11:06:58 | `session.compacted` — "373,735 tokens → 8,584" | _ring still 372,914_               |
| 11:06:59 | first measurement after the boundary           | **34,259**                         |

**The ring sat at 372,914 for 132 seconds** while the CLI summarised the
conversation — `turn.completed` recorded 2m 11s and $5.35. Nothing during that
window said the meter was stale, and nothing said a compaction was under way.
That is the "did it actually compact?" experience, and it is a _cadence_ problem,
not a "the meter never moves" problem.

There is also a **number mismatch**. The developer is shown two figures for the
same moment:

- `session.compacted` says **8,584** — `compact_metadata.post_tokens`, the
  summary alone.
- The ring settles on **34,259** — the whole window, which includes the ~20-25k
  floor the `/clear` table above establishes independently.

Both are right about different things. Worth knowing before choosing a fix,
because the obvious fix makes it visible.

## What upstream does

**It writes the boundary straight into the meter.** `ClaudeAdapter.ts:2622-2634`:

```ts
case "compact_boundary":
  yield* emitThreadTokenUsage(
    context,
    compactBoundaryTokenUsageSnapshot(
      message as unknown as Record<string, unknown>,
      context.lastKnownContextWindow,
      context.lastKnownTotalProcessedTokens,
    ),
    { rawMethod: "claude/system/compact_boundary", rawPayload: message },
  );
  yield* offerRuntimeEvent({ ...base, type: "thread.state.changed",
    payload: { state: "compacted", detail: message } });
  return;
```

and `compactBoundaryTokenUsageSnapshot` (lines 485-508) takes `post_tokens` as
the active figure and `pre_tokens` as `lastUsedTokens`:

```ts
const postTokens = finiteNonNegativeInteger(compactMetadata.post_tokens);
if (postTokens === undefined || postTokens <= 0) return undefined;
const preTokens = finiteNonNegativeInteger(compactMetadata.pre_tokens);
return makeClaudeTokenUsageSnapshot({
  activeTokens: postTokens,
  ...(preTokens !== undefined ? { lastUsedTokens: preTokens } : {}),
  ...
});
```

So upstream's ring drops **at the boundary**, to `post_tokens` — 8,584 in this
case. No 132-second wait.

**It also says that compaction is happening.** `ClaudeAdapter.ts:2611-2620` maps
the CLI's `system` / `status` message:

```ts
case "status":
  yield* offerRuntimeEvent({ ...base, type: "session.state.changed",
    payload: {
      state: message.status === "compacting" ? "waiting" : "running",
      reason: `status:${message.status ?? "active"}`,
      detail: message,
    }});
  return;
```

`waiting` is a declared `RuntimeSessionState` in the contract
(`packages/contracts/src/providerRuntime.ts`) that **laplus's `SessionStatus`
does not carry**. laplus has no `Status` variant on `SystemEvent` at all
(`protocol.rs:437-459`) — a `system`/`status` line falls through to
`SystemEvent::Other` and is counted as an unknown event.

**Upstream's ordinary cadence is the same shape as laplus's.**
`emitThreadTokenUsage` (`ClaudeAdapter.ts:1735-1773`) fires per usage-bearing
message, so it is real-time within a turn too, and `queryCurrentContextUsage`
(line 1774) is the out-of-band ask. The difference laplus already documents at
`turn.rs:685-692` — laplus additionally asks at `init`, which upstream does not,
so laplus's _opening_ turn has a real window where upstream's has a bare count.
That part of laplus is ahead.

## What to change, and the choice inside it

The cadence fix is one line in spirit: **set `driving.unmeasured = true` in
`Folded::Compacted`** (`turn.rs`, around line 712), so the boundary asks the same
question the turn ending already asks. `session::run` takes that on the very next
event (`session.rs:722`), so the row lands in milliseconds rather than after the
rest of the compaction turn.

That is deliberately **not** upstream's approach, and the reason is the mismatch
above. Writing `post_tokens` into the meter literally, as upstream does, would
make the ring read 8,584 and then jump to 34,259 at the next measurement — two
wrong-looking movements instead of one right one. Asking the CLI gets the figure
the ring is actually of. The options are:

1. **Ask at the boundary** (recommended). One correct reading, ~1s after the
   boundary. `post_tokens` stays where it is, in the `session.compacted` row's
   sentence, where it is describing the summary rather than the window.
2. **Write `post_tokens`, upstream's way.** Instant, and wrong by the size of the
   system prompt until the next measurement corrects it.
3. **Both** — `post_tokens` immediately as a floor, corrected by the measurement.
   Most movement on screen for the least gain.

Separately, and worth its own decision: **nothing tells the developer a
compaction is running.** Upstream's `status: "compacting"` → `waiting` is the
mechanism, and adopting it needs either a new `SessionStatus` variant or an
activity row. The 132-second silence is what made the developer ask whether it
had compacted at all, so this is arguably the more valuable half.

## Primary-source index

- Compact boundary → token usage:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ClaudeAdapter.ts#L2622-L2643>
- `compactBoundaryTokenUsageSnapshot`:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ClaudeAdapter.ts#L485-L508>
- `system`/`status` → `waiting`:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ClaudeAdapter.ts#L2611-L2620>
- `emitThreadTokenUsage` and `queryCurrentContextUsage`:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/apps/server/src/provider/Layers/ClaudeAdapter.ts#L1735-L1796>
- `RuntimeSessionState`, including `waiting`:
  <https://github.com/pingdotgg/t3code/blob/c30a6d9b9943cfbf2fd47efc9de6eb9675457d52/packages/contracts/src/providerRuntime.ts>
- The queued-prompt research this sits beside:
  `.scratch/prompt-queueing/upstream-research.md`
