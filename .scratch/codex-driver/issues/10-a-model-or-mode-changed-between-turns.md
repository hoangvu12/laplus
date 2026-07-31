# 10 — A model or access mode changed between turns applies to the next turn

**What to build:** The picker tells the truth on Codex. A developer who changes
the model or the access mode between turns gets that change on the next turn, and
a turn already in flight keeps the rules it started under.

This is **retune** — telling an agent already serving a conversation to change
what it _is_ — and it is the third of the things this server says to a running
agent, distinct from an interrupt (which ends a turn) and a permission decision
(which answers a question). Neither of those changes what the agent is.

The pairing rule is the one the `claude` driver already follows and the loop from
ticket 01 already owns: a mode belongs to one turn, so it travels with the prompt
rather than as a signal. Two turns queued behind a running one, with the picker
moved between them, must each be answered under the mode they were requested
under.

Ticket 07's table is what a runtime mode becomes. The reviewer is sent explicitly
here too, for the same reason it is sent on resume: omitting it leaves whatever
the thread last used.

**Blocked by:** 07.

**Status:** done

- [x] Changing the model between turns applies to the next Codex turn, without
      replacing the session or losing the conversation.
- [x] Changing the access mode between turns applies to the next Codex turn, as
      the approval policy and sandbox ticket 07's table names.
- [x] A turn already running when the picker moves finishes under the mode and
      model it started with.
- [x] Two turns queued behind a running one, with the picker moved between them,
      are each served under the mode they were requested under.
- [x] Every session event published for one turn reports the same mode; the
      thread and `thread.turn-start-requested` event report that turn's model,
      which is where the TypeScript contract represents model selection.
- [x] A retune that does not land is reported to the developer rather than
      silently dropped.

**Where it landed.** `crate::threads::Prompt` and the generic session loop already
owned the pairing rule: model and mode travel with the queued prompt and are spent
only when that prompt reaches the front. `crate::codex::Codex` now keeps the
model/access configuration its thread opened with and, after either value moves,
sends the complete sticky override on `turn/start`: model, approval policy,
`sandboxPolicy`, and `approvalsReviewer: "user"`. The app-server process and Codex
thread id are unchanged, so its history stays in place. The correlated
`turn/start` response clears the loop's pending retune bookkeeping; a refusal
settles the requested turn as an error carrying Codex's reason.

The socket suite drives model-only and mode-only changes, three prompts queued
under three different pairs while the first turn is paused, consistent session
events grouped by active turn id, one app-server/thread across every change, and a
rejected retuned `turn/start`. It also closes the app-server's input after one
turn and proves an actual retuned `turn/start` write failure is reported and
settled as an error. `cargo test --no-fail-fast -p laplus-server --test
socket_codex_turn` passes 26 tests, and `cargo check -p
laplus-server --tests`
passes. Formatting was checked manually and with `git diff --check`; this machine's
stable Rust toolchain does not have the `rustfmt` component installed.
