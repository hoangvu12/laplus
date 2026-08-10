export type ThreadShelfSection = "pinned" | "active" | "snoozed" | "settled";

export function resolveThreadShelfSection(input: {
  readonly pinned: boolean;
  readonly snoozed: boolean;
  readonly settled: boolean;
}): ThreadShelfSection {
  if (input.snoozed) return "snoozed";
  if (input.pinned) return "pinned";
  if (input.settled) return "settled";
  return "active";
}
