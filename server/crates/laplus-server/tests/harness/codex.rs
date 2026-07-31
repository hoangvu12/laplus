//! A scripted `codex app-server` for provider tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

pub struct ScriptedCodex {
    directory: tempfile::TempDir,
}

impl ScriptedCodex {
    pub fn conversation_paused_after_first_delta() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("01-plain-turn", Some(5));
        std::fs::write(codex.directory.path().join("pause-turn"), "")
            .expect("marks the first turn as paused");
        codex
    }

    pub fn command_conversation() -> ScriptedCodex {
        ScriptedCodex::conversation_from_fixture("02-command-execution", None)
    }

    pub fn failed_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("01-plain-turn", None);
        std::fs::write(codex.directory.path().join("fail-turn"), "")
            .expect("marks turns as failed");
        codex
    }

    pub fn rejected_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("01-plain-turn", None);
        std::fs::write(codex.directory.path().join("reject-turn"), "")
            .expect("rejects turn/start");
        codex
    }

    fn conversation_from_fixture(fixture: &str, pause_after: Option<usize>) -> ScriptedCodex {
        let codex = ScriptedCodex::provider_probe();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/codex-app-server/{fixture}.jsonl"));
        let received: Vec<Value> = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|error| panic!("reading {}: {error}", fixture.display()))
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a turn fixture record"))
            .filter(|record| record["dir"] == "recv")
            .map(|record| record["msg"].clone())
            .collect();
        for (name, id) in [("initialize", 1), ("thread", 2), ("turn", 3)] {
            let result = received
                .iter()
                .find(|message| message["id"] == id)
                .unwrap_or_else(|| panic!("the fixture has the {name} response"))["result"]
                .clone();
            std::fs::write(
                codex.directory.path().join(format!("conversation-{name}-result")),
                result.to_string(),
            )
            .expect("writes a fixture response");
        }
        let events: Vec<&Value> = received
            .iter()
            .skip_while(|message| message["id"] != 3)
            .skip(1)
            .collect();
        let terminal = events.last().expect("the fixture has a terminal turn event");
        let pause_after = pause_after.unwrap_or(events.len() - 1);
        let before = events[..events.len() - 1]
            .iter()
            .take(pause_after)
            .map(|message| format!("{message}\n"))
            .collect::<String>();
        let after = events[..events.len() - 1]
            .iter()
            .skip(pause_after)
            .map(|message| format!("{message}\n"))
            .collect::<String>();
        for (name, content) in [
            ("turn-events-before-pause", before),
            ("turn-events-after-pause", after),
            ("turn-terminal", format!("{terminal}\n")),
        ] {
            std::fs::write(codex.directory.path().join(name), content)
                .expect("writes fixture turn events");
        }
        std::fs::write(codex.app_server_path(), codex.conversation_script())
            .expect("writes the conversation app-server");
        codex
    }

    pub fn provider_probe() -> ScriptedCodex {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/01-provider-probe.jsonl");
        let records: Vec<Value> = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|error| panic!("reading {}: {error}", fixture.display()))
            .lines()
            .map(|line| serde_json::from_str(line).expect("a fixture record"))
            .collect();
        let received: Vec<String> = records
            .iter()
            .filter(|record| record["dir"] == "recv")
            .map(|record| record["msg"].to_string())
            .collect();

        let codex = ScriptedCodex {
            directory: tempfile::tempdir().expect("a temporary directory"),
        };
        for (index, line) in received.iter().enumerate() {
            std::fs::write(
                codex.directory.path().join(format!("response-{index}")),
                format!("{line}\n"),
            )
            .expect("writes a response");
        }
        std::fs::write(codex.app_server_path(), codex.app_server_script())
            .expect("writes the app-server");
        #[cfg(windows)]
        std::fs::write(codex.path(), codex.launcher_script()).expect("writes the cmd launcher");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                codex.app_server_path(),
                std::fs::Permissions::from_mode(0o755),
            )
                .expect("sets the mode");
        }
        codex
    }

    pub fn logged_out_provider_probe() -> ScriptedCodex {
        let codex = ScriptedCodex::provider_probe();
        std::fs::write(
            codex.directory.path().join("response-6"),
            "{\"id\":2,\"result\":{\"account\":null,\"requiresOpenaiAuth\":true}}\n",
        )
        .expect("writes the logged-out account response");
        codex
    }

    pub fn provider_probe_with_email(email: &str) -> ScriptedCodex {
        let codex = ScriptedCodex::provider_probe();
        codex.replace_account(email);
        codex
    }

    pub fn blocked_provider_probe_with_email(email: &str) -> ScriptedCodex {
        let codex = ScriptedCodex::provider_probe_with_email(email);
        std::fs::write(codex.directory.path().join("block"), "")
            .expect("marks the probe as blocked");
        codex
    }

    pub fn missing_user_agent() -> ScriptedCodex {
        ScriptedCodex::with_response(0, r#"{"id":1,"result":{}}"#)
    }

    pub fn missing_model_data() -> ScriptedCodex {
        ScriptedCodex::with_response(
            7,
            r#"{"id":3,"result":{"nextCursor":"page-2"}}"#,
        )
    }

    pub fn missing_skills_data() -> ScriptedCodex {
        ScriptedCodex::with_response(5, r#"{"id":4,"result":{}}"#)
    }

    fn with_response(index: usize, response: &str) -> ScriptedCodex {
        let codex = ScriptedCodex::provider_probe();
        std::fs::write(
            codex.directory.path().join(format!("response-{index}")),
            format!("{response}\n"),
        )
        .expect("replaces a provider response");
        codex
    }

    fn replace_account(&self, email: &str) {
        std::fs::write(
            self.directory.path().join("response-6"),
            format!(
                "{{\"id\":2,\"result\":{{\"account\":{{\"type\":\"chatgpt\",\"email\":{email:?},\"planType\":\"prolite\"}},\"requiresOpenaiAuth\":true}}}}\n"
            ),
        )
        .expect("replaces the account response");
    }

    pub fn configured(&self) -> String {
        self.path().display().to_string()
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    pub fn started(&self) -> bool {
        self.directory.path().join("started").exists()
    }

    pub fn release(&self) {
        std::fs::write(self.directory.path().join("release"), "")
            .expect("releases the blocked probe");
    }

    pub fn release_turn(&self) {
        std::fs::write(self.directory.path().join("release-turn"), "")
            .expect("releases the paused turn");
    }

    pub fn conversation_starts(&self) -> usize {
        std::fs::read_to_string(self.directory.path().join("conversation-starts"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    pub fn turn_requests(&self) -> usize {
        std::fs::read_to_string(self.directory.path().join("conversation-requests"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|message| message["method"] == "turn/start")
            .count()
    }

    pub fn conversation_cwd(&self) -> String {
        std::fs::read_to_string(self.directory.path().join("conversation-cwd"))
            .expect("the conversation app-server recorded its cwd")
            .trim()
            .to_string()
    }

    pub fn arguments(&self) -> String {
        std::fs::read_to_string(self.directory.path().join("arguments"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    pub fn codex_home(&self) -> String {
        std::fs::read_to_string(self.directory.path().join("codex-home"))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    pub fn skill_cwds(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .find(|message| message["method"] == "skills/list")
            .and_then(|message| message["params"]["cwds"].as_array().cloned())
            .expect("the probe sent skills/list with cwds")
            .into_iter()
            .map(|cwd| cwd.as_str().expect("a cwd string").to_string())
            .collect()
    }

    pub fn assert_exchange(&self) {
        let mut actual = self.requests();
        let expected: Vec<Value> = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/codex-app-server/01-provider-probe.jsonl"),
        )
        .expect("reads the provider fixture")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a fixture record"))
        .filter(|record| record["dir"] == "send")
        .map(|record| record["msg"].clone())
        .collect();

        let skills = actual
            .iter_mut()
            .find(|message| message["method"] == "skills/list")
            .expect("skills/list was sent");
        assert!(
            skills["params"]["cwds"]
                .as_array()
                .is_some_and(|cwds| !cwds.is_empty() && cwds.iter().all(Value::is_string)),
            "skills/list must name its workspaces: {skills}"
        );
        skills["params"]["cwds"] = serde_json::json!(["<workspace>"]);
        assert_eq!(actual, expected, "outbound Codex protocol drifted");
    }

    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("requests"))
            .expect("the app-server recorded requests")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a recorded request"))
            .collect()
    }

    pub fn assert_reaped(&self) {
        self.assert_pid_reaped("app-server-pid", "app-server behind the launcher");
        let moved = self.directory.path().join("reaped-codex");
        std::fs::rename(self.app_server_path(), &moved)
            .expect("the app-server behind the launcher is still running after refresh returned");
        std::fs::rename(moved, self.app_server_path()).expect("restores the fixture executable");
    }

    pub fn assert_conversation_reaped(&self) {
        self.assert_pid_reaped("conversation-pid", "conversation app-server");
    }

    fn assert_pid_reaped(&self, pid_file: &str, description: &str) {
        let pid = std::fs::read_to_string(self.directory.path().join(pid_file))
            .unwrap_or_else(|error| panic!("the {description} recorded its process id: {error}"));
        #[cfg(windows)]
        {
            let output = std::process::Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {}", pid.trim()), "/FO", "CSV", "/NH"])
                .output()
                .expect("tasklist checks the conversation app-server process");
            let listed = String::from_utf8_lossy(&output.stdout);
            assert!(
                !listed.contains(&format!(",\"{}\",", pid.trim())),
                "the {description} is still running: {listed}"
            );
        }
        #[cfg(not(windows))]
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success()),
            "the {description} is still running"
        );
    }

    #[cfg(not(windows))]
    pub fn running(&self) -> bool {
        let pid = std::fs::read_to_string(self.directory.path().join("app-server-pid"))
            .expect("the app-server recorded its process id");
        std::process::Command::new("kill")
            .args(["-0", pid.trim()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn path(&self) -> PathBuf {
        self.directory
            .path()
            .join(if cfg!(windows) { "codex.cmd" } else { "codex" })
    }

    fn app_server_path(&self) -> PathBuf {
        if cfg!(windows) {
            self.directory.path().join("codex-app-server.ps1")
        } else {
            self.path()
        }
    }

    #[cfg(windows)]
    fn launcher_script(&self) -> String {
        "@echo off\r\npowershell.exe -NoProfile -ExecutionPolicy Bypass -File \"%~dp0codex-app-server.ps1\" %*\r\n".to_string()
    }

    fn app_server_script(&self) -> String {
        if cfg!(windows) {
            r#"$requests = Join-Path $PSScriptRoot 'requests'
[IO.File]::WriteAllText($requests, '')
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'arguments'), ($args -join ' '))
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'codex-home'), $env:CODEX_HOME)
[IO.File]::WriteAllText((Join-Path $PSScriptRoot 'app-server-pid'), [string]$PID)
[Console]::Error.WriteLine('ERROR optional sandbox dependency is unavailable')

function Read-Request {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { exit 2 }
  [IO.File]::AppendAllText($requests, $line + [Environment]::NewLine)
}
function Send-Response([int]$index) {
  [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $PSScriptRoot "response-$index")))
  [Console]::Out.Flush()
}

Read-Request
0..3 | ForEach-Object { Send-Response $_ }
1..4 | ForEach-Object { Read-Request }
if (Test-Path (Join-Path $PSScriptRoot 'block')) {
  [IO.File]::WriteAllText((Join-Path $PSScriptRoot 'started'), '')
  while (-not (Test-Path (Join-Path $PSScriptRoot 'release'))) { Start-Sleep -Milliseconds 50 }
}
Send-Response 4
Read-Request
5..7 | ForEach-Object { Send-Response $_ }
Read-Request
Send-Response 8
while ($true) { Start-Sleep -Seconds 1 }
"#
            .to_string()
        } else {
            "#!/bin/sh\n\
             printf '%s\\n' \"$$\" > \"$(dirname \"$0\")/app-server-pid\"\n\
             requests=\"$(dirname \"$0\")/requests\"\n\
             : > \"$requests\"\n\
             read_request() {\n\
               IFS= read -r line || exit 2\n\
               printf '%s\\n' \"$line\" >> \"$requests\"\n\
             }\n\
             printf '%s\\n' \"$*\" > \"$(dirname \"$0\")/arguments\"\n\
             printf '%s\\n' \"$CODEX_HOME\" > \"$(dirname \"$0\")/codex-home\"\n\
             printf '%s\\n' 'ERROR optional sandbox dependency is unavailable' >&2\n\
             read_request\n\
             cat \"$(dirname \"$0\")/response-0\"\n\
             cat \"$(dirname \"$0\")/response-1\"\n\
             cat \"$(dirname \"$0\")/response-2\"\n\
             cat \"$(dirname \"$0\")/response-3\"\n\
             read_request\n\
             read_request\n\
             read_request\n\
             read_request\n\
             if [ -f \"$(dirname \"$0\")/block\" ]; then\n\
               : > \"$(dirname \"$0\")/started\"\n\
               while [ ! -f \"$(dirname \"$0\")/release\" ]; do sleep 0.05; done\n\
             fi\n\
             cat \"$(dirname \"$0\")/response-4\"\n\
             read_request\n\
             cat \"$(dirname \"$0\")/response-5\"\n\
             cat \"$(dirname \"$0\")/response-6\"\n\
             cat \"$(dirname \"$0\")/response-7\"\n\
             read_request\n\
             cat \"$(dirname \"$0\")/response-8\"\n\
             while true; do sleep 1; done\n"
                .to_string()
        }
    }

    fn conversation_script(&self) -> String {
        if cfg!(windows) {
            r#"$root = $PSScriptRoot
$allRequests = Join-Path $root 'requests'
$conversationRequests = Join-Path $root 'conversation-requests'
[IO.File]::WriteAllText($allRequests, '')
[IO.File]::WriteAllText((Join-Path $root 'arguments'), ($args -join ' '))
[IO.File]::WriteAllText((Join-Path $root 'codex-home'), $env:CODEX_HOME)
[IO.File]::WriteAllText((Join-Path $root 'app-server-pid'), [string]$PID)

function Read-Request {
  $line = [Console]::In.ReadLine()
  if ($null -eq $line) { exit 2 }
  [IO.File]::AppendAllText($allRequests, $line + [Environment]::NewLine)
  return $line
}
function Send-File([int]$index) {
  [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root "response-$index")))
  [Console]::Out.Flush()
}
function Send-Json([string]$json) {
  [Console]::Out.WriteLine($json)
  [Console]::Out.Flush()
}

$initialize = (Read-Request | ConvertFrom-Json)
Send-Json ('{"id":' + $initialize.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-initialize-result')) + '}')
1..3 | ForEach-Object { Send-File $_ }
$null = Read-Request
$nextLine = Read-Request
$next = $nextLine | ConvertFrom-Json
if ($next.method -eq 'thread/start') {
  [IO.File]::AppendAllText((Join-Path $root 'conversation-starts'), "$PID`n")
  [IO.File]::WriteAllText((Join-Path $root 'conversation-pid'), [string]$PID)
  [IO.File]::WriteAllText((Join-Path $root 'conversation-cwd'), (Get-Location).Path)
  [IO.File]::AppendAllText($conversationRequests, $nextLine + [Environment]::NewLine)
  Send-Json ('{"id":' + $next.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-thread-result')) + '}')
  $turn = 0
  while ($true) {
    $line = Read-Request
    [IO.File]::AppendAllText($conversationRequests, $line + [Environment]::NewLine)
    $request = $line | ConvertFrom-Json
    if ($request.method -ne 'turn/start') { continue }
    $turn += 1
    if (Test-Path (Join-Path $root 'reject-turn')) {
      Send-Json ('{"id":' + $request.id + ',"error":{"code":-32603,"message":"fixture turn start rejected"}}')
      continue
    }
    Send-Json ('{"id":' + $request.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-turn-result')) + '}')
    [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-events-before-pause')))
    [Console]::Out.Flush()
    if ($turn -eq 1 -and (Test-Path (Join-Path $root 'pause-turn'))) {
      while (-not (Test-Path (Join-Path $root 'release-turn'))) { Start-Sleep -Milliseconds 20 }
    }
    [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-events-after-pause')))
    [Console]::Out.Flush()
    if (Test-Path (Join-Path $root 'fail-turn')) {
      Send-Json '{"method":"turn/completed","params":{"threadId":"codex-thread-1","turn":{"id":"codex-turn-1","status":"failed","error":{"message":"fixture turn failed"},"durationMs":5750}}}'
    } else {
      [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-terminal')))
      [Console]::Out.Flush()
    }
  }
}

$model = Read-Request
$skills = Read-Request
Send-File 4
$null = Read-Request
5..7 | ForEach-Object { Send-File $_ }
$null = Read-Request
Send-File 8
while ($true) { Start-Sleep -Seconds 1 }
"#.to_string()
        } else {
            r#"#!/bin/sh
root="$(dirname "$0")"
requests="$root/requests"
conversation_requests="$root/conversation-requests"
: > "$requests"
printf '%s\n' "$*" > "$root/arguments"
printf '%s\n' "$CODEX_HOME" > "$root/codex-home"
printf '%s\n' "$$" > "$root/app-server-pid"
read_request() {
  IFS= read -r line || exit 2
  printf '%s\n' "$line" >> "$requests"
}
request_id() {
  rest=${1#*\"id\":}
  printf '%s' "${rest%%,*}"
}
send_file() {
  cat "$root/response-$1"
}

read_request
id=$(request_id "$line")
printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-initialize-result")"
send_file 1
send_file 2
send_file 3
read_request
read_request
next="$line"
case "$next" in
  *'"method":"thread/start"'*)
    printf '%s\n' "$$" >> "$root/conversation-starts"
    printf '%s\n' "$$" > "$root/conversation-pid"
    pwd > "$root/conversation-cwd"
    printf '%s\n' "$next" >> "$conversation_requests"
    id=$(request_id "$next")
    printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-thread-result")"
    turn=0
    while read_request; do
      printf '%s\n' "$line" >> "$conversation_requests"
      case "$line" in
        *'"method":"turn/start"'*)
          turn=$((turn + 1))
          id=$(request_id "$line")
          if [ -f "$root/reject-turn" ]; then
            printf '{"id":%s,"error":{"code":-32603,"message":"fixture turn start rejected"}}\n' "$id"
            continue
          fi
          printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-turn-result")"
          cat "$root/turn-events-before-pause"
          if [ "$turn" -eq 1 ] && [ -f "$root/pause-turn" ]; then
            while [ ! -f "$root/release-turn" ]; do sleep 0.02; done
          fi
          cat "$root/turn-events-after-pause"
          if [ -f "$root/fail-turn" ]; then
            printf '%s\n' '{"method":"turn/completed","params":{"threadId":"codex-thread-1","turn":{"id":"codex-turn-1","status":"failed","error":{"message":"fixture turn failed"},"durationMs":5750}}}'
          else
            cat "$root/turn-terminal"
          fi
          ;;
      esac
    done
    ;;
  *)
    read_request
    read_request
    send_file 4
    read_request
    send_file 5
    send_file 6
    send_file 7
    read_request
    send_file 8
    while true; do sleep 1; done
    ;;
esac
"#.to_string()
        }
    }
}
