# 03 — Manual pinned order that converges

**What to build:** A developer drags pinned rows in Sidebar V2 into a personal
priority order. The row stays where it was dropped while persistence catches up,
the order survives reload, and clients connected to the same environments
converge even when the neighboring pins belong to different servers.

The implementation follows T3's fractional-index design. A normal move assigns
one lexicographic base-26 key to the moved thread on its own server. A keyless or
corrupt boundary triggers a one-time evenly spread rewrite of the canonical
section. Keyed pins precede legacy keyless pins; all ties include environment id
because thread ids are not globally unique.

**Blocked by:** 02.

**Status:** done

- [x] Shared client logic ports the upstream fractional midpoint, spread,
      reorder planning, and canonical pinned-sort behavior, including validation
      and scoped deterministic tie-breaking.
- [x] Sidebar V2 uses upstream-compatible sortable drag behavior: a small
      activation distance preserves clicks, movement is vertical and constrained
      to the scroll container, and only reorder-capable rows participate.
- [x] An ordinary drop writes only the moved thread; section materialization
      writes only the assignments the planner determines are necessary.
- [x] Optimistic order holds until every assignment lands or canonical order
      already matches, and releases on membership change, a foreign winning
      write, or failure rather than displaying a stale or half-written order.
- [x] New pins, unpins, snooze/wake, project/filter changes, reconnects, and
      mixed-version environments follow the selected T3 snapshot's canonical
      behavior during and after a drag.
- [x] Failures return visibly to canonical order and surface an actionable toast;
      concurrent/equal-key outcomes render deterministically on every client.
- [x] Pure tests cover deep insertion churn, invalid and adjacent bounds,
      keyless materialization, new-pin-at-top, mixed environments/capabilities,
      and scoped identity collisions.
- [x] UI/state tests cover successful single- and multi-write confirmation,
      failure, membership changes, concurrent foreign writes, snooze/wake, and
      filtered reorder behavior.
