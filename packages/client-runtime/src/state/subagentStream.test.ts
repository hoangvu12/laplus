import { describe, expect, it } from "vite-plus/test";

import {
  applySubagentStreamItem,
  EMPTY_SUBAGENT_STREAM_STATE,
  type SubagentStreamState,
} from "./subagentStream.ts";

const stream = (
  overrides: Partial<NonNullable<SubagentStreamState["stream"]>> = {},
): NonNullable<SubagentStreamState["stream"]> =>
  ({
    childId: "call_task_1",
    parentChildId: null,
    name: "explore",
    assignment: "Count the files",
    state: "working",
    outcome: null,
    entryCount: 0,
    createdAt: "2026-08-17T00:00:00.000Z",
    updatedAt: "2026-08-17T00:00:00.000Z",
    ...overrides,
  }) as NonNullable<SubagentStreamState["stream"]>;

const entry = (id: string, sequence: number, text: string) =>
  ({
    id,
    sequence,
    kind: "message",
    payload: { text },
    createdAt: "2026-08-17T00:00:01.000Z",
  }) as const;

const fold = (
  items: ReadonlyArray<Parameters<typeof applySubagentStreamItem>[1]>,
  from: SubagentStreamState = EMPTY_SUBAGENT_STREAM_STATE,
) => items.reduce(applySubagentStreamItem, from);

const texts = (state: SubagentStreamState) =>
  state.entries.map((held) => (held.payload as { text?: string }).text ?? "");

describe("a subagent work stream", () => {
  it("opens with the work recorded so far", () => {
    const state = fold([
      {
        kind: "snapshot",
        snapshot: {
          stream: stream({ entryCount: 1 }),
          entries: [entry("a", 1, "looking through the directory")],
        },
      },
    ]);
    expect(state.stream?.childId).toBe("call_task_1");
    expect(texts(state)).toEqual(["looking through the directory"]);
  });

  /**
   * The replay/live boundary. A subscription resynchronises by describing the
   * stream again, so an entry a client already holds arrives a second time by
   * construction — and a client that appended would show the child saying the
   * same thing twice, with nothing later to correct it.
   */
  it("upserts an entry it has already seen rather than repeating it", () => {
    const opening = fold([
      {
        kind: "snapshot",
        snapshot: {
          stream: stream({ entryCount: 1 }),
          entries: [entry("a", 1, "looking")],
        },
      },
    ]);
    const continued = fold(
      [
        { kind: "entry-upserted", entry: entry("a", 1, "looking") },
        { kind: "entry-upserted", entry: entry("a", 1, "looking through the directory") },
        { kind: "entry-upserted", entry: entry("b", 2, "eleven so far") },
      ],
      opening,
    );
    expect(texts(continued)).toEqual(["looking through the directory", "eleven so far"]);
  });

  /** A revision must not move a child's earlier work to the end of its history. */
  it("orders by sequence rather than by arrival", () => {
    const state = fold([
      { kind: "entry-upserted", entry: entry("b", 2, "second") },
      { kind: "entry-upserted", entry: entry("a", 1, "first") },
      { kind: "entry-upserted", entry: entry("a", 1, "first, revised") },
    ]);
    expect(texts(state)).toEqual(["first, revised", "second"]);
  });

  /**
   * The conclusion settles the stream and stays in it. A client reads the
   * outcome off the stream rather than off the last entry, because a provider
   * may go on narrating after it has reported.
   */
  it("keeps the terminal outcome on the stream and in the entries", () => {
    const state = fold([
      {
        kind: "entry-upserted",
        entry: {
          id: "call_task_1:k:outcome",
          sequence: 3,
          kind: "outcome",
          payload: { kind: "completed", text: "eleven files" },
          createdAt: "2026-08-17T00:00:03.000Z",
        },
      },
      {
        kind: "stream-updated",
        stream: stream({
          state: "completed",
          outcome: { kind: "completed", text: "eleven files" },
          entryCount: 3,
        }),
      },
    ]);
    expect(state.stream?.state).toBe("completed");
    expect(state.stream?.outcome).toEqual({ kind: "completed", text: "eleven files" });
    expect(state.entries.at(-1)?.kind).toBe("outcome");
  });

  /** A child that has done nothing yet is not a child that has not loaded. */
  it("distinguishes an empty stream from an unloaded one", () => {
    expect(EMPTY_SUBAGENT_STREAM_STATE.stream).toBeNull();
    const opened = fold([
      { kind: "snapshot", snapshot: { stream: stream({ state: "pending" }), entries: [] } },
    ]);
    expect(opened.stream?.state).toBe("pending");
    expect(opened.entries).toEqual([]);
  });
});
