# 04 — Image attachments inform first-turn titles

**What to build:** A first turn containing images gives the configured title
generator those actual images, allowing an image-only request to receive a title
based on its visual subject instead of serialized upload metadata.

**Blocked by:** 02 — Claude receives chat attachments; 03 — Codex receives chat attachments.

**Status:** done

- [x] First-turn title generation accepts stored chat attachments beside its text context.
- [x] Claude title generation receives the same native base64 image representation established for Claude turns.
- [x] Codex title generation receives the same native structured image representation established for Codex turns.
- [x] OpenCode title generation receives file parts matching the existing OpenCode attachment behavior.
- [x] An image-only first turn can produce a title from its image content.
- [x] Raw upload JSON and inline data URLs are not substituted into title prompt text.
- [x] Title-generation failure remains isolated from the successful agent turn and preserves existing compare-before-write behavior.
- [x] The existing first-turn title harness asserts provider-facing image input without invoking a real provider.
