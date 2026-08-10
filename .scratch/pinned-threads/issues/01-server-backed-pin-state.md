# 01 — Server-backed pin state through the real client

**What to build:** A developer can pin and unpin a thread through Laplus's
normal orchestration path, and that choice survives restart. This is the tracer
bullet for the feature: shared schemas and capabilities, client operations,
Rust command handling and invariants, event folding, SQLite projection, and a
thin public read/write seam. It does not yet build the final pinned shelf.

The selected T3 snapshot defines the lifecycle interaction. Pinning clears a
settled state; explicitly settling a pinned thread clears both the pin and its
order key; snoozing preserves them; and unpinning clears the order key. Historic
shells without the new fields remain valid unpinned threads.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] Contracts declare optional `pinnedAt` and `pinOrderKey` shell fields,
      pin/unpin/reorder commands and events, and separate pinning/reorder
      capabilities without adding runtime logic to the schema package.
- [x] Client-runtime operations address the thread's scoped owning environment
      and expose optimistic pin, unpin, and reorder command seams.
- [x] The Rust server advertises the capabilities it implements and decodes,
      validates, acknowledges, and publishes the new commands through the
      existing orchestration path.
- [x] The thread fold enforces T3's pin/settle/snooze invariants and idempotency,
      including rejecting reorder for an unpinned thread.
- [x] A forward SQLite migration stores `pinned_at` and `pin_order_key`; live
      shells, applicable archived snapshots, replay, and restart rehydration
      agree on both values.
- [x] A fresh pin can carry its initial order key in the same command, avoiding
      a second round trip.
- [x] Focused schema, fold, projection, migration, and real-WebSocket tests pin,
      unpin, settle, snooze, reconnect, and prove the projected/event state.
