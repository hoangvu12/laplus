//! Golden-file tests for the `claude` CLI wire format — the project's drift
//! detector.
//!
//! Every `*.ndjson` under `fixtures/claude-cli/` is folded line by line through
//! a fresh `SessionState`, and the resulting state is compared against its
//! `*.expected.json` sibling. When a `claude` release moves the format,
//! re-capturing and re-running this says *the CLI moved* — directly, with no
//! server to stand up and no server logic to disentangle the failure from.
//!
//! Adding a capture takes no test code changes: drop the `.ndjson` in, run
//! `UPDATE_GOLDEN=1 cargo test -p laplus-server`, read the minted
//! `.expected.json` to check it says what you meant, and commit both.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use laplus_server::codex_protocol::ConversationState;
use laplus_server::protocol::{ContentBlock, SessionState};
use serde_json::Value;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/claude-cli")
}

fn captures() -> Vec<PathBuf> {
    let dir = fixtures_dir();
    let entries =
        fs::read_dir(&dir).unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()));

    let mut paths: Vec<PathBuf> = entries
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension() == Some(OsStr::new("ndjson")))
        .collect();
    paths.sort();
    paths
}

/// Fold a whole capture the way the agent driver will: one line at a time,
/// malformed lines included.
fn fold(capture: &str) -> SessionState {
    let mut state = SessionState::new();
    for line in capture.lines() {
        state.fold_line(line);
    }
    state
}

fn render(state: &SessionState) -> String {
    let mut json = serde_json::to_string_pretty(state).expect("state serializes");
    json.push('\n');
    json
}

/// Line endings are not the thing under test — a capture or golden file that
/// round-tripped through a CRLF checkout should not read as protocol drift.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

#[test]
fn every_codex_turn_fixture_folds_through_a_fresh_state() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codex-app-server");
    for fixture in ["01-plain-turn", "02-command-execution", "03-write-approval"] {
        let capture = fs::read_to_string(directory.join(format!("{fixture}.jsonl")))
            .unwrap_or_else(|error| panic!("reads the Codex turn fixture {fixture}: {error}"));
        let mut state = ConversationState::new();
        for record in capture.lines().map(|line| {
            serde_json::from_str::<Value>(line).expect("a Codex fixture record")
        }) {
            if record["dir"] == "recv" {
                state.fold_message(record["msg"].clone());
            }
        }

        let mut actual = serde_json::to_string_pretty(&state).expect("Codex state serializes");
        actual.push('\n');
        let expected = fs::read_to_string(directory.join(format!("{fixture}.expected.json")))
            .unwrap_or_else(|error| panic!("reads the expected Codex fold {fixture}: {error}"));

        assert_eq!(actual, normalize(&expected), "Codex fixture {fixture}");
    }
}

#[test]
fn the_write_capture_stops_at_a_request_and_filters_structured_decisions() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codex-app-server/03-write-approval.jsonl");
    let records: Vec<Value> = fs::read_to_string(&fixture)
        .expect("reads the approval fixture")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a Codex fixture record"))
        .collect();
    let request_index = records
        .iter()
        .position(|record| {
            record["dir"] == "recv"
                && record["msg"]["method"] == "item/commandExecution/requestApproval"
        })
        .expect("the captured approval request");
    let response_index = records
        .iter()
        .position(|record| record["dir"] == "send" && record["msg"]["id"] == 0)
        .expect("the captured approval response");

    assert_eq!(response_index, request_index + 1, "replay must stop at the request");
    assert_eq!(
        records[request_index]["msg"]["params"]["availableDecisions"],
        serde_json::json!([
            "accept",
            {"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["/bin/bash", "-lc", "printf 'hi' > hello.txt"]}},
            "cancel"
        ])
    );
}

