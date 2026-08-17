# 06 — Drive provider-complete attachments in Laplus

**What to build:** Verify the completed attachment experience in a running
Laplus window across every available runtime driver, fixing integration defects
so the image shown by the interface is the image delivered to the provider and
retained by the conversation.

**Blocked by:** 04 — Image attachments inform first-turn titles; 05 — Queued and retried attachments stay with their prompts.

**Status:** done

- [x] The running application can paste and send a text-plus-image turn through each available Claude, Codex, and owned/local OpenCode configuration.
- [x] The running application can send an image-only first turn and receive a meaningful generated title when title generation is configured.
- [x] Multiple attached images appear on the sent user message and reach the selected provider in order.
- [x] Reloading or reopening the conversation retains attachment metadata and a working preview.
- [x] A visibly invalid or oversized attachment produces an actionable refusal instead of a successful text-only turn.
- [x] External OpenCode retains T3's local file-URL/shared-filesystem behavior without a new warning or upload mechanism.
- [x] Any integration defects discovered while driving the application are covered by the highest practical automated regression test.
- [x] Focused formatting, lint, type, and test checks pass for affected scopes, and all development servers or watchers are stopped afterward.

## Verification

The built web bundle was driven in headless Chromium against isolated Laplus
servers and deterministic Claude, Codex app-server, and owned OpenCode
stand-ins. For every driver, the real composer pasted two ordered images, sent
them with text, rendered the provider reply, and resolved both persisted
previews after reload. The provider-side captures retained text-image order and
used each driver's native image representation. The same drive visibly refused
an image over 10 MiB and an unsupported SVG without dispatching a successful
text-only turn.

A fresh Codex-configured profile also sent an image-only first turn through the
running UI and visibly adopted the generated title `Screenshot subject`; its
provider capture contained the native image data URL. The existing socket
suites additionally assert durable user-message metadata, reloadable assets,
queue/retry ownership, title-generation inputs, and exact Claude, Codex, and
OpenCode provider requests. External OpenCode's existing local `file:` URL
behavior remains unchanged. Real provider accounts were not configured in this
environment, so no paid external service was contacted.
