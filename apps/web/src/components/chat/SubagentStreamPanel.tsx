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
 *
 * **Where the reader is, and whether they are still watching, is not this
 * file's.** `SubagentStreamScroller` is the shell around the entries below and
 * owns following, suspending, jump-to-latest and this tab's remembered place.
 * The two are separate because they change for different reasons: another kind
 * of child entry is a change to what is read, not to how it is read.
 */
import type {
  EnvironmentId,
  OrchestrationSubagentEntry,
  OrchestrationSubagentLauncher,
  OrchestrationSubagentOutcomeKind,
  OrchestrationSubagentResolution,
  OrchestrationSubagentState,
  OrchestrationSubagentWork,
  ScopedThreadRef,
  ThreadId,
} from "@t3tools/contracts";
import { scopeThreadRef } from "@t3tools/client-runtime/environment";
import { FileDiffIcon, Loader2 } from "lucide-react";

import { formatWorkspaceRelativePath } from "../../filePathDisplay";
import type { WorkLogEntry, WorkLogToolLifecycleStatus } from "../../session-logic";
import { useEnvironmentQuery } from "../../state/query";
import { subagentEnvironment } from "../../state/subagents";
import { openSubagentSurface } from "../../rightPanelStore";
import { openSubagentDiff, openSubagentFile, subagentFileTarget } from "../../subagentFileActions";
import ChatMarkdown from "../ChatMarkdown";
import { SimpleWorkEntryRow } from "./MessagesTimeline";
import { SubagentStreamScroller } from "./SubagentStreamScroller";
import { subagentScrollKey } from "./subagentScroll";

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
 * The `itemType` per kind is not a new taxonomy — it is the one the parent
 * transcript already uses, so a child's command and the root agent's command
 * are the same row, and a child's edit and the root agent's edit are the same
 * row. An `edit`'s paths become `changedFiles`, which is what gives it the
 * file-change icon and the path preview the conversation uses; a read's do not,
 * because a read is not a change and drawing it as one would misreport what the
 * child did.
 *
 * **Where the two part company:** the child stream's `read` and `tool` kinds are
 * coarser than the parent's six `itemType`s, so a child's web fetch, MCP call or
 * image view draws the generic tool row where the root agent's would draw a
 * globe, a wrench or an eye. Same component, same language, less specific icon.
 * `child_entry_kind` in `opencode.rs` carries why, and closing it is a decision
 * about the shared entry vocabulary rather than about this renderer.
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
 * How each way a blocker can end is named.
 *
 * Keyed by the contract's own closed literal, like `OUTCOME_LABELS` and for its
 * reason: the wire carries the resolution's *identity* and the wording is this
 * client's, so a resolution added to `OrchestrationSubagentResolution` is a type
 * error here rather than a blocker whose end silently goes unlabelled.
 * **undelivered** is worded as the news it is — the developer decided and the
 * child was never told, which is not the same as having been refused.
 */
const RESOLUTION_LABELS: Record<OrchestrationSubagentResolution, string> = {
  approved: "Approved",
  approvedForSession: "Approved for this session",
  declined: "Declined",
  cancelled: "Cancelled",
  answered: "Answered",
  rejected: "Rejected",
  undelivered: "Your decision could not be delivered",
};

/**
 * A blocker, as the row the conversation draws for the same wait.
 *
 * A question borrows the main agent's own `user-input.requested` chrome, because
 * it *is* the same event seen from the child's side. A permission has no
 * equivalent inert row in the transcript — the conversation shows it as an
 * actionable panel, which is where it stays — so it is drawn as an ordinary
 * informational row saying what the child stopped for.
 *
 * Its twin is `blocker_activity` in `opencode.rs`, which says the same thing on
 * the *compact* row. The sentence is written twice on purpose — see that
 * function for why sending it over the wire would be worse.
 */
