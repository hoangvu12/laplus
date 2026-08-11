export interface ThreadActionPolicyInput {
  readonly pinningSupported: boolean;
  readonly pinned: boolean;
  readonly settled?: boolean;
  readonly snoozed?: boolean;
  readonly settlementSupported?: boolean;
  readonly snoozeSupported?: boolean;
  readonly archived?: boolean;
  readonly canCopy?: boolean;
  readonly allowArchive?: boolean;
  readonly titleRegenerationSupported?: boolean;
  readonly titleRegenerationPending?: boolean;
}

export interface ThreadActionPolicy {
  readonly pinAction: { readonly id: "pin" | "unpin"; readonly label: string } | null;
  readonly rename: boolean;
  readonly regenerateTitle: {
    readonly id: "regenerate-title";
    readonly label: string;
    readonly disabled: boolean;
  } | null;
  readonly copy: boolean;
  readonly lifecycleAction: {
    readonly id: "settle" | "unsettle" | "snooze" | "unsnooze";
    readonly label: string;
  } | null;
  readonly lifecycleActions: ReadonlyArray<{
    readonly id: "settle" | "unsettle" | "snooze" | "unsnooze";
    readonly label: string;
  }>;
  readonly destructiveAction: { readonly id: "archive" | "delete"; readonly label: string };
}

/** Shared presentation policy consumed by sidebar and header action menus. */
export function threadActionPolicy(input: ThreadActionPolicyInput): ThreadActionPolicy {
  const pinAction = !input.pinningSupported
    ? null
    : input.pinned
      ? { id: "unpin" as const, label: "Unpin thread" }
      : { id: "pin" as const, label: "Pin thread" };
  const lifecycleActions = [
    ...(input.settlementSupported
      ? [
          input.settled
            ? { id: "unsettle" as const, label: "Un-settle thread" }
            : { id: "settle" as const, label: "Settle thread" },
        ]
      : []),
    ...(input.snoozeSupported
      ? [
          input.snoozed
            ? { id: "unsnooze" as const, label: "Wake thread" }
            : { id: "snooze" as const, label: "Snooze" },
        ]
      : []),
  ];
  const lifecycleAction = lifecycleActions[0] ?? null;
  return {
    pinAction,
    rename: !input.archived,
    regenerateTitle:
      input.titleRegenerationSupported && !input.archived
        ? {
            id: "regenerate-title",
            label: input.titleRegenerationPending ? "Regenerating…" : "Regenerate title",
            disabled: input.titleRegenerationPending === true,
          }
        : null,
    copy: input.canCopy ?? true,
    lifecycleAction,
    lifecycleActions,
    destructiveAction:
      input.archived || input.allowArchive === false
        ? { id: "delete", label: "Delete" }
        : { id: "archive", label: "Archive thread" },
  };
}
