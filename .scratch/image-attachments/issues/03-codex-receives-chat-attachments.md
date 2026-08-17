# 03 — Codex receives chat attachments

**What to build:** A developer can send text with one or more images, or images
without authored text, and Codex receives the complete request as ordered native
turn inputs rather than a single text-only input.

**Blocked by:** 01 — A stored image survives an OpenCode conversation.

**Status:** done

- [x] Codex turn start accepts an ordered list of structured inputs instead of assuming one text field.
- [x] Prompt text becomes a text input and every resolved image becomes an image input containing a correctly typed data URL.
- [x] Image-only turns reach Codex with image input and the established image-only bootstrap text behavior.
- [x] Multiple images retain their composer order.
- [x] An invalid stored identity or unreadable stored file refuses provider dispatch visibly rather than sending an incomplete text-only turn.
- [x] The scripted app-server harness asserts the complete provider-facing turn-start input list.
- [x] A focused protocol golden test protects the expanded turn-start request vocabulary.
- [x] Existing text-only Codex turns, continuation, retuning, approval, interruption, and settlement behavior remain unchanged.
