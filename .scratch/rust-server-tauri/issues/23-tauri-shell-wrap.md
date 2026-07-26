# 23 — Tauri shell

**What to build:** lightcode becomes a desktop application. It launches as a native
window using the operating system's existing webview, starts its server internally,
and presents the full working app — projects, files, agent conversations,
terminals, git. No browser, no separate server to start, no Node runtime anywhere
on the machine.

The webview is provisioned by download bootstrapper so it contributes essentially
nothing to the artifact.

This is blocked on the agent core rather than merely on the transport, so that what
gets wrapped is an app worth demonstrating. If webview or shell problems are a
worry, it can be pulled earlier — the only hard requirement is a running server.

**Blocked by:** 10 (One complete agent turn, streamed).

**Status:** ready-for-agent

- [ ] The application launches as a desktop window and reaches an interactive state
      quickly
- [ ] The server starts inside the application; nothing needs to be launched
      separately
- [ ] The UI is served from the embedded application rather than a development
      server
- [ ] A full agent conversation works end to end inside the window
- [ ] Terminals and git views work inside the window
- [ ] The custom titlebar drag regions behave correctly
- [ ] Closing the window shuts down the server and reaps all child processes —
      agent subprocesses and terminals alike
- [ ] No Node runtime is present in the built application
- [ ] Application state is stored in the appropriate per-user location
