/**
 * One delegated child's work stream, as a right-panel surface.
 *
 * **Read-only, and no chrome of its own.** No composer, no identity or task
 * header, no per-child stop: the inline row in the conversation is where a
 * child's identity, assignment and state are reported, and this opens directly
 * into the work. The file and diff affordances below are the one thing here that
 * can be clicked, and what they do is open a *neighbouring* workspace tab —
 * nothing on this surface can reach the provider.
 *
 * **Nothing here draws a child's work itself.** Prose goes through
 * `ChatMarkdown` and everything else through `SimpleWorkEntryRow` — the same two
 * components the main agent's transcript uses — because the spec asks for
 * exactly that: "Reuse the main agent's transcript and work-entry components in
 * a read-only configuration." So a command from a child reads as a command, an
 * edit as a file change, a failed call as a failed call, with the icons,
 * headings, disclosure and status indicators the developer already knows. A
 * bespoke renderer here would be a second, worse work log to keep in step with
 * the first.
 *
 * `workEntryForKind` is the whole of the translation, and it is deliberately
 * thin: the server has already put each entry in the client's own work-log
 * vocabulary (`OrchestrationSubagentWork`), so this maps names rather than
 * deciding anything.
 *
 * The stream is fetched only while this is mounted. Closing the tab releases
 * the view and stops nothing: the server goes on recording the child, and
 * reopening the tab replays what it recorded meanwhile.
 */
import type {
  EnvironmentId,
  OrchestrationSubagentEntry,
  OrchestrationSubagentOutcomeKind,
  OrchestrationSubagentWork,
  ScopedThreadRef,
  ThreadId,
} from "@t3tools/contracts";
import { FileDiffIcon, Loader2 } from "lucide-react";

import type { WorkLogEntry } from "../../session-logic";
import { useEnvironmentQuery } from "../../state/query";
import { subagentEnvironment } from "../../state/subagents";
import { openSubagentDiff, openSubagentFile } from "../../subagentFileActions";
import { ScrollArea } from "~/components/ui/scroll-area";
import ChatMarkdown from "../ChatMarkdown";
import { SimpleWorkEntryRow } from "./MessagesTimeline";

