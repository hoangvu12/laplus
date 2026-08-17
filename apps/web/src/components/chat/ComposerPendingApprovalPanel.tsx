import { memo } from "react";
import { type PendingApproval, type RequestingSubagent } from "../../session-logic";

interface ComposerPendingApprovalPanelProps {
  approval: PendingApproval;
  pendingCount: number;
}

/**
 * Who is waiting, when it is not the root agent.
 *
 * Beside the summary rather than inside the detail, because it changes what the
 * decision *means*: approving a command a delegated worker asked for is not the
 * same act as approving one the agent you are talking to asked for, and the
 * developer has to be able to tell before they click.
 */
export function WaitingSubagentLabel({ subagent }: { subagent: RequestingSubagent | undefined }) {
  if (!subagent) return null;
  return (
    <span className="text-sm text-muted-foreground" data-waiting-subagent={subagent.childId}>
      {subagent.name ? `from subagent ${subagent.name}` : "from a subagent"}
    </span>
  );
}

export const ComposerPendingApprovalPanel = memo(function ComposerPendingApprovalPanel({
  approval,
  pendingCount,
}: ComposerPendingApprovalPanelProps) {
  const approvalSummary =
    approval.requestKind === "command"
      ? "Command approval requested"
      : approval.requestKind === "file-read"
        ? "File-read approval requested"
        : "File-change approval requested";
  const detailLabel =
    approval.requestKind === "command"
      ? "Command"
      : approval.requestKind === "file-read"
        ? "File to read"
        : "File change";

  return (
    <div className="px-4 py-3.5 sm:px-5 sm:py-4">
      <div className="flex flex-wrap items-center gap-2">
        <span className="uppercase text-sm tracking-[0.2em]">PENDING APPROVAL</span>
        <span className="text-sm font-medium">{approvalSummary}</span>
        <WaitingSubagentLabel subagent={approval.subagent} />
        {pendingCount > 1 ? (
          <span className="text-xs text-muted-foreground">1/{pendingCount}</span>
        ) : null}
      </div>
      {approval.detail ? (
        <div className="mt-3 rounded-lg border border-border/65 bg-background/70 p-3">
          <p className="text-xs font-medium text-muted-foreground">{detailLabel}</p>
          <pre
            aria-label={detailLabel}
            className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-foreground"
            data-approval-detail="complete"
          >
            {approval.detail}
          </pre>
        </div>
      ) : null}
    </div>
  );
});
