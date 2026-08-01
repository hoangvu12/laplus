# ADR-0044 — Provider maintenance is generic and OpenCode matches T3

Date: 2026-08-01
Status: Accepted

Laplus will implement the contract's generic provider-maintenance surface and
register OpenCode's T3-compatible native, npm, pnpm, Bun, Vite+ and Homebrew
update strategies through it. Commands are selected from the resolved
installation, run only on explicit request, serialized by instance and package
manager, and followed by a provider refresh that reports whether the detected
version changed. Matching T3 exactly, an external OpenCode instance may still
advertise maintenance derived from its configured local `binaryPath`; that
command may update a local CLI without changing the external server, and the
post-update snapshot is the authoritative result rather than an assumption that
the command changed the endpoint.