interface SubagentStreamPanelProps {
  readonly environmentId: EnvironmentId;
  readonly threadId: ThreadId;
  readonly childId: string;
  readonly threadRef?: ScopedThreadRef | undefined;
  readonly markdownCwd?: string | undefined;
  readonly workspaceRoot?: string | undefined;
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

/**
 * The child's conclusion, as the work-log row the main agent's tool calls use.
 *
 * The row carries the outcome's *name* and the result text is rendered beneath
 * it as ordinary markdown rather than as the row's truncated preview: a child's
 * final report is the thing the developer opened the tab to read, and folding
 * it into one line behind a disclosure would bury it.
 */
function outcomeWorkEntry(
  entry: Extract<OrchestrationSubagentEntry, { kind: "outcome" }>,
): WorkLogEntry {
  const kind = entry.payload.kind;
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

type WorkEntryKind = "command" | "read" | "edit" | "tool";

/**
 * One piece of the child's work, as the row the main agent would draw for the
 * same thing.
 *
 * The `itemType` per kind is not a new taxonomy — it is the one
 * `opencode_item_type` already assigns to the same tools in the parent
 * transcript, so a child's `bash` and the root agent's `bash` are the same row.
 * An `edit`'s paths become `changedFiles`, which is what gives it the file-change
 * icon and the path preview the conversation uses; a read's do not, because a
 * read is not a change and drawing it as one would misreport what the child did.
 */
function workEntryForKind(
  kind: WorkEntryKind,
  entry: { id: string; createdAt: string },
  work: OrchestrationSubagentWork,
): WorkLogEntry {
  const shared = {
    id: entry.id,
    createdAt: entry.createdAt,
    label: work.title,
    toolTitle: work.title,
    tone: (work.status === "failed" ? "error" : "tool") as WorkLogEntry["tone"],
    toolLifecycleStatus: work.status,
    ...(work.detail ? { detail: work.detail } : {}),
  };
  switch (kind) {
    case "command":
      return {
        ...shared,
        itemType: "command_execution",
        ...(work.command ? { command: work.command } : {}),
      };
    case "edit":
      return { ...shared, itemType: "file_change", changedFiles: work.paths };
    case "read":
    case "tool":
      return { ...shared, itemType: "dynamic_tool_call" };
  }
}

/**
 * A blocker, as the row the conversation draws for the same wait.
 *
 * A question borrows the main agent's own `user-input.requested` chrome, because
 * it *is* the same event seen from the child's side. A permission has no
 * equivalent inert row in the transcript — the conversation shows it as an
 * actionable panel, which is where it stays — so it is drawn as an ordinary
 * informational row saying what the child stopped for.
 */
function blockerWorkEntry(
  entry: Extract<OrchestrationSubagentEntry, { kind: "blocker" }>,
): WorkLogEntry {
  const { blocker, title, detail, resolution } = entry.payload;
  const label = resolution
    ? `${resolution}: ${title}`
    : blocker === "question"
      ? "Waiting for your answer"
      : `Waiting for permission: ${title}`;
  return {
    id: entry.id,
    createdAt: entry.createdAt,
    label,
    toolTitle: label,
    tone: "info",
    ...(detail ? { detail } : {}),
    ...(blocker === "question" ? { sourceActivityKind: "user-input.requested" as const } : {}),
  };
}

function noticeWorkEntry(
  entry: Extract<OrchestrationSubagentEntry, { kind: "notice" }>,
): WorkLogEntry {
  return {
    id: entry.id,
    createdAt: entry.createdAt,
    label: entry.payload.text,
    toolTitle: entry.payload.text,
    tone: "error",
    sourceActivityKind: entry.payload.level === "warning" ? "runtime.warning" : "runtime.error",
  };
}

/**
 * The files a child entry names, as links into the workspace beside this tab.
 *
 * Rendered only when there is somewhere to open them — a surface with no thread
 * reference has no workspace to open a file in, and an affordance that did
 * nothing would be worse than none. The diff is offered for an edit only:
 * reading a file changes nothing, and there would be no diff to show.
 */
function EntryFileActions(props: {
  paths: ReadonlyArray<string>;
  threadRef: ScopedThreadRef | undefined;
  workspaceRoot: string | undefined;
  diffable: boolean;
}) {
  if (!props.threadRef || props.paths.length === 0) return null;
  const threadRef = props.threadRef;
  return (
    <div className="mt-0.5 flex flex-wrap items-center gap-1 pl-6">
      {props.paths.map((path) => (
        <button
          key={path}
          type="button"
          title={path}
          data-subagent-open-file={path}
          className="inline-flex max-w-64 items-center rounded-md border border-border/70 bg-background/45 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={() => openSubagentFile(threadRef, path)}
        >
          <span className="truncate">{displayPath(path, props.workspaceRoot)}</span>
        </button>
      ))}
      {props.diffable ? (
        <button
          type="button"
          data-subagent-open-diff="true"
          className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={() => openSubagentDiff(threadRef)}
        >
          <FileDiffIcon className="size-3" aria-hidden />
          Open diff
        </button>
      ) : null}
    </div>
  );
}

function displayPath(path: string, workspaceRoot: string | undefined): string {
  if (!workspaceRoot) return path;
  const prefix = workspaceRoot.endsWith("/") ? workspaceRoot : `${workspaceRoot}/`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
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
        {state.entries.map((entry) => (
          <SubagentStreamEntry
            key={entry.id}
            entry={entry}
            threadRef={props.threadRef}
            markdownCwd={props.markdownCwd}
            workspaceRoot={props.workspaceRoot}
          />
        ))}
      </div>
    </ScrollArea>
  );
}

/**
 * One entry, in the order the child produced it.
 *
 * Exhaustive over the contract's entry union rather than defaulted, so a kind
 * added to `OrchestrationSubagentEntry` is a type error here instead of a piece
 * of a child's work that silently disappears from the tab.
 */
function SubagentStreamEntry(props: {
  entry: OrchestrationSubagentEntry;
  threadRef: ScopedThreadRef | undefined;
  markdownCwd: string | undefined;
  workspaceRoot: string | undefined;
}) {
  const { entry } = props;
  switch (entry.kind) {
    case "message":
      return (
        <div className="min-w-0 px-1 py-0.5" data-subagent-entry="message">
          <ChatMarkdown
            text={entry.payload.text}
            cwd={props.markdownCwd}
            threadRef={props.threadRef}
          />
        </div>
      );
    case "command":
    case "read":
    case "edit":
    case "tool":
      return (
        <div className="min-w-0 px-1 py-0.5" data-subagent-entry={entry.kind}>
          <SimpleWorkEntryRow
            workEntry={workEntryForKind(entry.kind, entry, entry.payload)}
            workspaceRoot={props.workspaceRoot}
          />
          <EntryFileActions
            paths={entry.payload.paths}
            threadRef={props.threadRef}
            workspaceRoot={props.workspaceRoot}
            diffable={entry.kind === "edit"}
          />
        </div>
      );
    case "notice":
      return (
        <div className="min-w-0 px-1 py-0.5" data-subagent-entry="notice">
          <SimpleWorkEntryRow workEntry={noticeWorkEntry(entry)} workspaceRoot={undefined} />
        </div>
      );
    case "blocker":
      return (
        <div className="min-w-0 px-1 py-0.5" data-subagent-entry="blocker">
          <SimpleWorkEntryRow workEntry={blockerWorkEntry(entry)} workspaceRoot={undefined} />
        </div>
      );
    case "outcome":
      return (
        <div className="min-w-0 px-1 py-0.5" data-subagent-entry="outcome">
          <SimpleWorkEntryRow workEntry={outcomeWorkEntry(entry)} workspaceRoot={undefined} />
          {entry.payload.text && entry.payload.text.trim().length > 0 ? (
            <div className="mt-1">
              <ChatMarkdown
                text={entry.payload.text}
                cwd={props.markdownCwd}
                threadRef={props.threadRef}
              />
            </div>
          ) : null}
        </div>
      );
  }
}

export default SubagentStreamPanel;
