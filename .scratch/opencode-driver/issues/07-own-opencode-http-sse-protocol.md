# 07 — Own the narrow OpenCode HTTP/SSE protocol

**What to build:** A narrow, handwritten OpenCode client capable of the HTTP
operations and directory-scoped SSE subscription required by the accepted
specification. Captured fixtures pin the wire, structured errors are classified,
and compatible unknown events remain observable without ending the stream.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] Redacted fixtures cover health, inventory, session operations, prompting,
      abort, revert, replies, SSE framing and structured errors used by the spec
- [x] The client centralizes base URL, directory binding, optional Basic auth,
      JSON handling and HTTP status classification
- [x] Structured missing-session responses are distinguishable from transport,
      authentication and other server failures
- [x] SSE records decode across chunk boundaries, multiline data and heartbeat
      traffic and can be cancelled promptly
- [x] Valid unknown event kinds are retained and counted without becoming fatal
- [x] Malformed SSE or JSON is distinguishable from a well-formed unknown event
- [x] Golden and scripted-peer tests require no live OpenCode or network
