import { memo } from "react";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";

export const ComposerPlanFollowUpBanner = memo(function ComposerPlanFollowUpBanner({
  planTitle,
  decision,
}: {
  planTitle: string | null;
  decision?:
    | {
        planId: string;
        canImplement: boolean;
        canSaveAndStop: boolean;
        onDecide: (planId: string, decision: "implement" | "save-and-stop") => void;
      }
    | undefined;
}) {
  return (
    <div className="px-4 py-3.5 sm:px-5 sm:py-4">
      <div className="flex flex-wrap items-center gap-2">
        <Badge
          variant="info"
          size="sm"
          className="rounded-md px-1.5 py-0 font-semibold tracking-wide uppercase"
        >
          Plan Ready
        </Badge>
        {planTitle ? (
          <span className="min-w-0 flex-1 truncate text-sm font-medium">{planTitle}</span>
        ) : null}
      </div>
      {decision ? (
        <div className="mt-3 flex flex-wrap gap-2">
          <Button
            type="button"
            size="sm"
            disabled={!decision.canImplement}
            onClick={() => decision.onDecide(decision.planId, "implement")}
          >
            Implement
          </Button>
          <Button
            type="button"
            size="sm"
            variant="outline"
            disabled={!decision.canSaveAndStop}
            onClick={() => decision.onDecide(decision.planId, "save-and-stop")}
          >
            Save and stop
          </Button>
        </div>
      ) : null}
    </div>
  );
});
