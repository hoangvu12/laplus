# 02 — Claude receives chat attachments

**What to build:** A developer can send text with one or more images, or images
without authored text, and Claude receives the complete request as one native
streaming user message rather than a text-only prompt.

**Blocked by:** 01 — A stored image survives an OpenCode conversation.

**Status:** done

- [x] Claude receives each stored image as an ordered base64 image content block beside any text content.
- [x] PNG, JPEG, GIF, and WebP use the T3-compatible Claude media types and payload shape.
- [x] Image-only turns reach Claude with image content and the established image-only bootstrap text behavior.
- [x] Multiple images retain their composer order.
- [x] An unsupported attachment type, invalid stored identity, or unreadable stored file refuses provider dispatch visibly rather than sending an incomplete text-only turn.
- [x] The scripted Claude harness captures a normal streaming-input turn and asserts the exact provider-facing text and image blocks.
- [x] Existing text-only Claude turns, continuation, retuning, approval, interruption, and settlement behavior remain unchanged.
