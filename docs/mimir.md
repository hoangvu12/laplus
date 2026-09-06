# Mimir provider

Laplus connects to Mimir through a separately installed SDK bridge. Mimir owns
agent execution, credentials, compaction, classifier routing and tool summaries;
Laplus uses its native conversation, tool, question, plan, task and diff views.
There is no second agent loop or ACP dependency.

## Install

1. Install and configure Mimir; confirm your chosen model works there first.
2. Download the **bridge** and **context companion** archives from
   [mimir-bridge releases](https://github.com/wasimysaid/mimir-bridge/releases).
   Check the release's exact compatible Mimir revision and checksums. The Wasm
   packages are shared across Windows, macOS and Linux; the Mimir executable is
   platform-specific. These prerelease packages require matching host APIs, not
   just a matching `0.1.0` version string.
3. Extract both archives and explicitly install their package directories:
   ```sh
   mimir plugin install ./org.mimir.bridge
   mimir plugin install ./org.mimir.context
   ```
   Enable both plugins if previously disabled, then restart Laplus. Installation
   preserves existing enablement; Laplus never installs or enables plugins for you.
4. In **Settings → Providers → Add provider → Mimir**, choose the executable path
   (for example `C:\Tools\mimir.exe` on Windows) and keep the bridge command
   `/org.mimir.bridge:serve`. Enable the instance.
5. Pick a configured Mimir model in the composer. Provider keys stay in Mimir;
   do not put credentials in Laplus settings or command arguments.

The [standalone bridge repository](https://github.com/wasimysaid/mimir-bridge)
contains the protocol, plugin source, build instructions and real-host/browser
harnesses. Installing prebuilt packages needs no SDK access; source builds
currently require access to the private Mimir SDK repository.

## Native workflow

- Build/Plan and model/reasoning selection use Mimir's session controls.
- Compact tools expand to their inputs/results; context usage feeds the composer
  meter rather than generating transcript noise.
- File changes use **Open diff** for saved-turn checkpoints and the right panel's
  **Diff → Working tree** for current Git changes, including new files.
- `update_status` feeds a collapsible checklist above the text box. Observed child
  work remains available in the native subagent panel after reload.
- Questions and **Save and stop → Implement** use Mimir's actual request/plan IDs.
- **Steer** changes the running request; **Queue follow-up** schedules a later turn.
  Cancelled sessions can be reused, and saved conversations can be reopened.
- Type `/` for supported native command suggestions. Bare `/goal` displays the
  goal; `/goal <objective>` starts one. `/compress` and `/init` also run natively.
  Slash commands require an idle session with no queued/retryable work; they are
  never combined with queued prose or sent as live steering.
- Ask to use `configure-mimir` to inspect or change Mimir configuration. The
  companion provides lazy guidance, not automatic edits. Specify project/global
  scope; never paste credentials into chat.

## Boundaries

This is not full Mimir TUI parity. The SDK does not promise lossless history
replay, complete child-history recovery, arbitrary child control or a remote
plugin/provider administration UI. On a replay gap, recorded content and queued
input are retained, but **Retry** deliberately opens a fresh attachment rather
than admitting work against uncertain events. Deleting a Laplus thread does not
invoke a Mimir-history deletion API.

Mimir's tool policy remains authoritative; unsupported Laplus approval modes are
not offered. Laplus Git checkpoints are separate from Mimir's agent history.
Transient MCP attachments are conversation-owned and never write global MCP
configuration. This Laplus revision has no production MCP toolkits; that boundary
is exercised with injected tools in tests, not advertised as built-in actions.
