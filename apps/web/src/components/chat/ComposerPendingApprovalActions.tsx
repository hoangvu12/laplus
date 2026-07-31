import { type ApprovalRequestId, type ProviderApprovalDecision } from "@t3tools/contracts";
import { memo } from "react";
import { Button } from "../ui/button";

interface ComposerPendingApprovalActionsProps {
  requestId: ApprovalRequestId;
  availableDecisions?: ReadonlyArray<ProviderApprovalDecision>;
  isResponding: boolean;
  onRespondToApproval: (
    requestId: ApprovalRequestId,
    decision: ProviderApprovalDecision,
  ) => Promise<unknown>;
}

export const ComposerPendingApprovalActions = memo(function ComposerPendingApprovalActions({
  requestId,
  availableDecisions,
  isResponding,
  onRespondToApproval,
}: ComposerPendingApprovalActionsProps) {
  const offered = new Set(
    availableDecisions ?? ["accept", "acceptForSession", "decline", "cancel"],
  );
  return (
    <>
      {offered.has("cancel") ? (
        <Button
          size="sm"
          variant="ghost"
          disabled={isResponding}
          onClick={() => void onRespondToApproval(requestId, "cancel")}
        >
          Cancel turn
        </Button>
      ) : null}
      {offered.has("decline") ? (
        <Button
          size="sm"
          variant="destructive-outline"
          disabled={isResponding}
          onClick={() => void onRespondToApproval(requestId, "decline")}
        >
          Decline
        </Button>
      ) : null}
      {offered.has("acceptForSession") ? (
        <Button
          size="sm"
          variant="outline"
          disabled={isResponding}
          onClick={() => void onRespondToApproval(requestId, "acceptForSession")}
        >
          Always allow this session
        </Button>
      ) : null}
      {offered.has("accept") ? (
        <Button
          size="sm"
          variant="default"
          disabled={isResponding}
          onClick={() => void onRespondToApproval(requestId, "accept")}
        >
          Approve once
        </Button>
      ) : null}
    </>
  );
});
