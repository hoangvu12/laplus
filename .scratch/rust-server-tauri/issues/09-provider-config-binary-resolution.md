# 09 — Provider configuration and agent binary resolution

**What to build:** The server finds the developer's installed Claude Code binary
and the UI shows a configured, ready provider instance. When the binary is missing,
the developer gets a diagnostic naming exactly what was looked for and where it
looked — enough to fix it without opening a log file.

Resolution order is: an explicitly configured path, then a lookup on PATH, then a
clear failure. On Windows the executable is a native binary, so the upstream
server's npm shim resolution logic is deliberately not ported — it is dead weight
here.

No agent is spawned by this ticket. It establishes that the driver exists, is
locatable, and is configured.

**Blocked by:** 03 (Socket endpoint, local handshake, and the configuration
method).

**Status:** ready-for-agent

- [ ] The agent binary is located on PATH without configuration on a machine where
      it is installed
- [ ] An explicitly configured path takes precedence over the PATH lookup
- [ ] A missing binary produces a diagnostic naming both the configured path (if
      any) and the fact that PATH was searched
- [ ] A configured path that exists but is not executable is reported distinctly
      from one that does not exist
- [ ] The UI shows the provider instance as configured and ready
- [ ] The resolved binary's version is reported, so a pinned known-good version can
      be confirmed
- [ ] Tests cover the resolution order and each failure mode through the socket
      boundary, without requiring a real agent binary to be installed
