export interface PinnedThreadOrderInput<E = string, I = string> {
  readonly environmentId: E;
  readonly id: I;
  readonly pinOrderKey?: string | null | undefined;
  readonly pinnedAt?: string | null | undefined;
  readonly createdAt: string;
  readonly reorderCapable?: boolean;
}

export interface PinOrderAssignment<E = string, I = string> {
  readonly environmentId: E;
  readonly threadId: I;
  readonly pinOrderKey: string;
}

export function capableDestinationIndex<T>(
  displayedPins: readonly T[],
  activeId: string,
  overId: string,
  idOf: (thread: T) => string,
  isCapable: (thread: T) => boolean,
): number {
  const activeIndex = displayedPins.findIndex((thread) => idOf(thread) === activeId);
  const overIndex = displayedPins.findIndex((thread) => idOf(thread) === overId);
  if (activeIndex < 0 || overIndex < 0 || !isCapable(displayedPins[activeIndex]!)) {
    return -1;
  }
  const capable = displayedPins.filter(isCapable);
  if (isCapable(displayedPins[overIndex]!)) {
    return capable.findIndex((thread) => idOf(thread) === overId);
  }
  const capableBeforeTarget = displayedPins.slice(0, overIndex).filter(isCapable).length;
  return activeIndex < overIndex ? capableBeforeTarget : Math.max(0, capableBeforeTarget - 1);
}

export function shouldReleaseOptimisticPinOrder(input: {
  readonly keysAtDrop: ReadonlyMap<string, string | null>;
  readonly expected: ReadonlyMap<string, string>;
  readonly current: ReadonlyMap<string, string | null>;
  readonly failed?: boolean;
}): boolean {
  if (input.failed) return true;
  if (input.current.size !== input.keysAtDrop.size) return true;
  let allExpected = true;
  for (const [id, before] of input.keysAtDrop) {
    if (!input.current.has(id)) return true;
    const current = input.current.get(id)!;
    const expected = input.expected.get(id);
    if (expected === undefined) {
      if (current !== before) return true;
    } else if (current !== expected) {
      allExpected = false;
      if (current !== before) return true;
    }
  }
  return allExpected;
}

const VALID_KEY = /^[a-z]*[b-z]$/;
const BASE = 26;

const scopedId = (thread: Pick<PinnedThreadOrderInput<unknown, unknown>, "environmentId" | "id">) =>
  `${String(thread.environmentId)}:${String(thread.id)}`;

export function sortPinnedThreads<
  E extends string,
  I extends string,
  T extends PinnedThreadOrderInput<E, I>,
>(threads: readonly T[]): T[] {
  return threads.toSorted((left, right) => {
    const leftKey = left.pinOrderKey && VALID_KEY.test(left.pinOrderKey) ? left.pinOrderKey : null;
    const rightKey =
      right.pinOrderKey && VALID_KEY.test(right.pinOrderKey) ? right.pinOrderKey : null;
    if (leftKey !== null && rightKey !== null) {
      return leftKey.localeCompare(rightKey) || scopedId(left).localeCompare(scopedId(right));
    }
    if (leftKey !== null) return -1;
    if (rightKey !== null) return 1;
    const created = Date.parse(right.createdAt) - Date.parse(left.createdAt);
    return (Number.isNaN(created) ? 0 : created) || scopedId(left).localeCompare(scopedId(right));
  });
}

export function fractionalKeyBetween(lower: string | null, upper: string | null): string | null {
  if ((lower !== null && !VALID_KEY.test(lower)) || (upper !== null && !VALID_KEY.test(upper))) {
    return null;
  }
  if (lower !== null && upper !== null && lower >= upper) return null;
  if (upper === null) return lower === null ? "n" : `${lower}n`;
  const before = (bound: string): string => {
    const head = bound.charCodeAt(0) - 97;
    if (head > 1) return String.fromCharCode(97 + Math.floor(head / 2));
    if (head === 1) return "an";
    return `a${before(bound.slice(1))}`;
  };
  if (lower === null) return before(upper);
  let common = 0;
  while (lower[common] === upper[common]) common += 1;
  if (common === lower.length) return `${lower}${before(upper.slice(common))}`;
  const low = lower.charCodeAt(common) - 97;
  const high = upper.charCodeAt(common) - 97;
  if (high - low > 1) {
    return `${lower.slice(0, common)}${String.fromCharCode(97 + Math.floor((low + high) / 2))}`;
  }
  return `${lower}n`;
}

