/**
 * Who is waiting on a decision, when it is not the root agent.
 *
 * Beside the request's summary rather than inside its detail, because it changes
 * what the decision *means*: approving a command a delegated worker asked for is
 * not the same act as approving one the agent you are talking to asked for, and
 * the developer has to be able to tell before they click.
 *
 * Its own file because both request panels use it — the permission one and the
 * question one — and neither owns it.
 */
import type { RequestingSubagent } from "../../session-logic";

export function WaitingSubagentLabel({ subagent }: { subagent: RequestingSubagent | undefined }) {
  if (!subagent) return null;
  return (
    <span className="text-sm text-muted-foreground" data-waiting-subagent={subagent.childId}>
      {subagent.name ? `from subagent ${subagent.name}` : "from a subagent"}
    </span>
  );
}
