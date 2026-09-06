import { CheckIcon, ChevronDownIcon, CircleIcon, LoaderIcon, XIcon, BanIcon } from "lucide-react";
import type { ActivePlanState } from "../../session-logic";
import { cn } from "~/lib/utils";

export function TaskStatusIcon({
  status,
  running = true,
}: {
  status: ActivePlanState["steps"][number]["status"];
  running?: boolean;
}) {
  const Icon =
    status === "completed"
      ? CheckIcon
      : status === "failed"
        ? XIcon
        : status === "cancelled"
          ? BanIcon
          : status === "inProgress" && running
            ? LoaderIcon
            : CircleIcon;
  const label = status === "inProgress" ? (running ? "In progress" : "Paused") : status;
  return (
    <Icon
      aria-label={label}
      className={cn(
        "size-3.5 shrink-0",
        status === "completed"
          ? "text-success-foreground"
          : status === "failed"
            ? "text-destructive"
            : "text-muted-foreground",
        status === "inProgress" && running && "animate-spin text-primary",
      )}
    />
  );
}

/** A provider-neutral summary of the same task snapshot used by the plan sidebar. */
export function ComposerTaskProgress({
  plan,
  running,
}: {
  plan: ActivePlanState;
  running: boolean;
}) {
  if (plan.steps.length === 0) return null;
  const completed = plan.steps.filter((step) => step.status === "completed").length;
  const current =
    plan.steps.find((step) => step.status === "inProgress") ??
    plan.steps.find((step) => step.status === "failed") ??
    plan.steps.find((step) => step.status === "pending");
  return (
    <details aria-label="Task progress" className="group/tasks border-b border-border/60 text-xs">
      <summary className="flex cursor-pointer list-none items-center gap-2 px-4 py-2.5 text-muted-foreground [&::-webkit-details-marker]:hidden">
        <TaskStatusIcon
          status={current?.status ?? (completed === plan.steps.length ? "completed" : "cancelled")}
          running={running}
        />
        <span className="shrink-0 tabular-nums">
          {completed}/{plan.steps.length} completed
        </span>
        <span className="min-w-0 flex-1 truncate text-foreground/80">
          {current?.step ?? "Tasks"}
        </span>
        <ChevronDownIcon
          aria-hidden
          className="size-3.5 shrink-0 transition-transform group-open/tasks:rotate-180"
        />
      </summary>
      <div className="max-h-48 overflow-y-auto px-4 pb-3">
        {plan.explanation ? <p className="mb-2 text-muted-foreground">{plan.explanation}</p> : null}
        <ol className="space-y-2" aria-label="Tasks">
          {plan.steps.map((step, index) => (
            <li key={index} className="flex items-start gap-2">
              <TaskStatusIcon status={step.status} running={running} />
              <span
                className={cn(
                  "min-w-0 break-words",
                  (step.status === "completed" || step.status === "cancelled") &&
                    "text-muted-foreground",
                )}
              >
                {step.step}
              </span>
            </li>
          ))}
        </ol>
      </div>
    </details>
  );
}
