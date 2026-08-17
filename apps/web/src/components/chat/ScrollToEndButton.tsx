/**
 * The way back to the live edge of a transcript that has run on without you.
 *
 * One component rather than two, because the main conversation and a subagent
 * work stream must offer the *same* affordance: the spec asks a child surface
 * to expose "the existing jump-to-latest behavior", and two copies of a pill
 * are only the same affordance until someone edits one of them.
 *
 * Positioning belongs to the caller — the conversation floats this above a
 * composer whose height it measures, and a subagent surface has no composer to
 * clear — so this owns the control and not where it sits.
 */
import { ChevronDownIcon } from "lucide-react";

export const SCROLL_TO_END_LABEL = "Scroll to end";

export function ScrollToEndButton(props: { onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={SCROLL_TO_END_LABEL}
      title={SCROLL_TO_END_LABEL}
      onClick={props.onClick}
      className="pointer-events-auto flex items-center gap-1.5 rounded-full border border-border/60 bg-card px-3 py-1 text-muted-foreground text-xs shadow-sm transition-colors hover:cursor-pointer hover:border-border hover:text-foreground"
    >
      <ChevronDownIcon className="size-3.5" />
      {SCROLL_TO_END_LABEL}
    </button>
  );
}
