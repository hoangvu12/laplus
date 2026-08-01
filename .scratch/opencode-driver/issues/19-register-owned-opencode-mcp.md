# 19 — Register owned OpenCode sessions with MCP

**What to build:** Once Laplus's generic MCP platform surface exists, each owned
OpenCode session receives its authenticated per-thread endpoint through
OpenCode's MCP operation. External OpenCode infrastructure is never mutated.

**Blocked by:** 09 — Run the first owned OpenCode text turn; the separately
specified MCP platform effort required by ADR-0030.

**Status:** ready-for-agent

- [ ] An owned OpenCode session registers the generic per-thread MCP endpoint
      before the first prompt can use it
- [ ] Registration carries the scoped authentication material produced by the
      MCP platform and never exposes it in snapshots or logs
- [ ] MCP session lifetime follows the conversation and is released during
      every owned-session cleanup path
- [ ] An external OpenCode instance receives no automatic MCP registration or
      configuration mutation
- [ ] Registration failure becomes an actionable session startup failure rather
      than an apparently working agent with missing tools
- [ ] Socket tests use a fake MCP platform and scripted OpenCode peer to assert
      registration ordering, authentication handling and cleanup
- [ ] The OpenCode driver consumes the generic MCP interface and does not embed
      a private MCP server or preview-specific routing
