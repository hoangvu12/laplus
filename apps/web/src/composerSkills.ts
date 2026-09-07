import type { PromptSkillSelection, ServerProviderSkill } from "@t3tools/contracts";

export interface ComposerSkillSelection {
  name: string;
  path: string | null;
  visibleText: string;
  /** UTF-16 offsets while the text is edited in the browser. */
  start: number;
  end: number;
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