function blockerWorkEntry(
  entry: Extract<OrchestrationSubagentEntry, { kind: "blocker" }>,
): WorkLogEntry {
  const { blocker, title, detail, resolution } = entry.payload;
  const label = resolution
    ? `${RESOLUTION_LABELS[resolution]}: ${title}`
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

/**
 * How a nested child's state reads on its launcher, and which work-entry
 * lifecycle the shared row draws it in.
 *
 * One table rather than a label table beside a cascade of ternaries, because
 * both answers are the same question about the same closed literal — and keyed
 * by that literal, like `OUTCOME_LABELS` and `RESOLUTION_LABELS`, so a state
 * added to `OrchestrationSubagentState` is a type error here rather than a
 * descendant whose row silently loses its status.
 */
const LAUNCHER_STATES: Record<
  OrchestrationSubagentState,
  { readonly label: string; readonly status: WorkLogToolLifecycleStatus }
> = {
  pending: { label: "Pending", status: "inProgress" },
  working: { label: "Working", status: "inProgress" },
  blocked: { label: "Blocked", status: "inProgress" },
  completed: { label: "Completed", status: "completed" },
  interrupted: { label: "Interrupted", status: "stopped" },
  failed: { label: "Failed", status: "failed" },
};

/**
 * A child this child launched, as the same compact row the conversation draws
 * for a direct child.
 *
 * `subagentChildId` is what makes it a launcher rather than a mention:
 * `SimpleWorkEntryRow` reads it and offers the identical "Open subagent work
 * stream" activation the inline row in the transcript offers, so a nested child
 * opens as another ordinary right-panel tab and clicking one already open
 * activates it. Nothing about the descendant's own work is copied here — that
 * lives in its own stream, which is what this opens.
 */
function launcherWorkEntry(
  entry: Extract<OrchestrationSubagentEntry, { kind: "subagent" }>,
): WorkLogEntry {
  const launcher: OrchestrationSubagentLauncher = entry.payload;
  const label = `Subagent ${launcher.name ?? launcher.childId}`;
  const reached = LAUNCHER_STATES[launcher.state];
  return {
    id: entry.id,
    createdAt: entry.createdAt,
    label,
    toolTitle: label,
    tone: launcher.outcome?.kind === "failed" ? "error" : "tool",
    itemType: "collab_agent_tool_call",
    toolLifecycleStatus: reached.status,
    subagentChildId: launcher.childId,
    detail: launcherDetail(launcher, reached.label),
  };
}

/**
 * What a nested launcher's row says it is doing, or what came back.
 *
 * **A terminal descendant never falls back to its assignment.** The spec's rule
 * is that terminal state "replace[s] latest activity atomically with a bounded
 * result, failure, interruption, or empty-result preview" — and an assignment is
 * what the child was asked before it began, which is staler than the activity
 * the rule exists to displace. A descendant that was interrupted, or failed
 * without saying why, or completed without a report, carries `outcome.text:
 * null`, so reading the text alone would have shown its task as though it were
 * its answer.
 *
 * `OUTCOME_LABELS` is the same sentence the child's own conclusion row uses, so
 * a descendant reads the same in its parent's stream as it does in its own tab.
 */
function launcherDetail(launcher: OrchestrationSubagentLauncher, reached: string): string {
  if (launcher.outcome) {
    return launcher.outcome.text ?? OUTCOME_LABELS[launcher.outcome.kind];
  }
  return launcher.assignment ?? reached;
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
 * Rendered only where there is somewhere to open them. Three things can make
 * that false, and all three end in no affordance rather than a broken one: no
 * thread reference (no workspace at all), a path the file surface cannot be
 * addressed by (`subagentFileTarget` — a child may legitimately read a file
 * outside the workspace), and no paths on the entry. The diff is offered for an
 * edit only: reading a file changes nothing, so there would be no diff to show.
 *
 * The label is the main agent's own path formatting, so a file a child touched
 * reads the way the same file reads in the conversation.
 */
function EntryFileActions(props: {
  paths: ReadonlyArray<string>;
  threadRef: ScopedThreadRef | undefined;
  workspaceRoot: string | undefined;
  diffable: boolean;
}) {
  const threadRef = props.threadRef;
  if (!threadRef) return null;
  const openable = props.paths.flatMap((path) => {
    const target = subagentFileTarget(path, props.workspaceRoot);
    return target === null ? [] : [{ path, target }];
  });
  if (openable.length === 0) return null;
  return (
    <div className="mt-0.5 flex flex-wrap items-center gap-1 pl-6">
      {openable.map(({ path, target }) => (
        <button
          key={path}
          type="button"
          title={path}
          data-subagent-open-file={target}
          className="inline-flex max-w-64 items-center rounded-md border border-border/70 bg-background/45 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground transition-colors hover:bg-accent/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onClick={() => openSubagentFile(threadRef, target)}
        >
          <span className="truncate">{formatWorkspaceRelativePath(path, props.workspaceRoot)}</span>
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
    <SubagentStreamScroller
      surfaceKey={subagentScrollKey(
        scopeThreadRef(props.environmentId, props.threadId),
        props.childId,
      )}
      streamState={state.stream.state}
    >
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
    </SubagentStreamScroller>
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
    case "subagent":
      return (
        <div className="min-w-0 px-1 py-0.5" data-subagent-entry="subagent">
          <SimpleWorkEntryRow
            workEntry={launcherWorkEntry(entry)}
            workspaceRoot={undefined}
            onOpenSubagent={(childId) => openSubagentSurface(props.threadRef, childId)}
          />
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
