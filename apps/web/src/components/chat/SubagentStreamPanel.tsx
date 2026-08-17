/**
 * One delegated child's work stream, as a right-panel surface.
 *
 * **Read-only, and no chrome of its own.** No composer, no identity or task
 * header, no per-child stop: the inline row in the conversation is where a
 * child's identity, assignment and state are reported, and this opens directly
 * into the work. What it renders it renders through the main agent's own
 * language — `ChatMarkdown` for prose, the work-entry row shape for the
 * conclusion — so a child's reply reads the way the parent's does.
 *
 * The stream is fetched only while this is mounted. Closing the tab releases the
 * view and stops nothing: the server goes on recording the child, and reopening
 * the tab replays what it recorded meanwhile.
 */
import type { EnvironmentId, OrchestrationSubagentEntry, ThreadId } from "@t3tools/contracts";
import type { ScopedThreadRef } from "@t3tools/contracts";
import { CircleAlert, CircleCheck, CircleSlash, Loader2 } from "lucide-react";

import { useEnvironmentQuery } from "../../state/query";
import { subagentEnvironment } from "../../state/subagents";
import { cn } from "~/lib/utils";
import { ScrollArea } from "~/components/ui/scroll-area";
import ChatMarkdown from "../ChatMarkdown";

interface SubagentStreamPanelProps {
  readonly environmentId: EnvironmentId;
  readonly threadId: ThreadId;
  readonly childId: string;
  readonly threadRef?: ScopedThreadRef | undefined;
  readonly markdownCwd?: string | undefined;
}

interface OutcomePayload {
  readonly kind?: string;
  readonly text?: string | null;
}

function entryText(entry: OrchestrationSubagentEntry): string {
  const payload = entry.payload as { text?: unknown } | null;
  return typeof payload?.text === "string" ? payload.text : "";
}

/**
 * The four terminal answers, worded as answers. An `empty` child has concluded
 * and returned nothing — which is a result, and must not read as a stream that
 * broke off.
 */
const OUTCOMES = {
  completed: { icon: CircleCheck, tone: "text-muted-foreground/65", label: "Result" },
  empty: {
    icon: CircleCheck,
    tone: "text-muted-foreground/65",
    label: "Completed — no result returned",
  },
  failed: { icon: CircleAlert, tone: "text-destructive", label: "Failed" },
  interrupted: { icon: CircleSlash, tone: "text-muted-foreground/65", label: "Interrupted" },
} as const;

function OutcomeEntry(props: {
  entry: OrchestrationSubagentEntry;
  threadRef?: ScopedThreadRef | undefined;
  markdownCwd?: string | undefined;
}) {
  const payload = props.entry.payload as OutcomePayload | null;
  const outcome =
    OUTCOMES[(payload?.kind ?? "completed") as keyof typeof OUTCOMES] ?? OUTCOMES.completed;
  const Icon = outcome.icon;
  const text = typeof payload?.text === "string" ? payload.text : "";
  return (
    <div className="min-w-0 px-1 py-0.5" data-subagent-entry="outcome">
      <div className={cn("flex items-center gap-2 text-xs", outcome.tone)}>
        <Icon className="size-3.5 shrink-0" />
        <span className="font-medium">{outcome.label}</span>
      </div>
      {text.trim().length > 0 ? (
        <div className="mt-1">
          <ChatMarkdown text={text} cwd={props.markdownCwd} threadRef={props.threadRef} />
        </div>
      ) : null}
    </div>
  );
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
        {state.entries.map((entry) =>
          entry.kind === "outcome" ? (
            <OutcomeEntry
              key={entry.id}
              entry={entry}
              threadRef={props.threadRef}
              markdownCwd={props.markdownCwd}
            />
          ) : (
            <div className="min-w-0 px-1 py-0.5" key={entry.id} data-subagent-entry="message">
              <ChatMarkdown
                text={entryText(entry)}
                cwd={props.markdownCwd}
                threadRef={props.threadRef}
              />
            </div>
          ),
        )}
      </div>
    </ScrollArea>
  );
}

export default SubagentStreamPanel;
