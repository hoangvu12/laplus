import type {
  OrchestrationSubagentEntry,
  OrchestrationSubagentStream,
  OrchestrationSubagentStreamItem,
} from "@t3tools/contracts";

/**
 * One delegated child's work stream, as a client holds it.
 *
 * `stream` is `null` until the subscription's opening snapshot arrives, which is
 * what distinguishes "loading" from "a child that has done nothing yet" — the
 * second is a stream with no entries and a state of its own.
 */
export interface SubagentStreamState {
  readonly stream: OrchestrationSubagentStream | null;
  readonly entries: ReadonlyArray<OrchestrationSubagentEntry>;
}

export const EMPTY_SUBAGENT_STREAM_STATE: SubagentStreamState = {
  stream: null,
  entries: [],
};

/**
 * Fold one item of a subagent work stream.
 *
 * The whole rule is **upsert by `id`, order by `sequence`**, and it is what makes
 * the subscription's replay and its live continuation meet without a cursor:
 *
 * - a snapshot is the stream as the server holds it, so it replaces;
 * - an entry is upserted, so one seen twice — which happens by construction when
 *   a subscription resynchronises — lands on the state already held rather than
 *   appearing twice;
 * - a provider that revises a part it already sent moves that entry rather than
 *   appending a near-duplicate of the same prose.
 *
 * Ordering is by `sequence` rather than arrival, so a revision cannot move an
 * entry to the end of the child's history.
 */
export function applySubagentStreamItem(
  state: SubagentStreamState,
  item: OrchestrationSubagentStreamItem,
): SubagentStreamState {
  switch (item.kind) {
    case "snapshot":
      return {
        stream: item.snapshot.stream,
        entries: [...item.snapshot.entries].sort((left, right) => left.sequence - right.sequence),
      };
    case "stream-updated":
      return { ...state, stream: item.stream };
    case "entry-upserted": {
      const held = state.entries.findIndex((entry) => entry.id === item.entry.id);
      const entries =
        held < 0
          ? [...state.entries, item.entry]
          : state.entries.map((entry, index) => (index === held ? item.entry : entry));
      return {
        ...state,
        entries: entries.sort((left, right) => left.sequence - right.sequence),
      };
    }
  }
}
