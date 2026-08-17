# 07 — Drive the complete cross-provider experience

**What to build:** Prove and finish the complete user-visible feature across Claude, Codex, and OpenCode by driving a running Laplus through live child inspection, neighboring workspace surfaces, safe tab lifecycle, reload, and replay; remove the throwaway mockup once the real interaction is established.

**Blocked by:** 02 — Show OpenCode's complete child work and blockers; 03 — Show Claude subagent work streams; 04 — Show Codex subagent work streams and hierarchy; 05 — Make subagent tabs durable workspace citizens; 06 — Treat the delegation tree as active work

**Status:** ready-for-agent

- [ ] One focused browser-driver scenario clicks a compact child row and observes a normal right-panel child tab displaying live prose and work through the shared main-agent UI.
- [ ] The scenario opens a child file or diff beside the child, switches among surfaces, and returns to the preserved child stream.
- [ ] The scenario closes and reopens a running child tab and proves the child continued working while hidden.
- [ ] The scenario scrolls away from live output, observes jump-to-latest behavior, and proves independent position survives tab switching.
- [ ] The scenario reloads the application and proves tab order, active selection, lazy replay, live continuation, and terminal result restore correctly.
- [ ] The scenario confirms child details do not appear as duplicated parent transcript messages and that no child tab opens automatically.
- [ ] Provider integration evidence confirms Claude, Codex, and OpenCode each satisfy the shared release contract with honest provider-specific omissions.
- [ ] Focused contract, client, provider, and UI checks pass, and the user-visible flow is driven rather than inferred from a green suite.
- [ ] Development servers, provider doubles, and browser processes used for verification are stopped after the focused run.
- [ ] The throwaway subagent layout prototype and its generated route entry are removed from production code after the real UI is validated; the research/design record remains the decision source.
