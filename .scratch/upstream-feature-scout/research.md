# Upstream feature scout — 2026-08-10

## Scope and provenance

Laplus identifies its UI origin as [`pingdotgg/t3code`](https://github.com/pingdotgg/t3code),
while its only configured Git remote is `hoangvu12/laplus`. This audit therefore
read upstream directly rather than assuming a merge relationship. Findings are
pinned to upstream commit
[`9a1472d9558e`](https://github.com/pingdotgg/t3code/tree/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6)
(2026-08-09/10 history) and Laplus commit
[`fbf296f`](https://github.com/hoangvu12/laplus/tree/fbf296f).

This is a feature-candidate audit, not a claim that Laplus should track upstream
wholesale. I compared upstream feature commits and their changed source with the
corresponding Laplus contracts, client state, UI, and Rust persistence. An
upstream feature is called missing only when its identifying contract/state/UI
seam is absent locally, not merely because filenames differ.

## Recommended order

### 1. Add pinned threads and user-controlled pinned order

**Why first:** this is a compact, durable attention-management feature that fits
Laplus's existing working/snoozed/settled sidebar model. Upstream added a pinned
shelf and persisted ordering, then added drag-to-reorder and one-click unpin;
the implementation deliberately keeps a stable `pinOrderKey` through projection
and sorting. See upstream
[`#5312`](https://github.com/pingdotgg/t3code/pull/5312),
[`#5581`](https://github.com/pingdotgg/t3code/pull/5581), and
[`#5578`](https://github.com/pingdotgg/t3code/pull/5578), plus the pinned-order
[contract and client sort implementation](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/packages/client-runtime/src/state/threadSort.ts).

Laplus already has the natural UI home — active, snoozed, and settled shelves in
[`SidebarV2.tsx`](https://github.com/hoangvu12/laplus/blob/fbf296f/apps/web/src/components/SidebarV2.tsx) —
but has no pin/unpin command, `pinOrderKey`, or pinned shelf in its contracts,
client runtime, Rust thread fold, or sidebar. This is a medium-sized vertical
slice: schema/command, SQLite field, fold/projection, optimistic client state,
sorting, and drag UI.

### 2. Surface unsent new-thread drafts in the sidebar

**Why second:** most of the risky local-draft machinery already exists in
Laplus; the missing value is discoverability. Upstream's draft shelf keeps
unsent project drafts one click away, supports returning to them, and cleans up
the placeholder when the draft becomes a real thread. Its change is isolated to
the sidebar, draft store, and new-thread handler; see
[`#5777`](https://github.com/pingdotgg/t3code/pull/5777) and the upstream
[`composerDraftStore.ts`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/web/src/composerDraftStore.ts).

Laplus already persists scoped drafts and attachments in its much richer
[`composerDraftStore.ts`](https://github.com/hoangvu12/laplus/blob/fbf296f/apps/web/src/composerDraftStore.ts),
but neither local sidebar implementation renders a draft shelf. That makes this
primarily a UI/routing integration rather than a new server feature, and likely
the best benefit-to-effort candidate.

### 3. Add thread actions to the chat-header title

**Why third:** upstream makes the title itself the consistent entry point for
rename, pin, archive/delete, copy, and related thread actions, rather than
requiring a trip back to a sidebar row. It centralizes action-menu policy in a
shared hook so the header and sidebar do not drift; see
[`#5592`](https://github.com/pingdotgg/t3code/pull/5592),
[`useThreadActionMenu.ts`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/web/src/hooks/useThreadActionMenu.ts),
and [`ChatHeader.tsx`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/web/src/components/chat/ChatHeader.tsx).

Laplus has substantial action behavior in
[`useThreadActions.ts`](https://github.com/hoangvu12/laplus/blob/fbf296f/apps/web/src/hooks/useThreadActions.ts)
and sidebar menus, but no shared `useThreadActionMenu` seam and no title-triggered
menu. Implement this alongside pinning so both entry points share one policy.

### 4. Paginate old turns with user-anchored history windows

**Why fourth:** this is the strongest scalability feature, but it crosses every
layer. Upstream stopped sending complete histories by introducing keyed turn
windows, older/newer cursors, stable anchoring while pages arrive, and dedicated
cache/runtime tests. See
[`#5493`](https://github.com/pingdotgg/t3code/pull/5493), the
[`threadDetailCursor`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/server/src/orchestration/threadDetailCursor.ts)
wire helper, and upstream's
[`threads.ts`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/packages/client-runtime/src/state/threads.ts)
pagination state.

Laplus's thread client has a resume cursor for event catch-up, but no turn-window
cursor or load-older state in
[`packages/client-runtime/src/state/threads.ts`](https://github.com/hoangvu12/laplus/blob/fbf296f/packages/client-runtime/src/state/threads.ts),
and the Rust server has no thread-detail cursor contract. Treat this as a
separate performance project, justified by measured large-thread payload or
render cost rather than visual parity alone.

### 5. Offer a small appearance upgrade before copying the full theme editor

Upstream now offers configurable UI/mono font families and sizes
([`#5103`](https://github.com/pingdotgg/t3code/pull/5103)) and a large modular
theme editor/library with import/export
([`#5226`](https://github.com/pingdotgg/t3code/pull/5226)). The latter adds many
theme-specific components, including
[`ThemeEditorPanel.tsx`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/web/src/components/settings/ThemeEditorPanel.tsx).

Laplus currently exposes only system/light/dark in
[`SettingsPanels.tsx`](https://github.com/hoangvu12/laplus/blob/fbf296f/apps/web/src/components/settings/SettingsPanels.tsx).
Font family/size controls are a contained first increment; importing the whole
theme editor is high surface area and should follow only if customization is a
product goal.

### 6. Add an explicit project-icon picker only if `t3.json` is too technical

Upstream lets users select an image from project files and persists the selected
favicon through project projection; it also sandboxes user-provided SVGs. See
[`#5775`](https://github.com/pingdotgg/t3code/pull/5775), the
[`ProjectFaviconPickerDialog`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/web/src/components/settings/ProjectFaviconPickerDialog.tsx),
and the follow-up SVG hardening in
[`#5916`](https://github.com/pingdotgg/t3code/pull/5916).

Laplus already resolves an explicit `t3.json` `iconPath` and safe conventional
fallbacks in
[`project_favicon.rs`](https://github.com/hoangvu12/laplus/blob/fbf296f/server/crates/laplus-server/src/project_favicon.rs),
but has no picker or project-settings panel. A picker improves usability without
adding icon capability; if implemented, the upstream SVG hardening is part of
the feature, not an optional follow-up.

## Additional candidates after the core list

- **A dedicated agents/workflows panel.** Upstream's native observability change
  adds an `AgentsPanel`, provider/runtime schemas, background liveness, and
  workflow-script queries, rather than only decorating tool rows; see
  [`#5219`](https://github.com/pingdotgg/t3code/pull/5219) and upstream
  [`AgentsPanel.tsx`](https://github.com/pingdotgg/t3code/blob/9a1472d9558ec74b5ed419bd7b87b2aa0e6be1e6/apps/web/src/components/AgentsPanel.tsx).
  Laplus already owns the hard first half — provider-specific subagent event
  folds — but has no agents panel. Promote this above appearance work if users
  routinely run multiple parallel agents; otherwise the existing work rows are
  an adequate smaller surface.
- **Recent sites in the Browser panel.** Upstream persists and suggests recently
  used preview sites ([`#5270`](https://github.com/pingdotgg/t3code/pull/5270)).
  Laplus has browser preview surfaces but no `browserHistoryStore` or recent-site
  picker under [`apps/web/src`](https://github.com/hoangvu12/laplus/tree/fbf296f/apps/web/src).
  This is a bounded local-persistence/UI enhancement and a sensible small win
  for users who repeatedly open the same dev-server URLs.
- **Make Sidebar V2 the default, then remove the compatibility branch.** Upstream
  completed that migration in [`#5672`](https://github.com/pingdotgg/t3code/pull/5672).
  Laplus still selects `Sidebar` versus `SidebarV2` from `sidebarV2Enabled` in
  [`AppSidebarLayout.tsx`](https://github.com/hoangvu12/laplus/blob/fbf296f/apps/web/src/components/AppSidebarLayout.tsx).
  This is low implementation effort but high regression surface; drive the
  window before deleting the fallback.

## Features not worth treating as current gaps

- **Usage dashboard:** upstream's cross-platform dashboard landed in
  [`#5743`](https://github.com/pingdotgg/t3code/pull/5743), but Laplus already
  implemented and verified the corresponding report in its
  [`usage-report` work](https://github.com/hoangvu12/laplus/tree/fbf296f/.scratch/usage-report).
- **Choose worktree or current checkout:** upstream added the per-project choice
  in [`#5766`](https://github.com/pingdotgg/t3code/pull/5766); Laplus already
  carries `defaultThreadEnvMode` through its
  [`settings contract`](https://github.com/hoangvu12/laplus/blob/fbf296f/packages/contracts/src/settings.ts)
  and new-thread flow.
- **Basic subagent visibility:** upstream's broad native agent/workflow panel is
  documented by [`#5219`](https://github.com/pingdotgg/t3code/pull/5219), with a
  later running-count badge in
  [`#5745`](https://github.com/pingdotgg/t3code/pull/5745). Laplus already folds
  Claude, Codex, and OpenCode subagent activity into first-class work rows in
  [`turn.rs`](https://github.com/hoangvu12/laplus/blob/fbf296f/server/crates/laplus-server/src/turn.rs)
  and provider-specific protocol code. A dedicated agents panel could still be
  useful, but it is a visualization expansion rather than missing observability.
- **Mobile, hosted identity, per-device provider settings, and T3 Connect:**
  upstream's per-device UI in
  [`#4479`](https://github.com/pingdotgg/t3code/pull/4479) depends on its
  multi-environment/mobile product. Laplus is a Rust server plus Tauri shell, so
  those changes are architecture expansion, not upstream parity work.

## Suggested delivery slices

1. Draft shelf (UI-only, validates demand for more sidebar organization).
2. Pin/unpin plus pinned shelf; then persisted drag ordering.
3. Shared thread-action menu and chat-title entry point.
4. Font family/size settings.
5. Benchmark large histories; build turn-window pagination only if the numbers
   justify the protocol and persistence work.
6. Project-icon picker and SVG sandboxing as one security-complete slice.
