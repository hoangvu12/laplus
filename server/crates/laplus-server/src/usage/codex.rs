//! Pure extraction of aggregate-safe usage metadata from one Codex rollout.
//!
//! Rollouts contain conversation content alongside the counters. This parser
//! deliberately has no output field capable of retaining that content.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Totals {
    pub(crate) uncached_input_tokens: u64,
    pub(crate) cached_input_tokens: u64,
    pub(crate) cache_creation_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) reasoning_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedRecord {
    pub(crate) day: String,
    pub(crate) model: String,
    pub(crate) session: String,
    pub(crate) totals: Totals,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ParseOutcome {
    pub(crate) records: Vec<ParsedRecord>,
    pub(crate) malformed_records: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TokenUsage {
    input: u64,
    cached_input: u64,
    output: u64,
    reasoning: u64,
}

/// Parses one rollout independently, carrying session/model state only within
/// that file. `fallback_session` must be unique to the file when possible.
pub(crate) fn parse_rollout(text: &str, zone: Tz, fallback_session: &str) -> ParseOutcome {
    let mut outcome = ParseOutcome::default();
    let mut session = fallback_session.to_string();
    let mut model: Option<String> = None;
    let mut previous_eligible: Option<TokenUsage> = None;

    for line in text.lines() {
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                if line.contains("token_count") {
                    outcome.malformed_records += 1;
                }
                continue;
            }
        };
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            continue;
        };
        let Some(payload) = value.get("payload") else {
            continue;
        };

        match kind {
            "session_meta" => {
                if let Some(id) = payload.get("id").and_then(non_empty_text) {
                    session = id.to_string();
                }
            }
            "turn_context" => {
                if let Some(next) = payload.get("model").and_then(non_empty_text) {
                    model = Some(next.to_string());
                }
            }
            "event_msg" if payload.get("type").and_then(Value::as_str) == Some("token_count") => {
                let Some(active_model) = model.as_ref() else {
                    // An ineligible notification must not suppress the same
                    // counters once a turn supplies their model context.
                    continue;
                };
                let Some(stamp) = value
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                    .map(|stamp| stamp.with_timezone(&Utc))
                else {
                    outcome.malformed_records += 1;
                    continue;
                };
                let Some(usage) = payload
                    .get("info")
                    .and_then(|info| info.get("last_token_usage"))
                    .and_then(parse_usage)
                else {
                    outcome.malformed_records += 1;
                    continue;
                };
                if previous_eligible.as_ref() == Some(&usage) {
                    continue;
                }
                previous_eligible = Some(usage.clone());
                outcome.records.push(ParsedRecord {
                    day: stamp.with_timezone(&zone).format("%Y-%m-%d").to_string(),
                    model: active_model.clone(),
                    session: session.clone(),
                    totals: Totals {
                        uncached_input_tokens: usage.input.saturating_sub(usage.cached_input),
                        cached_input_tokens: usage.cached_input,
                        cache_creation_tokens: 0,
                        output_tokens: usage.output,
                        reasoning_tokens: usage.reasoning,
                    },
                });
            }
            _ => {}
        }
    }

    outcome
}

fn non_empty_text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn parse_usage(value: &Value) -> Option<TokenUsage> {
    let count = |name| value.get(name).and_then(Value::as_u64);
    let usage = TokenUsage {
        input: count("input_tokens")?,
        cached_input: count("cached_input_tokens").unwrap_or(0),
        output: count("output_tokens")?,
        reasoning: count("reasoning_output_tokens").unwrap_or(0),
    };
    (usage.cached_input <= usage.input && usage.reasoning <= usage.output).then_some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: Option<&str>) -> String {
        serde_json::json!({"timestamp":"2026-08-09T00:00:00Z","type":"session_meta","payload":{"id":id}}).to_string()
    }

    fn context(model: &str) -> String {
        serde_json::json!({"timestamp":"2026-08-09T00:00:00Z","type":"turn_context","payload":{"model":model}}).to_string()
    }

    fn usage(stamp: &str, input: u64, cached: u64, output: u64, reasoning: u64) -> String {
        serde_json::json!({
            "timestamp": stamp,
            "type":"event_msg",
            "payload":{"type":"token_count","info":{"last_token_usage":{
                "input_tokens":input,"cached_input_tokens":cached,
                "output_tokens":output,"reasoning_output_tokens":reasoning
            }}}
        })
        .to_string()
    }

    #[test]
    fn carries_session_and_model_and_maps_disjoint_token_categories() {
        let text = [
            meta(Some("session-1")),
            context("gpt-5-codex"),
            usage("2026-08-09T23:30:00Z", 20, 7, 11, 5),
        ]
        .join("\n");
        let parsed = parse_rollout(&text, "Asia/Tokyo".parse().unwrap(), "file-fallback");

        assert_eq!(
            parsed.records,
            vec![ParsedRecord {
                day: "2026-08-10".into(),
                model: "gpt-5-codex".into(),
                session: "session-1".into(),
                totals: Totals {
                    uncached_input_tokens: 13,
                    cached_input_tokens: 7,
                    cache_creation_tokens: 0,
                    output_tokens: 11,
                    reasoning_tokens: 5
                },
            }]
        );
    }

    #[test]
    fn skips_consecutive_eligible_duplicates_but_not_a_pre_context_copy() {
        let counters = usage("2026-08-09T12:00:00Z", 10, 2, 4, 1);
        let text = [
            counters.clone(),
            context("gpt-5-codex"),
            counters.clone(),
            counters,
        ]
        .join("\n");
        let parsed = parse_rollout(&text, "UTC".parse().unwrap(), "rollout-a");
        assert_eq!(parsed.records.len(), 1);
    }

    #[test]
    fn model_switches_attribute_subsequent_notifications_to_the_new_model() {
        let text = [
            context("gpt-5-a"),
            usage("2026-08-09T10:00:00Z", 10, 0, 1, 0),
            context("gpt-5-b"),
            usage("2026-08-09T11:00:00Z", 12, 1, 2, 1),
        ]
        .join("\n");
        let parsed = parse_rollout(&text, "UTC".parse().unwrap(), "rollout-a");
        assert_eq!(
            parsed
                .records
                .iter()
                .map(|record| record.model.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-5-a", "gpt-5-b"]
        );
    }

    #[test]
    fn missing_session_id_uses_the_file_scoped_fallback() {
        let text = [
            meta(None),
            context("gpt-5"),
            usage("2026-08-09T12:00:00Z", 1, 0, 1, 0),
        ]
        .join("\n");
        let parsed = parse_rollout(&text, "UTC".parse().unwrap(), "rollout-b");
        assert_eq!(parsed.records[0].session, "rollout-b");
    }

    #[test]
    fn malformed_and_irrelevant_rows_do_not_hide_later_valid_usage() {
        let text = [
            "not json".into(),
            r#"{"type":"event_msg","payload":{"type":"token_count""#.into(),
            serde_json::json!({"type":"response_item","payload":{"text":"private"}}).to_string(),
            context("gpt-5"),
            usage("not-a-date", 1, 0, 1, 0),
            usage("2026-08-09T12:00:00Z", 5, 6, 1, 0),
            usage("2026-08-09T12:01:00Z", 5, 1, 3, 2),
        ]
        .join("\n");
        let parsed = parse_rollout(&text, "UTC".parse().unwrap(), "rollout-c");
        assert_eq!(parsed.records.len(), 1);
        assert_eq!(parsed.records[0].totals.output_tokens, 3);
        assert_eq!(parsed.malformed_records, 3);
    }
}
