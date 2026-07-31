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
        std::fs::write(codex.path(), codex.script()).expect("writes the app-server");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(codex.path(), std::fs::Permissions::from_mode(0o755))
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

    pub fn configured(&self) -> String {
        self.path().display().to_string()
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

    pub fn assert_reaped(&self) {
        let moved = self.directory.path().join("reaped-codex");
        std::fs::rename(self.path(), &moved)
            .expect("the probe app-server still holds its executable after refresh returned");
        std::fs::rename(moved, self.path()).expect("restores the fixture executable");
    }

    fn path(&self) -> PathBuf {
        self.directory
            .path()
            .join(if cfg!(windows) { "codex.cmd" } else { "codex" })
    }

    fn script(&self) -> String {
        if cfg!(windows) {
            "@echo off\r\n\
             >\"%~dp0arguments\" echo %*\r\n\
             >\"%~dp0codex-home\" echo %CODEX_HOME%\r\n\
             >&2 echo ERROR optional sandbox dependency is unavailable\r\n\
             set \"LINE=\"\r\n\
             set /p LINE=\r\n\
             type \"%~dp0response-0\"\r\n\
             type \"%~dp0response-1\"\r\n\
             type \"%~dp0response-2\"\r\n\
             type \"%~dp0response-3\"\r\n\
             set \"LINE=\"\r\n\
             set /p LINE=\r\n\
             type \"%~dp0response-4\"\r\n\
             type \"%~dp0response-5\"\r\n\
             type \"%~dp0response-6\"\r\n\
             type \"%~dp0response-7\"\r\n\
             ping -n 2 127.0.0.1 >nul\r\n\
             type \"%~dp0response-8\"\r\n\
             :waiting\r\n\
             set \"LINE=\"\r\n\
             set /p LINE=\r\n\
             if defined LINE goto waiting\r\n"
                .to_string()
        } else {
            "#!/bin/sh\n\
             printf '%s\\n' \"$*\" > \"$(dirname \"$0\")/arguments\"\n\
             printf '%s\\n' \"$CODEX_HOME\" > \"$(dirname \"$0\")/codex-home\"\n\
             printf '%s\\n' 'ERROR optional sandbox dependency is unavailable' >&2\n\
             IFS= read -r line\n\
             cat \"$(dirname \"$0\")/response-0\"\n\
             cat \"$(dirname \"$0\")/response-1\"\n\
             cat \"$(dirname \"$0\")/response-2\"\n\
             cat \"$(dirname \"$0\")/response-3\"\n\
             IFS= read -r line\n\
             IFS= read -r line\n\
             IFS= read -r line\n\
             IFS= read -r line\n\
             cat \"$(dirname \"$0\")/response-4\"\n\
             cat \"$(dirname \"$0\")/response-5\"\n\
             cat \"$(dirname \"$0\")/response-6\"\n\
             cat \"$(dirname \"$0\")/response-7\"\n\
             IFS= read -r line\n\
             IFS= read -r line\n\
             cat \"$(dirname \"$0\")/response-8\"\n\
             while IFS= read -r line; do :; done\n"
                .to_string()
        }
    }
}
