# 02 — Register owned OpenCode through the MCP platform

Status: ready-for-human

Implements `.scratch/opencode-driver/issues/19-register-owned-opencode-mcp.md`.

- [x] Register before recovery and the first prompt
- [x] Require the named status to be `connected`
- [x] Release the MCP session on every owned cleanup path
- [x] Never mutate external OpenCode infrastructure
- [x] Cover ordering and lifetime through the socket seam
