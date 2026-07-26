# 22 — Settings and keybindings

**What to build:** A developer configures the app once and it stays configured.
Settings and keybindings persist across restarts, the Claude Code provider instance
can be configured so model and options match how they work, and changes take effect
immediately rather than requiring a restart.

**Blocked by:** 05 (Project registry), 04 (First streaming subscription).

**Status:** ready-for-agent

- [ ] Settings can be read and updated from the UI
- [ ] Settings survive a restart
- [ ] The Claude Code provider instance can be configured, including model
      selection
- [ ] Keybindings can be added, changed and removed
- [ ] A configuration change reaches the UI without a restart
- [ ] Invalid settings are rejected with a message, leaving the previous values
      intact
- [ ] A corrupt or unreadable settings store falls back to defaults with a warning
      rather than failing to start
- [ ] A newly configured model is used by the next agent session
- [ ] Tests cover update, persistence across restart, live propagation, and
      rejection of invalid input through the socket boundary
