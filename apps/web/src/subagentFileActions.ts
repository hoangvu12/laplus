/**
 * What a file or diff action inside a child's work stream does.
 *
 * **Neighbouring surfaces, not a replacement.** A child tab is an ordinary
 * right-panel tab, and so are the file and diff tabs these open: the workspace
 * gains a surface and activates it, and the child's own tab is still there to
 * come back to. Nothing here closes anything, which is the whole property the
 * spec asks for — "child file and diff actions open neighboring right-panel
 * tabs, so the child tab remains available while I inspect an artifact".
 *
 * Named functions rather than two calls inlined in the panel, for
 * `diffFileActions.ts`'s reason: "what does clicking a file in a child's work
 * do" is a question that deserves a name and a test, and the answer must not
 * differ between the two places a child exposes a path.
 */
import type { ScopedThreadRef } from "@t3tools/contracts";

import { workspaceRelativePath } from "./markdown-links";
import { useRightPanelStore } from "./rightPanelStore";

/** Absolute in either family: `/src/main.rs`, `C:\src\main.rs`, `C:/src`. */
function isAbsolute(path: string): boolean {
  return /^([A-Za-z]:)?[\\/]/.test(path);
}

/**
 * The file surface's address for a path a child reported, or `null` when there
 * is nowhere to open it.
 *
 * **A provider reports the path it used, and that is usually absolute.**
 * OpenCode's `read` and `edit` tools carry `input.filePath` as the model wrote
 * it; the right panel's file surface is addressed by a *workspace-relative*
 * path (`useRightPanelStore.openFile`, and `ChatMarkdown` converts for exactly
 * this reason). Handing the raw path straight through opens a tab on a file the
 * panel cannot resolve — a surface that appears and is empty, which is worse
 * than no affordance at all.
 *
 * So: an already-relative path is passed through, an absolute one inside the
 * workspace is made relative, and anything else — outside the workspace, or a
 * workspace whose root this client does not know — is `null`. A child may
 * legitimately read a file outside the workspace, and the honest answer for one
 * of those is that this window has no surface for it.
 */
export function subagentFileTarget(path: string, workspaceRoot: string | undefined): string | null {
  if (!isAbsolute(path)) return path;
  return workspaceRelativePath(path, workspaceRoot);
}

/** Open a file the child read or changed, beside the child's own tab. */
export function openSubagentFile(threadRef: ScopedThreadRef, relativePath: string): void {
  useRightPanelStore.getState().openFile(threadRef, relativePath);
}

/**
 * Open the workspace diff beside the child's own tab.
 *
 * The thread's existing diff selection is left alone, and this is a **known
 * limitation** rather than a preference. The main agent's equivalent is
 * `onOpenTurnDiff` (`ChatView.tsx`), which calls
 * `useDiffPanelStore.selectTurn(ref, turnId, filePath)` before opening, so the
 * developer lands on the file they clicked within the turn that changed it. A
 * child's edit entry carries the path but no revision the diff panel is
 * addressed by — a subagent work stream has no turn identity of its own — so
 * selecting a scope here would be laplus guessing which comparison the developer
 * meant. This brings the existing diff surface into view and no more. Giving a
 * child's edit real per-file navigation needs a decision about what a child's
 * edit is diffed *against*, which is a change to the shared model.
 */
export function openSubagentDiff(threadRef: ScopedThreadRef): void {
  useRightPanelStore.getState().open(threadRef, "diff");
}
