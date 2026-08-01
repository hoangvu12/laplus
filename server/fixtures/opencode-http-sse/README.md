# OpenCode HTTP/SSE

Redacted conformance evidence for the narrow OpenCode operations Laplus owns.
`operations.json` covers each request/response and structured-error family;
`events.sse` pins comments, multiline data, known events, and compatible drift.
The integration test deliberately feeds the SSE file in short chunks.

Provenance: captured from the disposable protocol prototype against OpenCode
1.18.10, then redacted by replacing credentials, identifiers, paths, prompts,
and failure detail. Route and schema names were cross-checked against that
release's pinned `packages/sdk/openapi.json`; upgrading OpenCode requires
reviewing these files as a wire diff.
