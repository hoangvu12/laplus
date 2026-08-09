//! Claude Code transcript parsing for the historical Usage report.
//!
//! The types in this module deliberately have no place to retain message
//! content. A parsed record contains only the metadata needed for aggregation.

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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedRecord {
    pub(crate) day: String,
    pub(crate) model: String,
    pub(crate) session_id: String,
    /// The upstream message/request identity. `None` means this record cannot
    /// be safely de-duplicated and must remain independently countable.
    pub(crate) dedupe_key: Option<String>,
    pub(crate) totals: Totals,
    pub(crate) reported_cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ParseOutcome {
    Record(ParsedRecord),
    /// A valid transcript row that is not an assistant usage record.
    Irrelevant,
    /// A candidate assistant usage row whose required metadata is damaged.
    Malformed,
}

pub(crate) fn parse_line(line: &str, zone: Tz) -> ParseOutcome {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return ParseOutcome::Malformed,
    };
    let Some(object) = value.as_object() else {
        return ParseOutcome::Irrelevant;
    };
    if object.get("type").and_then(Value::as_str) != Some("assistant") {
        return ParseOutcome::Irrelevant;
    }

    let Some(message) = object.get("message").and_then(Value::as_object) else {
        return ParseOutcome::Malformed;
    };
    let Some(usage) = message.get("usage").and_then(Value::as_object) else {
        return ParseOutcome::Malformed;
    };
    let Some(model) = message
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
    else {
        return ParseOutcome::Malformed;
    };
    let Some(timestamp) = object.get("timestamp").and_then(Value::as_str) else {
        return ParseOutcome::Malformed;
    };
    let Ok(timestamp) = DateTime::parse_from_rfc3339(timestamp) else {
        return ParseOutcome::Malformed;
    };

    let message_id = message.get("id").and_then(Value::as_str);
    let request_id = text_alias(object, "requestId", "request_id");
    let dedupe_key = if message_id.is_none() && request_id.is_none() {
        None
    } else {
        Some(format!(
            "{}:{}",
            message_id.unwrap_or_default(),
            request_id.unwrap_or_default()
        ))
    };
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    let cost = object
        .get("costUSD")
        .and_then(Value::as_f64)
        .filter(|cost| cost.is_finite());

    ParseOutcome::Record(ParsedRecord {
        day: timestamp
            .with_timezone(&Utc)
            .with_timezone(&zone)
            .format("%Y-%m-%d")
            .to_string(),
        model: model.to_string(),
        session_id: text_alias(object, "sessionId", "session_id")
            .unwrap_or_default()
            .to_string(),
        dedupe_key,
        totals: Totals {
            uncached_input_tokens: count("input_tokens"),
            cached_input_tokens: count("cache_read_input_tokens"),
            cache_creation_tokens: count("cache_creation_input_tokens"),
            output_tokens: count("output_tokens"),
            reasoning_tokens: 0,
        },
        reported_cost_usd: cost,
    })
}

fn text_alias<'a>(
    object: &'a serde_json::Map<String, Value>,
    camel: &str,
    snake: &str,
) -> Option<&'a str> {
    object
        .get(camel)
        .and_then(Value::as_str)
        .or_else(|| object.get(snake).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(overrides: Value) -> String {
        let mut base = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-08-09T23:30:00Z",
            "sessionId": "session-1",
            "requestId": "request-1",
            "costUSD": 1.25,
            "message": {
                "id": "message-1",
                "model": "claude-opus-5",
                "content": [{"type":"text", "text":"must not be retained"}],
                "usage": {
                    "input_tokens": 2,
                    "cache_read_input_tokens": 3,
                    "cache_creation_input_tokens": 4,
                    "output_tokens": 5
                }
            }
        });
        merge(&mut base, overrides);
        serde_json::to_string(&base).unwrap()
    }

    fn merge(target: &mut Value, changes: Value) {
        let (Some(target), Some(changes)) = (target.as_object_mut(), changes.as_object()) else {
            return;
        };
        for (key, value) in changes {
            if let (Some(current), true) = (
                target.get_mut(key),
                value.as_object().is_some_and(|object| !object.is_empty()),
            ) {
                merge(current, value.clone());
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
    }

    fn record(line: &str) -> ParsedRecord {
        match parse_line(line, "Asia/Tokyo".parse().unwrap()) {
            ParseOutcome::Record(record) => record,
            outcome => panic!("expected record, got {outcome:?}"),
        }
    }

    #[test]
    fn extracts_usage_metadata_cost_and_caller_local_day_without_content() {
        let record = record(&assistant(serde_json::json!({})));
        assert_eq!(record.day, "2026-08-10");
        assert_eq!(record.model, "claude-opus-5");
        assert_eq!(record.session_id, "session-1");
        assert_eq!(record.dedupe_key.as_deref(), Some("message-1:request-1"));
        assert_eq!(record.reported_cost_usd, Some(1.25));
        assert_eq!(
            record.totals,
            Totals {
                uncached_input_tokens: 2,
                cached_input_tokens: 3,
                cache_creation_tokens: 4,
                output_tokens: 5,
                reasoning_tokens: 0,
            }
        );
        assert!(!format!("{record:?}").contains("must not be retained"));
    }

    #[test]
    fn preserves_identity_less_records_as_independently_countable() {
        let line = assistant(serde_json::json!({
            "requestId": null,
            "message": {"id": null}
        }));
        assert_eq!(record(&line).dedupe_key, None);
    }

    #[test]
    fn accepts_each_request_and_session_spelling() {
        let snake = assistant(serde_json::json!({
            "requestId": null,
            "sessionId": null,
            "request_id": "snake-request",
            "session_id": "snake-session"
        }));
        let parsed = record(&snake);
        assert_eq!(
            parsed.dedupe_key.as_deref(),
            Some("message-1:snake-request")
        );
        assert_eq!(parsed.session_id, "snake-session");
    }

    #[test]
    fn ignores_non_finite_or_non_numeric_provider_costs() {
        for cost in [serde_json::json!("1.25"), serde_json::json!(null)] {
            assert_eq!(
                record(&assistant(serde_json::json!({"costUSD": cost}))).reported_cost_usd,
                None
            );
        }
    }

    #[test]
    fn distinguishes_irrelevant_rows_from_damaged_candidates() {
        assert_eq!(
            parse_line(r#"{"type":"user"}"#, chrono_tz::UTC),
            ParseOutcome::Irrelevant
        );
        for line in [
            "not json",
            r#"{"type":"assistant"}"#,
            r#"{"type":"assistant","message":{"model":"m","usage":{}}}"#,
            r#"{"type":"assistant","timestamp":"bad","message":{"model":"m","usage":{}}}"#,
            r#"{"type":"assistant","timestamp":"2026-08-09T00:00:00Z","message":{"model":"","usage":{}}}"#,
        ] {
            assert_eq!(parse_line(line, chrono_tz::UTC), ParseOutcome::Malformed);
        }
    }

    #[test]
    fn missing_usage_counts_are_zero_without_losing_the_valid_record() {
        let parsed = record(&assistant(serde_json::json!({
            "message": {"usage": {}}
        })));
        assert_eq!(
            parsed.totals,
            Totals {
                uncached_input_tokens: 0,
                cached_input_tokens: 0,
                cache_creation_tokens: 0,
                output_tokens: 0,
                reasoning_tokens: 0,
            }
        );
    }
}
