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

import { useRightPanelStore } from "./rightPanelStore";

/** Open a file the child read or changed, beside the child's own tab. */
export function openSubagentFile(threadRef: ScopedThreadRef, relativePath: string): void {
  useRightPanelStore.getState().openFile(threadRef, relativePath);
}

/**
 * Open the workspace diff beside the child's own tab.
 *
 * The thread's existing diff selection is left alone. A child's edit entry
 * carries the file it changed and not a revision the diff panel is addressed by,
 * so choosing a scope here would be laplus deciding which comparison the
 * developer meant — the diff surface already knows, and this only brings it into
 * view.
 */
export function openSubagentDiff(threadRef: ScopedThreadRef): void {
  useRightPanelStore.getState().open(threadRef, "diff");
}
