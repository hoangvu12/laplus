# 05 — Queued and retried attachments stay with their prompts

**What to build:** Images remain paired with the messages that introduced them
when turns queue, merge for delivery, survive interruption, or are retried, so a
provider never receives an image on the wrong prompt or loses it silently.

**Blocked by:** 02 — Claude receives chat attachments; 03 — Codex receives chat attachments.

**Status:** done

- [x] A prompt queued behind active work retains its own text and attachments until dispatch.
- [x] Several queued prompts preserve message order and the attachment order within each message.
- [x] When queued prompts are merged for provider delivery, all images remain ordered with their originating prompt text.
- [x] Interrupting active work does not move queued attachments onto the interrupted turn or discard them from the retained queue.
- [x] A retryable OpenCode delivery failure retains the original attachment metadata and sends the same file parts when retried.
- [x] Durable user messages continue to show each message's own attachments regardless of provider-side batching.
- [x] Existing session and OpenCode queue harnesses demonstrate queueing, merge, interruption recovery, failed delivery, and retry without a real provider account.