export function spreadFractionalKeys(count: number): string[] {
  if (count <= 0) return [];
  let width = 1;
  while (BASE ** (width - 1) * 25 <= count + 1) width += 1;
  const validCount = BASE ** (width - 1) * 25;
  return Array.from({ length: count }, (_, index) => {
    const ordinal = Math.floor(((index + 1) * (validCount + 1)) / (count + 1));
    let prefix = Math.floor((ordinal - 1) / 25);
    let key = String.fromCharCode(98 + ((ordinal - 1) % 25));
    for (let place = 1; place < width; place += 1) {
      key = String.fromCharCode(97 + (prefix % BASE)) + key;
      prefix = Math.floor(prefix / BASE);
    }
    return key;
  });
}

export function planPinnedReorder<
  E extends string,
  I extends string,
  T extends PinnedThreadOrderInput<E, I>,
>(
  canonicalPins: readonly T[],
  moved: Pick<T, "environmentId" | "id">,
  destinationIndex: number,
): PinOrderAssignment<E, I>[] {
  const allCanonical = sortPinnedThreads(canonicalPins);
  const canonical = allCanonical.filter((thread) => thread.reorderCapable !== false);
  const source = canonical.find((thread) => scopedId(thread) === scopedId(moved));
  if (!source) return [];
  const withoutMoved = canonical.filter((thread) => thread !== source);
  const target = Math.max(0, Math.min(destinationIndex, withoutMoved.length));
  const reorderedCapable = withoutMoved.toSpliced(target, 0, source);
  let capableIndex = 0;
  const reordered = allCanonical.map((thread) =>
    thread.reorderCapable === false ? thread : reorderedCapable[capableIndex++]!,
  );
  const sourceIndex = reordered.indexOf(source);
  const lowerNeighbor = reordered[sourceIndex - 1];
  const upperNeighbor = reordered[sourceIndex + 1];
  const lower = lowerNeighbor?.pinOrderKey ?? null;
  const upper = upperNeighbor?.pinOrderKey ?? null;
  const hasKeylessBoundary =
    (lowerNeighbor !== undefined && (lower == null || !VALID_KEY.test(lower))) ||
    (upperNeighbor !== undefined && (upper == null || !VALID_KEY.test(upper)));
  const key = hasKeylessBoundary ? null : fractionalKeyBetween(lower, upper);
  if (key !== null) {
    const assignment = {
      environmentId: source.environmentId,
      threadId: source.id,
      pinOrderKey: key,
    };
    const persisted = sortPinnedThreads(
      allCanonical.map((thread) => (thread === source ? { ...thread, pinOrderKey: key } : thread)),
    );
    if (persisted.every((thread, index) => scopedId(thread) === scopedId(reordered[index]!))) {
      return [assignment];
    }
  }

  if (allCanonical.some((thread) => thread.reorderCapable === false)) {
    const assignments: PinOrderAssignment<E, I>[] = [];
    for (let start = 0; start < reordered.length; ) {
      if (scopedId(reordered[start]!) === scopedId(allCanonical[start]!)) {
        start += 1;
        continue;
      }
      let end = start + 1;
      while (end < reordered.length && scopedId(reordered[end]!) !== scopedId(allCanonical[end]!))
        end += 1;
      let lowerBound = reordered[start - 1]?.pinOrderKey ?? null;
      const upperBound = reordered[end]?.pinOrderKey ?? null;
      if (lowerBound !== null && !VALID_KEY.test(lowerBound)) return [];
      if (upperBound !== null && !VALID_KEY.test(upperBound)) return [];
      for (let index = start; index < end; index += 1) {
        const capable = reordered[index]!;
        if (capable.reorderCapable === false) return [];
        const nextKey = fractionalKeyBetween(lowerBound, upperBound);
        if (nextKey === null) return [];
        if (capable.pinOrderKey !== nextKey) {
          assignments.push({
            environmentId: capable.environmentId,
            threadId: capable.id,
            pinOrderKey: nextKey,
          });
        }
        lowerBound = nextKey;
      }
      start = end;
    }
    return assignments;
  }

  const keys = spreadFractionalKeys(reordered.length);
  return reordered.flatMap((thread, index) =>
    thread.pinOrderKey === keys[index]!
      ? []
      : [{ environmentId: thread.environmentId, threadId: thread.id, pinOrderKey: keys[index]! }],
  );
}

export function keyBeforeAllPinned(threads: readonly PinnedThreadOrderInput[]): string {
  const first = sortPinnedThreads(threads).find(
    (thread) => thread.pinOrderKey != null && VALID_KEY.test(thread.pinOrderKey),
  )?.pinOrderKey;
  return fractionalKeyBetween(null, first ?? null) ?? spreadFractionalKeys(threads.length + 1)[0]!;
}
