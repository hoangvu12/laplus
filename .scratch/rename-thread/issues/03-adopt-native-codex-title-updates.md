# 03 — Adopt native Codex title updates

**What to build:** When Codex publishes `thread/name/updated` for the conversation
Laplus is driving, the provider's non-empty name becomes the Laplus thread title
through the same metadata event and durable projection used by every other title
source. This gives Codex the provider-native parity OpenCode already has.

**Blocked by:** None — can start immediately.

**Status:** ready-for-human

- [x] A real-shaped Codex `thread/name/updated` notification for the owned thread is accepted.
- [x] Its non-empty name is published through the normal thread and shell/sidebar title updates.
- [x] Another connected client, a fresh subscriber, and a restarted server observe the Codex title.
- [x] Blank names are ignored and cannot replace a usable title.
- [x] Notifications for another provider thread cannot rename the active Laplus thread.
- [x] Existing OpenCode native title ingestion continues to pass unchanged.
- [x] The Codex protocol fixture and socket test prove behavior through provider input and public subscriptions rather than driver internals.
