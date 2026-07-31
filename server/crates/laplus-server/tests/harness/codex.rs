//! A scripted `codex app-server` for provider tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::Value;

pub struct ScriptedCodex {
    directory: tempfile::TempDir,
}

impl ScriptedCodex {
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
        #[cfg(windows)]
        {
            let pid = std::fs::read_to_string(self.directory.path().join("app-server-pid"))
                .expect("the app-server recorded its process id");
            let output = std::process::Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {}", pid.trim()), "/FO", "CSV", "/NH"])
                .output()
                .expect("tasklist checks the app-server process");
            let listed = String::from_utf8_lossy(&output.stdout);
            assert!(
                !listed.contains(&format!(",\"{}\",", pid.trim())),
                "the app-server behind the launcher is still running: {listed}"
            );
        }
        let moved = self.directory.path().join("reaped-codex");
        std::fs::rename(self.app_server_path(), &moved)
            .expect("the app-server behind the launcher is still running after refresh returned");
        std::fs::rename(moved, self.app_server_path()).expect("restores the fixture executable");
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
}
