# 04 — Follow-up upstream changes worth reading

Status: needs-triage

**What to build:** nothing yet. This is the shortlist from
[`../research.md`](../research.md), kept as a ticket so it is triaged rather than
re-derived.

Each row needs its own design pass before it becomes tickets. None of them is a
merge: upstream is TypeScript and this server is Rust, so what transfers is the
behaviour and the reasoning, not the diff.

| Upstream                                                                                                              | What it is                                                                                               | First question                                                                          |
| --------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| [`#8605`](https://github.com/pingdotgg/t3code/pull/8605)                                                              | `fix(codex): avoid quadratic app-server input buffering`                                                 | Does `codex_protocol.rs` have the same shape? Pairs with ticket 03.                     |
| [`#8480`](https://github.com/pingdotgg/t3code/pull/8480)                                                              | `fix(opencode): handle child approvals, stops, and model catalogs`                                       | How much is already covered by `../../opencode-correctness/`?                           |
| [`#8897`](https://github.com/pingdotgg/t3code/pull/8897)                                                              | `fix(codex): accept rate limit errors on thread resume`                                                  | Does laplus's resume treat a rate-limit error as a refusal?                             |
| [`#9924`](https://github.com/pingdotgg/t3code/pull/9924)                                                              | `fix(opencode): revert from the first removed assistant message`                                         | Compare against `crate::checkpoints`' revert.                                           |
| [`#8808`](https://github.com/pingdotgg/t3code/pull/8808), [`#9293`](https://github.com/pingdotgg/t3code/pull/9293)    | context compaction across harnesses                                                                      | Is compaction a product goal here? A decision, not a design.                            |
| [`#9173`](https://github.com/pingdotgg/t3code/pull/9173)                                                              | recall sent prompts with the up arrow                                                                    | Composer-local, no server work. Smallest thing on this list.                            |
| [`#10198`](https://github.com/pingdotgg/t3code/pull/10198)–[`#10200`](https://github.com/pingdotgg/t3code/pull/10200) | report image dimensions with signed asset URLs, so a chat image's frame is sized before its bytes arrive | Wants a contract field and a server-side decode. Pairs with `../../image-attachments/`. |
| [`#9167`](https://github.com/pingdotgg/t3code/pull/9167)                                                              | continue active threads across server restarts                                                           | The constructive counterpart to the repair in ticket 01. Product decision first.        |

Deliberately not listed: `feat(mobile)`, `feat(desktop)` and `feat(marketing)`
work, which has no counterpart in this tree; upstream's Knip export-pruning
campaign, which is TypeScript hygiene; and the Antigravity, Cursor and Grok
drivers, which stay inherited contract vocabulary here.
