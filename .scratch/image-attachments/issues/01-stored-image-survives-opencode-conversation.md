# 01 — A stored image survives an OpenCode conversation

**What to build:** A developer's image upload is validated and stored before the
turn is committed, remains attached to the durable user message, travels through
the existing OpenCode file-part path, and is still visible after the optimistic
client state is gone or the conversation is reloaded.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] PNG, JPEG, GIF, and WebP uploads produce safe stored chat attachments whose declared size agrees with their decoded bytes.
- [x] Empty images, malformed data URLs, unsupported MIME types, unsafe identities or paths, byte-count mismatches, and persistence failures reject the command before a user message or turn is committed.
- [x] A decoded image exactly at 10 MiB is accepted and an image one byte larger is rejected.
- [x] The durable user-message event and a fresh conversation snapshot contain attachment identity, name, MIME type, and byte size without retaining the inline data URL.
- [x] The attachment asset remains resolvable after a server restart and can render in a reloaded transcript.
- [x] Owned/local OpenCode still receives ordered file parts with the expected MIME type, filename, and local file URL.
- [x] OpenCode continues to omit an independently unresolved file reference, matching current T3 behavior.
- [x] Focused validation tests and an OpenCode socket-level test demonstrate the complete path without a real provider account.
