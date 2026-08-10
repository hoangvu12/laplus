# 02 — Pinned shelf and shared thread actions

**What to build:** A developer can pin or unpin from either Sidebar V2 or the
open conversation's title menu. Visible pins from all projects and connected
environments render once as full rows above the active inbox. A pinned row has a
one-click Unpin action. This ticket establishes the complete non-draggable user
experience and one shared action policy; ticket 03 adds manual ordering.

T3's lifecycle partition is authoritative: snooze temporarily wins and hides a
pin while preserving it; wake restores it; visible pins do not auto-settle;
settle clears pin; and pinning a settled thread makes it active. The
compatibility `Sidebar` gets no duplicate pinned shelf.

**Blocked by:** 01.

**Status:** done

- [x] Sidebar V2 partitions visible threads into pinned, active, snoozed, and
      settled exactly once, with pins above the inbox and T3's divider, full-row
      treatment, pin indicator, and one-click Unpin affordance.
- [x] The pinned shelf is one merged list across visible projects and
      environments; project scope and search show the matching subsequence with
      the same open-thread and selection semantics as upstream.
- [x] A fresh or re-pinned thread receives an order key before the canonical
      minimum across every pin, including pins hidden by snooze, and appears at
      the top when visible.
- [x] One shared thread-action-menu policy owns capability, lifecycle, and copy,
      rename, pin/unpin, archive/delete decisions for Sidebar V2 and ChatHeader.
- [x] The chat title is the upstream-style action-menu entry point and pin/unpin
      dispatches against the correct scoped environment and thread.
- [x] Older servers expose no unsupported pin action; mixed environments still
      render all supported pin state deterministically.
- [x] The compatibility `Sidebar` remains functional and does not acquire a
      local or second canonical pin implementation.
- [x] Focused component and action tests cover both entry points, global
      partitioning, snooze/wake, settle/pin precedence, project/search scope,
      mixed capabilities, failure toasts, and accessible controls.
