import type {
  OrchestrationThreadActivity,
  PromptSkillSelection,
  ServerProviderSkill,
} from "@t3tools/contracts";

export interface ComposerSkillSelection {
  name: string;
  path: string | null;
  visibleText: string;
  /** UTF-16 offsets while the text is edited in the browser. */
  start: number;
  end: number;
}

export interface MimirSkillCatalogScope {
  providerInstanceId: string;
  cwd: string;
  sdkSessionId: string;
}

export interface MimirSkillCatalog {
  scope: MimirSkillCatalogScope;
  skills: ServerProviderSkill[];
}

export interface MimirSkillPreparation {
  providerInstanceId: string;
  cwd: string;
  previousActivityIds: ReadonlySet<string>;
}

export type MimirSkillPreparationOutcome =
  | { status: "pending" }
  | { status: "ready" }
  | { status: "failed"; message: string };

function isMimirSkillCatalog(value: unknown): value is MimirSkillCatalog {
  if (!value || typeof value !== "object") return false;
  const payload = value as Partial<MimirSkillCatalog>;
  const scope = payload.scope as Partial<MimirSkillCatalogScope> | undefined;
  return (
    Array.isArray(payload.skills) &&
    typeof scope?.providerInstanceId === "string" &&
    typeof scope.cwd === "string" &&
    typeof scope.sdkSessionId === "string" &&
    scope.sdkSessionId.length > 0
  );
}

export function selectMimirSkillCatalog(
  activities: ReadonlyArray<OrchestrationThreadActivity>,
  providerInstanceId: string,
  cwd: string | null,
): MimirSkillCatalog | null {
  if (!cwd) return null;
  for (let index = activities.length - 1; index >= 0; index -= 1) {
    const activity = activities[index];
    if (activity?.kind !== "provider.skills" || !isMimirSkillCatalog(activity.payload)) continue;
    if (
      activity.payload.scope.providerInstanceId === providerInstanceId &&
      activity.payload.scope.cwd === cwd
    ) {
      return activity.payload;
    }
  }
  return null;
}
export function resolveMimirSkillPreparation(
  activities: ReadonlyArray<OrchestrationThreadActivity>,
  preparation: MimirSkillPreparation,
): MimirSkillPreparationOutcome {
  for (let index = activities.length - 1; index >= 0; index -= 1) {
    const activity = activities[index];
    if (!activity || preparation.previousActivityIds.has(activity.id)) continue;
    if (
      activity.tone === "error" &&
      (activity.kind === "provider.skills" || activity.kind === "session.failed")
    ) {
      const payload =
        activity.payload && typeof activity.payload === "object"
          ? (activity.payload as { detail?: unknown })
          : null;
      return {
        status: "failed",
        message: typeof payload?.detail === "string" ? payload.detail : activity.summary,
      };
    }
    if (
      activity.kind === "provider.skills" &&
      isMimirSkillCatalog(activity.payload) &&
      activity.payload.scope.providerInstanceId === preparation.providerInstanceId &&
      activity.payload.scope.cwd === preparation.cwd
    ) {
      return { status: "ready" };
    }
  }
  return { status: "pending" };
}

export function selectedComposerSkill(
  skill: ServerProviderSkill,
  start: number,
): ComposerSkillSelection {
  const visibleText = `$${skill.name}`;
  return {
    name: skill.name,
    path: skill.path ?? null,
    visibleText,
    start,
    end: start + visibleText.length,
  };
}

/** Retain identities only when the selected chip itself survived the edit. */
export function reconcileComposerSkills(
  previousText: string,
  nextText: string,
  selections: ReadonlyArray<ComposerSkillSelection>,
): ComposerSkillSelection[] {
  let prefix = 0;
  while (
    prefix < previousText.length &&
    prefix < nextText.length &&
    previousText[prefix] === nextText[prefix]
  )
    prefix += 1;
  let suffix = 0;
  while (
    suffix < previousText.length - prefix &&
    suffix < nextText.length - prefix &&
    previousText[previousText.length - 1 - suffix] === nextText[nextText.length - 1 - suffix]
  )
    suffix += 1;
  const previousChangedEnd = previousText.length - suffix;
  const delta = nextText.length - previousText.length;
  return selections.flatMap((selection) => {
    let next = selection;
    if (selection.end <= prefix) {
      next = selection;
    } else if (selection.start >= previousChangedEnd) {
      next = { ...selection, start: selection.start + delta, end: selection.end + delta };
    } else {
      return [];
    }
    return nextText.slice(next.start, next.end) === next.visibleText ? [next] : [];
  });
}

export function encodeComposerSkills(
  finalText: string,
  sourceText: string,
  selections: ReadonlyArray<ComposerSkillSelection>,
): PromptSkillSelection[] {
  const sourceStart = finalText.indexOf(sourceText);
  if (sourceStart < 0) return [];
  const encoder = new TextEncoder();
  return selections.flatMap((selection) => {
    if (sourceText.slice(selection.start, selection.end) !== selection.visibleText) return [];
    const start = sourceStart + selection.start;
    const end = sourceStart + selection.end;
    return [
      {
        name: selection.name,
        path: selection.path,
        visibleText: selection.visibleText,
        textRange: {
          start: encoder.encode(finalText.slice(0, start)).length,
          end: encoder.encode(finalText.slice(0, end)).length,
        },
      },
    ];
  });
}
