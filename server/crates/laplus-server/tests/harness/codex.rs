//! A scripted `codex app-server` for provider tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;

pub struct ScriptedCodex {
    directory: tempfile::TempDir,
}

fn rewrite_conversation_ids(value: &mut Value) {
    match value {
        Value::String(text) if text == "codex-thread-1" => {
            *text = "codex-thread-4".to_string();
        }
        Value::String(text) if text == "codex-turn-1" => {
            *text = "codex-turn-5".to_string();
        }
        Value::Array(values) => values.iter_mut().for_each(rewrite_conversation_ids),
        Value::Object(fields) => fields.values_mut().for_each(rewrite_conversation_ids),
        _ => {}
    }
}

impl ScriptedCodex {
    pub fn plain_conversation() -> ScriptedCodex {
        ScriptedCodex::conversation_from_fixture("01-plain-turn", None)
    }

    pub fn conversation_paused_after_first_delta() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("01-plain-turn", Some(5));
        std::fs::write(codex.directory.path().join("pause-turn"), "")
            .expect("marks the first turn as paused");
        codex
    }

    pub fn command_conversation() -> ScriptedCodex {
        ScriptedCodex::conversation_from_fixture("02-command-execution", None)
    }

    pub fn initialization_drift_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("01-plain-turn", None);
        std::fs::write(
            codex
                .directory
                .path()
                .join("initialize-events-before-response"),
            concat!(
                "{\"method\":\"future/startup\",\"params\":{}}\n",
                "[]\n",
                "{\"method\":\n",
                "{\"id\":\"startup-request\",\"method\":\"future/request\",\"params\":{}}\n",
            ),
        )
        .expect("writes initialization drift");
        std::fs::write(
            codex.directory.path().join("thread-events-before-response"),
            "}{\n[\"future thread envelope\"]\n",
        )
        .expect("writes thread/start drift");
        std::fs::write(codex.directory.path().join("await-initialize-answer"), "")
            .expect("marks the startup server request");
        codex
    }

    pub fn malformed_turn_start_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("01-plain-turn", None);
        std::fs::write(codex.directory.path().join("conversation-turn-result"), "{}")
            .expect("writes the malformed turn/start result");
        codex
    }

    pub fn synthetic_drift_conversation() -> ScriptedCodex {
        ScriptedCodex::conversation_from_fixture("07-synthetic-drift", None)
    }

    pub fn approval_conversation() -> ScriptedCodex {
        ScriptedCodex::conversation_from_fixture("03-write-approval", None)
    }

    pub fn interrupted_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("04-interrupt", None);
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/04-interrupt.jsonl");
        let records: Vec<Value> = std::fs::read_to_string(&fixture)
            .expect("reads the interrupt fixture")
            .lines()
            .map(|line| serde_json::from_str(line).expect("an interrupt fixture record"))
            .collect();
        let turn_response = records
            .iter()
            .position(|record| record["dir"] == "recv" && record["msg"]["id"] == 3)
            .expect("the turn/start response");
        let interrupt = records
            .iter()
            .position(|record| {
                record["dir"] == "send" && record["msg"]["method"] == "turn/interrupt"
            })
            .expect("the interrupt request");
        let acknowledgement = records
            .iter()
            .position(|record| record["dir"] == "recv" && record["msg"]["id"] == 4)
            .expect("the interrupt acknowledgement");
        let received = |records: &[Value]| {
            records
                .iter()
                .filter(|record| record["dir"] == "recv")
                .map(|record| format!("{}\n", record["msg"]))
                .collect::<String>()
        };
        std::fs::write(
            codex.directory.path().join("turn-events-before-pause"),
            received(&records[turn_response + 1..interrupt]),
        )
        .expect("writes pre-interrupt events");
        std::fs::write(
            codex.directory.path().join("turn-events-after-pause"),
            received(&records[interrupt + 1..acknowledgement]),
        )
        .expect("writes post-interrupt events");
        std::fs::write(codex.directory.path().join("await-interrupt"), "")
            .expect("marks the fixture interrupt stop");
        let correction_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/01-plain-turn.jsonl");
        let correction: Vec<Value> = std::fs::read_to_string(&correction_fixture)
            .expect("reads the captured correction turn")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a correction fixture record"))
            .collect();
        let correction_start = correction
            .iter()
            .position(|record| record["dir"] == "recv" && record["msg"]["id"] == 3)
            .expect("the captured correction turn response");
        let mut correction_result = correction[correction_start]["msg"]["result"].clone();
        correction_result["turn"]["id"] = Value::String("codex-turn-5".to_string());
        std::fs::write(
            codex.directory.path().join("correction-turn-result"),
            correction_result.to_string(),
        )
        .expect("writes the correction result");
        let correction_events = correction[correction_start + 1..]
            .iter()
            .filter(|record| record["dir"] == "recv")
            .map(|record| {
                let mut message = record["msg"].clone();
                rewrite_conversation_ids(&mut message);
                format!("{message}\n")
            })
            .collect::<String>();
        std::fs::write(
            codex.directory.path().join("correction-turn-events"),
            correction_events,
        )
        .expect("writes the captured correction events");
        std::fs::write(codex.app_server_path(), codex.conversation_script())
            .expect("rewrites the interrupt app-server");
        codex
    }

    pub fn resumable_conversation() -> ScriptedCodex {
        ScriptedCodex::conversation_from_fixture("05-resume", None)
    }

    pub fn add_resume_drift(&self) {
        let path = self.directory.path().join("thread-events-before-response");
        let mut events = std::fs::read_to_string(&path).expect("reads captured resume events");
        events.push_str("}{\n[\"future resume envelope\"]\n");
        std::fs::write(path, events).expect("adds drift before the resume response");
    }

    pub fn missing_resume_conversation() -> ScriptedCodex {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/06-resume-missing.jsonl");
        let records: Vec<Value> = std::fs::read_to_string(&fixture)
            .expect("reads the missing-resume fixture")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a missing-resume record"))
            .collect();
        let initialize = records
            .iter()
            .find(|record| record["dir"] == "recv" && record["msg"]["id"] == 1)
            .expect("the captured initialize response")["msg"]["result"]
            .clone();
        let refusal = records
            .iter()
            .find(|record| record["dir"] == "recv" && record["msg"]["error"].is_object())
            .expect("the captured resume refusal")["msg"]["error"]
            .clone();
        let before_refusal = records
            .iter()
            .skip_while(|record| record["dir"] != "send" || record["msg"]["id"] != 2)
            .skip(1)
            .take_while(|record| record["dir"] != "recv" || !record["msg"]["error"].is_object())
            .filter(|record| record["dir"] == "recv")
            .map(|record| format!("{}\n", record["msg"]))
            .collect::<String>();

        let codex = ScriptedCodex::provider_probe();
        for (name, content) in [
            ("conversation-initialize-result", initialize.to_string()),
            ("initialize-events-before-response", String::new()),
            ("thread-events-before-response", before_refusal),
            ("resume-error", refusal.to_string()),
            (
                "conversation-fallback-thread-result",
                serde_json::json!({"thread": {"id": "codex-thread-fresh"}}).to_string(),
            ),
            (
                "conversation-turn-result",
                serde_json::json!({
                    "turn": {"id": "codex-turn-fresh", "status": "inProgress", "error": null}
                })
                .to_string(),
            ),
            ("turn-events-before-pause", String::new()),
            ("turn-events-after-pause", String::new()),
            (
                "turn-terminal",
                format!(
                    "{}\n",
                    serde_json::json!({
                        "method": "turn/completed",
                        "params": {
                            "threadId": "codex-thread-fresh",
                            "turn": {
                                "id": "codex-turn-fresh",
                                "status": "completed",
                                "error": null,
                                "durationMs": 1
                            }
                        }
                    })
                ),
            ),
        ] {
            std::fs::write(codex.directory.path().join(name), content)
                .unwrap_or_else(|error| panic!("writes {name}: {error}"));
        }
        std::fs::write(codex.directory.path().join("skip-provider-startup-noise"), "")
            .expect("keeps the capture 06 replay free of provider-fixture noise");
        std::fs::write(codex.app_server_path(), codex.conversation_script())
            .expect("writes the missing-resume app-server");
        codex
    }

    pub fn arbitrary_resume_failure_conversation(message: &str) -> ScriptedCodex {
        ScriptedCodex::resume_failure_conversation(serde_json::json!({
            "code": -32099,
            "message": message,
        }))
    }

    fn resume_failure_conversation(refusal: Value) -> ScriptedCodex {
        let codex = ScriptedCodex::conversation_from_fixture("05-resume", None);
        std::fs::write(
            codex.directory.path().join("resume-error"),
            refusal.to_string(),
        )
        .expect("writes the captured resume refusal");
        let mut fresh: Value = serde_json::from_str(
            &std::fs::read_to_string(codex.directory.path().join("conversation-thread-result"))
                .expect("reads the captured thread result"),
        )
        .expect("the thread result is JSON");
        fresh["thread"]["id"] = Value::String("codex-thread-fresh".to_string());
        std::fs::write(
            codex.directory.path().join("conversation-fallback-thread-result"),
            fresh.to_string(),
        )
        .expect("writes the fallback thread result");
        codex
    }

    pub fn unrestricted_write_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::approval_conversation();
        codex.rewrite_turn_events(|message| {
            (message["method"] != "item/commandExecution/requestApproval").then_some(message)
        });
        std::fs::remove_file(codex.directory.path().join("await-approval"))
            .expect("removes the fixture approval stop");
        codex
    }

    pub fn declinable_approval_conversation() -> ScriptedCodex {
        let codex = ScriptedCodex::approval_conversation();
        codex.rewrite_turn_events(|mut message| {
            if message["method"] == "item/commandExecution/requestApproval" {
                message["params"]["availableDecisions"] =
                    serde_json::json!(["accept", "decline", "cancel"]);
            }
            Some(message)
        });
        codex
    }

    pub fn file_read_approval_conversation() -> ScriptedCodex {
        ScriptedCodex::file_approval_conversation("item/fileRead/requestApproval")
    }

    pub fn file_change_approval_conversation() -> ScriptedCodex {
        ScriptedCodex::file_approval_conversation("item/fileChange/requestApproval")
    }

    fn file_approval_conversation(method: &str) -> ScriptedCodex {
        let codex = ScriptedCodex::approval_conversation();
        codex.rewrite_turn_events(|mut message| {
            if message["method"] == "item/started"
                && message["params"]["item"]["type"] == "commandExecution"
            {
                return None;
            }
            if message["method"] == "item/commandExecution/requestApproval" {
                message["method"] = Value::String(method.to_string());
                message["params"]["path"] = Value::String("hello.txt".to_string());
            }
            Some(message)
        });
        codex
    }

    fn rewrite_turn_events(&self, mut rewrite: impl FnMut(Value) -> Option<Value>) {
        let path = self.directory.path().join("turn-events-before-pause");
        let rewritten = std::fs::read_to_string(&path)
            .expect("reads approval events")
            .lines()
            .filter_map(|line| {
                let message = serde_json::from_str(line).expect("an approval event");
                rewrite(message).map(|message| format!("{message}\n"))
            })
            .collect::<String>();
        std::fs::write(path, rewritten).expect("writes rewritten approval events");
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
        let records: Vec<Value> = std::fs::read_to_string(&fixture)
            .unwrap_or_else(|error| panic!("reading {}: {error}", fixture.display()))
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a turn fixture record"))
            .collect();
        for (name, id) in [("initialize", 1), ("thread", 2), ("turn", 3)] {
            let result = records
                .iter()
                .find(|record| record["dir"] == "recv" && record["msg"]["id"] == id)
                .unwrap_or_else(|| panic!("the fixture has the {name} response"))["msg"]["result"]
                .clone();
            std::fs::write(
                codex.directory.path().join(format!("conversation-{name}-result")),
                result.to_string(),
            )
            .expect("writes a fixture response");
        }
        let before_thread_response = records
            .iter()
            .skip_while(|record| record["dir"] != "send" || record["msg"]["id"] != 2)
            .skip(1)
            .take_while(|record| record["dir"] != "recv" || record["msg"]["id"] != 2)
            .filter(|record| record["dir"] == "recv" || record["dir"] == "recv-raw")
            .map(|record| match record["dir"].as_str() {
                Some("recv") => format!("{}\n", record["msg"]),
                Some("recv-raw") => format!(
                    "{}\n",
                    record["msg"]
                        .as_str()
                        .expect("a raw Codex fixture line is a string")
                ),
                direction => panic!("unexpected Codex fixture direction {direction:?}"),
            })
            .collect::<String>();
        std::fs::write(
            codex.directory.path().join("thread-events-before-response"),
            before_thread_response,
        )
        .expect("writes fixture events before the thread response");
        std::fs::write(
            codex
                .directory
                .path()
                .join("initialize-events-before-response"),
            "",
        )
        .expect("writes the empty initialization prelude");
        let events: Vec<&Value> = records
            .iter()
            .skip_while(|record| record["dir"] != "recv" || record["msg"]["id"] != 3)
            .skip(1)
            .filter(|record| record["dir"] == "recv" || record["dir"] == "recv-raw")
            .collect();
        let terminal = events.last().expect("the fixture has a terminal turn event");
        let approval_pause = events
            .iter()
            .position(|record| {
                record["msg"]["method"]
                    .as_str()
                    .is_some_and(|method| method.ends_with("/requestApproval"))
            })
            .map(|index| index + 1);
        let pause_after = approval_pause.or(pause_after).unwrap_or(events.len() - 1);
        if approval_pause.is_some() {
            std::fs::write(codex.directory.path().join("await-approval"), "")
                .expect("marks the fixture approval stop");
        }
        let line = |record: &&Value| match record["dir"].as_str() {
            Some("recv") => format!("{}\n", record["msg"]),
            Some("recv-raw") => format!(
                "{}\n",
                record["msg"]
                    .as_str()
                    .expect("a raw Codex fixture line is a string")
            ),
            direction => panic!("unexpected Codex fixture direction {direction:?}"),
        };
        let before = events[..events.len() - 1]
            .iter()
            .take(pause_after)
            .map(line)
            .collect::<String>();
        let after = events[..events.len() - 1]
            .iter()
            .skip(pause_after)
            .map(line)
            .collect::<String>();
        for (name, content) in [
            ("turn-events-before-pause", before),
            ("turn-events-after-pause", after),
            ("turn-terminal", line(terminal)),
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

    pub fn reject_turns(&self) {
        std::fs::write(self.directory.path().join("reject-turn"), "")
            .expect("marks later turn starts as rejected");
    }

    pub fn fail_next_turn_write(&self) {
        std::fs::write(self.directory.path().join("close-input-after-turn"), "")
            .expect("marks the app-server input for closing after this turn");
    }

    pub fn release_interrupt(&self) {
        std::fs::write(self.directory.path().join("release-interrupt"), "")
            .expect("releases the interrupt acknowledgement");
    }

    pub fn conversation_starts(&self) -> usize {
        std::fs::read_to_string(self.directory.path().join("conversation-starts"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    pub fn turn_requests(&self) -> usize {
        self.turn_start_requests().len()
    }

    pub fn turn_start_requests(&self) -> Vec<Value> {
        self.conversation_requests()
            .into_iter()
            .filter(|message| message["method"] == "turn/start")
            .collect()
    }

    pub fn interrupt_requests(&self) -> Vec<Value> {
        self.conversation_requests()
            .into_iter()
            .filter(|message| message["method"] == "turn/interrupt")
            .collect()
    }

    pub fn thread_requests(&self) -> Vec<Value> {
        self.conversation_requests()
            .into_iter()
            .filter(|message| {
                message["method"] == "thread/start" || message["method"] == "thread/resume"
            })
            .collect()
    }

    pub fn approval_answers(&self) -> Vec<Value> {
        self.conversation_requests()
            .into_iter()
            .filter(|message| message["result"]["decision"].is_string())
            .collect()
    }

    pub fn unsupported_answers(&self) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|message| message["error"]["code"] == -32601)
            .collect()
    }

    pub fn assert_missing_resume_capture_prefix(&self) {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/codex-app-server/06-resume-missing.jsonl");
        let expected: Vec<Value> = std::fs::read_to_string(fixture)
            .expect("reads capture 06")
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a capture 06 record"))
            .filter(|record| record["dir"] == "send")
            .map(|record| record["msg"].clone())
            .collect();
        let actual: Vec<Value> = self.requests().into_iter().take(expected.len()).collect();
        assert_eq!(actual, expected, "the missing-resume replay drifted from capture 06");
    }

    fn conversation_requests(&self) -> Vec<Value> {
        std::fs::read_to_string(self.directory.path().join("conversation-requests"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
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
[Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'initialize-events-before-response')))
[Console]::Out.Flush()
if (Test-Path (Join-Path $root 'await-initialize-answer')) { $null = Read-Request }
Send-Json ('{"id":' + $initialize.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-initialize-result')) + '}')
if (-not (Test-Path (Join-Path $root 'skip-provider-startup-noise'))) {
  1..3 | ForEach-Object { Send-File $_ }
}
$null = Read-Request
$nextLine = Read-Request
$next = $nextLine | ConvertFrom-Json
[Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'thread-events-before-response')))
[Console]::Out.Flush()
if ($next.method -eq 'thread/start') {
  [IO.File]::AppendAllText((Join-Path $root 'conversation-starts'), "$PID`n")
  [IO.File]::WriteAllText((Join-Path $root 'conversation-pid'), [string]$PID)
  [IO.File]::WriteAllText((Join-Path $root 'conversation-cwd'), (Get-Location).Path)
  [IO.File]::AppendAllText($conversationRequests, $nextLine + [Environment]::NewLine)
  Send-Json ('{"id":' + $next.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-thread-result')) + '}')
  $turn = 0
} elseif ($next.method -eq 'thread/resume') {
  [IO.File]::WriteAllText((Join-Path $root 'conversation-pid'), [string]$PID)
  [IO.File]::WriteAllText((Join-Path $root 'conversation-cwd'), (Get-Location).Path)
  [IO.File]::AppendAllText($conversationRequests, $nextLine + [Environment]::NewLine)
  if (Test-Path (Join-Path $root 'resume-error')) {
    Send-Json ('{"id":' + $next.id + ',"error":' + [IO.File]::ReadAllText((Join-Path $root 'resume-error')) + '}')
    $nextLine = Read-Request
    $next = $nextLine | ConvertFrom-Json
    if ($next.method -ne 'thread/start') { exit 4 }
    [IO.File]::AppendAllText((Join-Path $root 'conversation-starts'), "$PID`n")
    [IO.File]::AppendAllText($conversationRequests, $nextLine + [Environment]::NewLine)
    Send-Json ('{"id":' + $next.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-fallback-thread-result')) + '}')
  } else {
    Send-Json ('{"id":' + $next.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-thread-result')) + '}')
  }
  $turn = 0
}
if ($next.method -eq 'thread/start' -or $next.method -eq 'thread/resume') {
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
    if ($turn -gt 1 -and (Test-Path (Join-Path $root 'await-interrupt'))) {
      Send-Json ('{"id":' + $request.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'correction-turn-result')) + '}')
      [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'correction-turn-events')))
      [Console]::Out.Flush()
      continue
    }
    Send-Json ('{"id":' + $request.id + ',"result":' + [IO.File]::ReadAllText((Join-Path $root 'conversation-turn-result')) + '}')
    [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-events-before-pause')))
    [Console]::Out.Flush()
    if (Test-Path (Join-Path $root 'await-interrupt')) {
      $interruptLine = Read-Request
      [IO.File]::AppendAllText($conversationRequests, $interruptLine + [Environment]::NewLine)
      $interrupt = $interruptLine | ConvertFrom-Json
      if ($interrupt.method -ne 'turn/interrupt') { exit 3 }
      [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-events-after-pause')))
      [Console]::Out.Flush()
      while (-not (Test-Path (Join-Path $root 'release-interrupt'))) { Start-Sleep -Milliseconds 20 }
      Send-Json ('{"id":' + $interrupt.id + ',"result":{}}')
      continue
    }
    if (Test-Path (Join-Path $root 'await-approval')) {
      $approvalLine = Read-Request
      [IO.File]::AppendAllText($conversationRequests, $approvalLine + [Environment]::NewLine)
      $approval = $approvalLine | ConvertFrom-Json
      if ($approval.result.decision -ne 'accept') {
        continue
      }
    }
    if ($turn -eq 1 -and (Test-Path (Join-Path $root 'pause-turn'))) {
      while (-not (Test-Path (Join-Path $root 'release-turn'))) { Start-Sleep -Milliseconds 20 }
    }
    [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-events-after-pause')))
    [Console]::Out.Flush()
    if (Test-Path (Join-Path $root 'fail-turn')) {
      Send-Json '{"method":"turn/completed","params":{"threadId":"codex-thread-1","turn":{"id":"codex-turn-1","status":"failed","error":{"message":"fixture turn failed"},"durationMs":5750}}}'
    } else {
      if (Test-Path (Join-Path $root 'close-input-after-turn')) {
        [Console]::In.Close()
      }
      [Console]::Out.Write([IO.File]::ReadAllText((Join-Path $root 'turn-terminal')))
      [Console]::Out.Flush()
      if (Test-Path (Join-Path $root 'close-input-after-turn')) {
        while ($true) { Start-Sleep -Seconds 1 }
      }
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
cat "$root/initialize-events-before-response"
if [ -f "$root/await-initialize-answer" ]; then read_request; fi
printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-initialize-result")"
if [ ! -f "$root/skip-provider-startup-noise" ]; then
  send_file 1
  send_file 2
  send_file 3
fi
read_request
read_request
next="$line"
cat "$root/thread-events-before-response"
case "$next" in
  *'"method":"thread/start"'*)
    printf '%s\n' "$$" >> "$root/conversation-starts"
    printf '%s\n' "$$" > "$root/conversation-pid"
    pwd > "$root/conversation-cwd"
    printf '%s\n' "$next" >> "$conversation_requests"
    id=$(request_id "$next")
    printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-thread-result")"
    ;;
  *'"method":"thread/resume"'*)
    printf '%s\n' "$$" > "$root/conversation-pid"
    pwd > "$root/conversation-cwd"
    printf '%s\n' "$next" >> "$conversation_requests"
    id=$(request_id "$next")
    if [ -f "$root/resume-error" ]; then
      printf '{"id":%s,"error":%s}\n' "$id" "$(cat "$root/resume-error")"
      read_request
      next="$line"
      case "$next" in
        *'"method":"thread/start"'*) ;;
        *) exit 4 ;;
      esac
      printf '%s\n' "$$" >> "$root/conversation-starts"
      printf '%s\n' "$next" >> "$conversation_requests"
      id=$(request_id "$next")
      printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-fallback-thread-result")"
    else
      printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-thread-result")"
    fi
    ;;
