# 22 — Settings and keybindings

**What to build:** A developer configures the app once and it stays configured.
Settings and keybindings persist across restarts, the Claude Code provider instance
can be configured so model and options match how they work, and changes take effect
immediately rather than requiring a restart.

**Blocked by:** 05 (Project registry), 04 (First streaming subscription).

**Status:** done

- [x] Settings can be read and updated from the UI
- [x] Settings survive a restart
- [x] The Claude Code provider instance can be configured, including model
      selection
- [x] Keybindings can be added, changed and removed
- [x] A configuration change reaches the UI without a restart
- [x] Invalid settings are rejected with a message, leaving the previous values
      intact
- [x] A corrupt or unreadable settings store falls back to defaults with a warning
      rather than failing to start
- [x] A newly configured model is used by the next agent session
- [x] Tests cover update, persistence across restart, live propagation, and
      rejection of invalid input through the socket boundary

## Comments

### What was built

Four methods and the two files behind them.

- `server.getSettings` and `server.updateSettings`, in
  `crates/lightcode-server/src/settings.rs`, over `settings.json`.
- `server.upsertKeybinding` and `server.removeKeybinding`, in
  `src/keybindings.rs`, over `keybindings.json` — whose path this server has
  advertised since ticket 03 precisely so it can be hand-edited.
- Both files are read at startup by `ConfigStore::opening` and written back
  under the store's own write lock, so a change is persisted, applied and
  announced in one order and only one at a time.
- `settings.textGenerationModelSelection` and `keybindings` are now filled;
  both were declared divergences in `tests/socket_conformance.rs` and both
  declarations are gone, which is how the payload is held to the capture.

### The decision worth reading

**ADR-0009** — a setting this server does not know is refused, and one it cannot
decode is never stored. The pressure is that this server _writes_ values
upstream's client _reads_, and Effect schemas fail whole: a `kind` of our own
invention on a config issue, a keybinding command outside the closed union, or a
bad `instanceId` does not degrade — it costs the client the entire payload.

The exception in it is the one that took a bug to find: a patch that **repeats**
a value this server reports but cannot change succeeds, because `save` writes
every field and `load` reads them back through the same `apply`. Refusing the
mention rather than the change made `settings.json` a file this server wrote and
then threw away, and every setting was forgotten at the next restart.

### What the review caught

Three of these would have shipped a config payload the real UI cannot decode.

- **Every issue kind was invented.** `kind: "settings"` / `"keybindings"` against
  a closed union of `keybindings.malformed-config` and
  `keybindings.invalid-entry`. A corrupt keybindings file would have stopped the
  app opening — the exact inverse of the criterion it was written for. Settings
  problems now go to the log, having no member of their own; that gap is named in
  the ADR.
- **Commands were not checked.** `KeybindingCommand` is closed, and `keybindings`
  is an array of rules carrying one, so a single typo in a hand-edited file would
  have cost all forty-one bindings. Checked now on both doors, with the bad entry
  dropped and its index reported.
- **`instanceId` was not checked**, and it is stored — so one bad write would
  have poisoned every settings read until the file was edited by hand.
- Two smaller ones: an upsert naming a `replace` target could leave a duplicate
  of the rule it was also adding (upstream filters on both), and every refusal
  carried an empty `configPath`/`settingsPath` — which is the whole of what those
  errors say to a developer.

### One thing left short

**Hand-editing `keybindings.json` still needs a restart.** Changes made through
the app reach every open window immediately, which is what the criterion asks
for; the file is only re-read at startup. The path is advertised as editable, so
this is a gap rather than a decision — `crate::watcher` already exists to close
it, and upstream watches with a debounce.
