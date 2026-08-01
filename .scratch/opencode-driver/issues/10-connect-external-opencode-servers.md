# 10 — Connect to operator-owned OpenCode servers

**What to build:** The text-turn tracer bullet also works against a configured
operator-owned OpenCode HTTP or HTTPS endpoint, with optional Basic
authentication and no attempt to start, stop or secure that endpoint.

**Blocked by:** 08 — Discover and configure OpenCode instances; 09 — Run the
first owned OpenCode text turn.

**Status:** ready-for-agent

- [ ] An external instance opens a directory-bound session and completes a text
      turn through the same driver behavior as an owned instance
- [ ] An optional password is sent using OpenCode Basic authentication and is
      never exposed in snapshots, logs or errors
- [ ] HTTP and HTTPS URLs are accepted without a Laplus loopback restriction
- [ ] Authentication, reachability and structured server failures remain
      distinct and actionable
- [ ] Stopping the Laplus session cancels its subscription but never performs an
      endpoint-lifetime action
- [ ] Socket tests prove authenticated and unauthenticated flows and the absence
      of ownership actions