esac
case "$next" in
  *'"method":"thread/start"'*|*'"method":"thread/resume"'*)
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
          if [ "$turn" -gt 1 ] && [ -f "$root/await-interrupt" ]; then
            printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/correction-turn-result")"
            cat "$root/correction-turn-events"
            continue
          fi
          printf '{"id":%s,"result":%s}\n' "$id" "$(cat "$root/conversation-turn-result")"
          cat "$root/turn-events-before-pause"
          if [ -f "$root/await-interrupt" ]; then
            read_request
            printf '%s\n' "$line" >> "$conversation_requests"
            case "$line" in
              *'"method":"turn/interrupt"'*) ;;
              *) exit 3 ;;
            esac
            id=$(request_id "$line")
            cat "$root/turn-events-after-pause"
            while [ ! -f "$root/release-interrupt" ]; do sleep 0.02; done
            printf '{"id":%s,"result":{}}\n' "$id"
            continue
          fi
          if [ -f "$root/await-approval" ]; then
             read_request
             printf '%s\n' "$line" >> "$conversation_requests"
              case "$line" in
                *'"decision":"accept"'*) ;;
                *) continue ;;
              esac
           fi
          if [ "$turn" -eq 1 ] && [ -f "$root/pause-turn" ]; then
            while [ ! -f "$root/release-turn" ]; do sleep 0.02; done
          fi
          cat "$root/turn-events-after-pause"
          if [ -f "$root/fail-turn" ]; then
            printf '%s\n' '{"method":"turn/completed","params":{"threadId":"codex-thread-1","turn":{"id":"codex-turn-1","status":"failed","error":{"message":"fixture turn failed"},"durationMs":5750}}}'
          else
            if [ -f "$root/close-input-after-turn" ]; then
              exec 0<&-
            fi
            cat "$root/turn-terminal"
            if [ -f "$root/close-input-after-turn" ]; then
              while true; do sleep 1; done
            fi
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
