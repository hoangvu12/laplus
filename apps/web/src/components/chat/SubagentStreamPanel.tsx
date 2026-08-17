/**
 * One delegated child's work stream, as a right-panel surface.
 *
 * **Read-only, and no chrome of its own.** No composer, no identity or task
 * header, no per-child stop: the inline row in the conversation is where a
 * child's identity, assignment and state are reported, and this opens directly
 * into the work.
 *
 * **Nothing here draws a child's work itself.** Prose goes through
 * `ChatMarkdown` and the terminal outcome through `SimpleWorkEntryRow` — the
 * same two components the main agent's transcript uses — because the spec asks
 * for exactly that: "Reuse the main agent's transcript and work-entry
 * components in a read-only configuration." A bespoke renderer here would be a
 * second, worse work log to keep in step with the first, and ticket 02's
 * commands, reads, edits, diffs and tool calls would each have to be drawn
 * twice.
 *
 * The stream is fetched only while this is mounted. Closing the tab releases
 * the view and stops nothing: the server goes on recording the child, and
 * reopening the tab replays what it recorded meanwhile.
 */
import type {
  EnvironmentId,
  OrchestrationSubagentEntry,
  OrchestrationSubagentOutcomeKind,
  ScopedThreadRef,
  ThreadId,
} from "@t3tools/contracts";
import { Loader2 } from "lucide-react";

import type { WorkLogEntry } from "../../session-logic";
import { useEnvironmentQuery } from "../../state/query";
import { subagentEnvironment } from "../../state/subagents";
import { ScrollArea } from "~/components/ui/scroll-area";
import ChatMarkdown from "../ChatMarkdown";
import { SimpleWorkEntryRow } from "./MessagesTimeline";

interface SubagentStreamPanelProps {
  readonly environmentId: EnvironmentId;
  readonly threadId: ThreadId;
  readonly childId: string;
  readonly threadRef?: ScopedThreadRef | undefined;
  readonly markdownCwd?: string | undefined;
}

/**
 * How each terminal outcome is named.
 *
 * Keyed by the contract's own closed literal rather than by a string, so a kind
 * added to `OrchestrationSubagentOutcomeKind` is a type error here instead of a
 * child whose conclusion silently reads as "Result". **Empty** is worded as the
 * answer it is: a child that finished and returned nothing has concluded, and a
 * blank tail would otherwise read as a stream that broke off.
 */
const OUTCOME_LABELS: Record<OrchestrationSubagentOutcomeKind, string> = {
  completed: "Result",
  empty: "Completed — no result returned",
  failed: "Failed",
  interrupted: "Interrupted",
};

/** The text an entry carries, whatever kind it is. `""` when it carries none. */
function entryText(entry: OrchestrationSubagentEntry): string {
  const payload = entry.payload as { text?: unknown } | null;
  return typeof payload?.text === "string" ? payload.text : "";
}

function outcomeKind(entry: OrchestrationSubagentEntry): OrchestrationSubagentOutcomeKind {
  const payload = entry.payload as { kind?: unknown } | null;
  return typeof payload?.kind === "string" && payload.kind in OUTCOME_LABELS
    ? (payload.kind as OrchestrationSubagentOutcomeKind)
    : "completed";
}

/**
 * The child's conclusion, as the work-log row the main agent's tool calls use.
 *
 * The row carries the outcome's *name* and the result text is rendered beneath
 * it as ordinary markdown rather than as the row's truncated preview: a child's
 * final report is the thing the developer opened the tab to read, and folding
 * it into one line behind a disclosure would bury it.
 */
function outcomeWorkEntry(entry: OrchestrationSubagentEntry): WorkLogEntry {
  const kind = outcomeKind(entry);
  return {
    id: entry.id,
    createdAt: entry.createdAt,
    label: OUTCOME_LABELS[kind],
    toolTitle: OUTCOME_LABELS[kind],
    tone: kind === "failed" ? "error" : "tool",
    itemType: "collab_agent_tool_call",
    toolLifecycleStatus: kind === "failed" ? "failed" : "completed",
  };
}

export function SubagentStreamPanel(props: SubagentStreamPanelProps) {
  const query = useEnvironmentQuery(
    subagentEnvironment.stream({
      environmentId: props.environmentId,
      input: { threadId: props.threadId, childId: props.childId },
    }),
  );
  const state = query.data ?? null;

  // An unresolvable child is said out loud rather than left as a blank tab: a
  // conversation whose children were deleted with it, or a reference from a
  // build before the stream model existed, both land here.
  if (query.error !== null) {
    return (
      <div
        className="flex min-h-0 flex-1 items-center justify-center p-6 text-center text-xs text-muted-foreground"
        data-subagent-state="unavailable"
      >
        This subagent&rsquo;s work is no longer available.
      </div>
    );
  }

  if (state === null || state.stream === null) {
    return (
      <div
        className="flex min-h-0 flex-1 items-center justify-center p-6 text-xs text-muted-foreground"
        data-subagent-state="loading"
      >
        <Loader2 className="mr-2 size-3.5 animate-spin" />
        Loading the subagent&rsquo;s work…
      </div>
    );
  }

  if (state.entries.length === 0) {
    return (
      <div
        className="flex min-h-0 flex-1 items-center justify-center p-6 text-xs text-muted-foreground"
        data-subagent-state="empty"
      >
        No subagent activity yet.
      </div>
    );
  }

  return (
    <ScrollArea className="min-h-0 flex-1" data-subagent-state={state.stream.state}>
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-2 px-3 py-3">
        {state.entries.map((entry) => {
          const text = entryText(entry);
          if (entry.kind !== "outcome") {
            return (
              <div className="min-w-0 px-1 py-0.5" key={entry.id} data-subagent-entry="message">
                <ChatMarkdown text={text} cwd={props.markdownCwd} threadRef={props.threadRef} />
              </div>
            );
          }
          return (
            <div className="min-w-0 px-1 py-0.5" key={entry.id} data-subagent-entry="outcome">
              <SimpleWorkEntryRow workEntry={outcomeWorkEntry(entry)} workspaceRoot={undefined} />
              {text.trim().length > 0 ? (
                <div className="mt-1">
                  <ChatMarkdown text={text} cwd={props.markdownCwd} threadRef={props.threadRef} />
                </div>
              ) : null}
            </div>
          );
        })}
      </div>
    </ScrollArea>
  );
}

export default SubagentStreamPanel;
