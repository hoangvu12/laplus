# 05 — Make subagent tabs durable workspace citizens

**What to build:** Make child surfaces behave exactly like established right-panel workspace tabs while retaining child-specific replay and scroll state: several children can coexist with every existing surface, hide safely, restore after reload, and remain usable during live output.

**Blocked by:** 01 — Open an OpenCode child work stream

**Status:** ready-for-agent

- [ ] Several child tabs can remain open together and coexist with files, diffs, terminals, previews, and plans.
- [ ] Opening an existing child activates its tab; opening another child adds one tab using the workspace's existing ordering and activation rules.
- [ ] Child tabs use existing label, icon, close, context-menu, resizing, and narrow-layout conventions without bespoke status decoration.
- [ ] Closing a child tab removes only the surface and emits no interrupt, cancellation, detachment, or provider command.
- [ ] Starting, updating, blocking, completing, or failing a child never opens or focuses the right panel automatically.
- [ ] Open child tabs, their order, and the active selection restore with the parent thread after reload or restart, while full streams remain lazily loaded.
- [ ] A restored child reference that cannot be resolved keeps an explicit unavailable surface instead of disappearing silently.
- [ ] Every child tab preserves independent scroll position while the user switches among workspace tabs.
- [ ] A live child follows new entries only while pinned to the bottom; manual scroll suspends following and exposes the existing jump-to-latest behavior.
- [ ] Focused right-panel state and rendering tests prove these behaviors without depending on private component structure.