#[test]
fn the_untrusted_read_only_command_capture_contains_no_approval_request() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/codex-app-server/02-command-execution.jsonl");
    let records: Vec<Value> = fs::read_to_string(&fixture)
        .expect("reads the command fixture")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a Codex fixture record"))
        .collect();
    let thread_start = records
        .iter()
        .find(|record| record["dir"] == "send" && record["msg"]["method"] == "thread/start")
        .expect("the captured thread/start request");

    assert_eq!(thread_start["msg"]["params"]["approvalPolicy"], "untrusted");
    assert_eq!(thread_start["msg"]["params"]["sandbox"], "read-only");
    assert!(!records.iter().any(|record| {
        record["dir"] == "recv"
            && record["msg"]["method"] == "item/commandExecution/requestApproval"
    }));
}

#[test]
fn every_capture_folds_to_its_golden_state() {
    let captures = captures();
    assert!(
        !captures.is_empty(),
        "no captures in {} — the drift detector has nothing to detect drift against",
        fixtures_dir().display()
    );

    let updating = std::env::var_os("UPDATE_GOLDEN").is_some();
    let mut failures: Vec<String> = Vec::new();

    for capture_path in captures {
        let golden_path = capture_path.with_extension("expected.json");
        let name = capture_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("<capture>")
            .to_string();

        let capture = fs::read_to_string(&capture_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", capture_path.display()));
        let actual = render(&fold(&capture));

        if updating {
            fs::write(&golden_path, &actual)
                .unwrap_or_else(|e| panic!("writing {}: {e}", golden_path.display()));
            continue;
        }

        match fs::read_to_string(&golden_path) {
            Ok(expected) if normalize(&expected) == actual => {}
            Ok(expected) => failures.push(format!(
                "{name} folded to a different state than {}:\n--- expected ---\n{}\n--- actual ---\n{actual}",
                golden_path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap_or("<golden>"),
                normalize(&expected)
            )),
            Err(_) => failures.push(format!(
                "{name} has no golden file. Mint it with `UPDATE_GOLDEN=1 cargo test -p laplus-server`, then read it before committing.\n--- would be ---\n{actual}"
            )),
        }
    }

    assert!(failures.is_empty(), "\n\n{}\n", failures.join("\n\n"));
}

/// A golden file with no capture beside it is a capture someone deleted and a
/// stale expectation left behind — quiet loss of coverage, so it fails loudly.
#[test]
fn every_golden_file_has_a_capture() {
    let dir = fixtures_dir();
    let orphans: Vec<String> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter_map(|path| {
            let name = path.file_name().and_then(OsStr::to_str)?;
            let stem = name.strip_suffix(".expected.json")?;
            Some((name.to_string(), dir.join(format!("{stem}.ndjson"))))
        })
        .filter(|(_, capture)| !capture.exists())
        .map(|(name, _)| name)
        .collect();

    assert!(orphans.is_empty(), "golden files with no capture: {orphans:?}");
}

/// The captures exist to exercise the reducer, so at least one of them has to
/// reach each of the wire format's interesting paths. Without this, a capture
/// set could quietly narrow to one boring session and still pass everything.
#[test]
fn the_captures_cover_the_wire_format() {
    let mut totals: BTreeMap<&str, usize> = BTreeMap::new();

    for capture_path in captures() {
        let capture = fs::read_to_string(&capture_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", capture_path.display()));
        let state = fold(&capture);

        *totals.entry("sessions initialized").or_default() +=
            usize::from(state.session_id.is_some());
        *totals.entry("assistant turns").or_default() += state.transcript.len();
        *totals.entry("results").or_default() += usize::from(state.last_result.is_some());
        *totals.entry("streamed turns").or_default() +=
            usize::from(state.counts.contains_key("stream/content_block_delta"));
        *totals.entry("turns reconciled from deltas").or_default() += state
            .transcript
            .iter()
            .filter(|turn| turn.from_deltas)
            .count();
        *totals.entry("unknown events").or_default() += state.unknown_events;
        *totals.entry("parse errors").or_default() += state.parse_errors;

        // Ticket 15's, and the reason they are read off `counts` rather than off
        // the serialized state is that neither leaves a trace in it: compaction
        // deliberately changes nothing a client can see, and a rate-limit notice
        // is a moment rather than a fact about the session. The count is the only
        // record that the capture set still reaches them.
        *totals.entry("results that failed").or_default() += usize::from(
            state
                .last_result
                .as_ref()
                .is_some_and(|result| result.is_error),
        );
        *totals.entry("failures the agent explained").or_default() += usize::from(
            state
                .last_result
                .as_ref()
                .is_some_and(|result| result.error.is_some()),
        );
        *totals.entry("context compactions").or_default() +=
            state.counts.get("system/compact_boundary").copied().unwrap_or(0);
        *totals.entry("rate-limit notices").or_default() +=
            state.counts.get("rate_limit_event").copied().unwrap_or(0);

        // Ticket 76's. The exchange is the only one in this directory that the
        // *server* starts, so it is also the only one a re-capture could quietly
        // drop — a recorder run without the question in it produces a perfectly
        // healthy turn with no answer in it, and every other assertion here would
        // still pass. Counted in two parts because they fail separately: an
        // answer that stopped arriving, and an answer that arrived carrying
        // nothing the meter could be made of.
        *totals.entry("context-window answers").or_default() += state
            .counts
            .get("control_response/get_context_usage")
            .copied()
            .unwrap_or(0);
        *totals
            .entry("sessions told whether the agent compacts by itself")
            .or_default() += usize::from(state.compacts_automatically.is_some());

        // Ticket 12's cases, counted rather than asserted per file: what matters
        // is that the capture set as a whole reaches a tool call, a result that
        // failed, a session that made several calls, and the reasoning between
        // them. A set that narrowed to one happy read would otherwise still pass.
        //
        // Every key is seeded, and that is the whole of whether this works: a
        // counter only inserted where the thing occurs *disappears from `totals`*
        // when the last capture containing it is deleted, and a key that is not
        // there cannot be reported as uncovered.
        let mut counted = |path: &'static str, by: usize| {
            *totals.entry(path).or_default() += by;
        };
        let mut calls = 0;
        let (mut results, mut failures, mut thoughts) = (0, 0, 0);
        for block in state.transcript.iter().flat_map(|turn| turn.content.iter()) {
            match block {
                ContentBlock::ToolUse { .. } => calls += 1,
                ContentBlock::ToolResult { is_error, .. } => {
                    results += 1;
                    failures += usize::from(*is_error);
                }
                ContentBlock::Thinking { .. } => thoughts += 1,
                _ => {}
            }
        }
        counted("tool calls", calls);
        counted("tool results", results);
        counted("failed tool calls", failures);
        counted("thinking blocks", thoughts);
        counted("sessions making several tool calls", usize::from(calls > 1));

        // Ticket 13's, counted the same way and for the same reason. The three
        // the ticket names — approval, rejection and no answer — are three
        // *recordings* rather than three shapes, so what a fold can check is that
        // the capture set still contains a request at all and that a request is
        // still answerable: an approval sends the input back and a
        // session-wide one sends the suggestions back, so a request carrying
        // neither would leave both decisions with nothing to say.
        counted("permission requests", state.permissions.len());
        counted(
            "permission requests naming their tool call",
            state
                .permissions
                .iter()
                .filter(|asked| asked.tool_use_id.is_some())
                .count(),
        );
        counted(
            "permission requests offering a way to stop asking",
            state
                .permissions
                .iter()
                .filter(|asked| !asked.suggestions.is_empty())
                .count(),
        );
    }

    let uncovered: Vec<&&str> = totals
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(path, _)| path)
        .collect();

    assert!(
        uncovered.is_empty(),
        "no capture in {} exercises: {uncovered:?}",
        fixtures_dir().display()
    );
}
