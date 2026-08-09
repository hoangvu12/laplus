//! Historical provider usage read from the provider's own transcript home.
//!
//! This module deliberately extracts only usage metadata. Transcript content is
//! never retained in an output type and therefore cannot cross the socket.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use serde_json::{json, Value};

use crate::config::Settings;

pub const GET_SUMMARY: &str = "server.getUsageSummary";
pub const CONTRACT_VERSION: u8 = 3;

#[derive(Debug, Clone)]
pub struct ReadSummary {
    since_day: String,
    until_day: String,
    time_zone: String,
}

impl ReadSummary {
    pub fn read(payload: &Value) -> Result<Self, Value> {
        let field = |name: &str| {
            payload.get(name).and_then(Value::as_str).map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| read_error("invalidWindow", format!("{name} is required")))
        };
        let since_day = field("sinceDay")?.to_string();
        let until_day = field("untilDay")?.to_string();
        let time_zone = field("timeZone")?.to_string();
        let since = day(&since_day)?;
        let until = day(&until_day)?;
        if since > until {
            return Err(read_error("invalidWindow", "sinceDay must not be after untilDay"));
        }
        time_zone.parse::<Tz>().map_err(|_| read_error("invalidWindow", "timeZone is not a known IANA time zone"))?;
        Ok(Self { since_day, until_day, time_zone })
    }

    pub fn run(self, settings: Settings, host_id: String) -> Result<Value, Value> {
        let started = Instant::now();
        let claude = crate::provider::claude_instance(&settings, crate::provider::CLAUDE_INSTANCE_ID)
            .ok_or_else(|| read_error("scanFailed", "the Claude provider configuration is unavailable"))?;
        let home = crate::catalogue::config_dir(&claude.settings);
        let projects = home.join("projects");
        let zone = self.time_zone.parse::<Tz>()
            .map_err(|_| read_error("invalidWindow", "timeZone is not a known IANA time zone"))?;

        let mut files = Vec::new();
        let missing = !projects.exists();
        if !missing {
            collect_jsonl(&projects, &mut files).map_err(|_| read_error("scanFailed", "Claude transcripts could not be enumerated"))?;
        }

        let mut buckets: BTreeMap<(String, String), Bucket> = BTreeMap::new();
        let mut dedupe = HashSet::new();
        let mut all_sessions = HashSet::new();
        let mut malformed = 0_u64;
        let mut skipped_files = 0_u64;
        for file in &files {
            let text = match fs::read_to_string(file) {
                Ok(text) => text,
                Err(_) => { skipped_files += 1; continue; }
            };
            for line in text.lines() {
                let Some(record) = parse_claude(line, zone) else {
                    if line.contains("\"usage\"") { malformed += 1; }
                    continue;
                };
                if !dedupe.insert(record.dedupe.clone()) { continue; }
                if record.day < self.since_day || record.day > self.until_day { continue; }
                all_sessions.insert(record.session.clone());
                let bucket = buckets.entry((record.day.clone(), record.model.clone()))
                    .or_insert_with(|| Bucket::new(record.day.clone(), record.model.clone()));
                bucket.add(record);
            }
        }

        let status = if missing { "missing" } else if skipped_files > 0 { "partial" } else { "ok" };
        let source = json!({
            "fingerprint": {
                "hostId": host_id,
                "provider": "claude",
                "resolvedHomePath": home.to_string_lossy(),
                "volumeId": volume_id(&projects),
            },
            "status": status,
            "scannedFiles": files.len().saturating_sub(skipped_files as usize),
            "skippedFiles": skipped_files,
            "malformedRecords": malformed,
            "distinctSessions": all_sessions.len(),
            "message": if missing { Some("Claude transcript directory was not found") } else if skipped_files > 0 { Some("Some Claude transcript files could not be read") } else { None },
        });
        Ok(json!({
            "contractVersion": CONTRACT_VERSION,
            "readAt": crate::clock::now_iso(),
            "timeZone": self.time_zone,
            "sinceDay": self.since_day,
            "untilDay": self.until_day,
            "buckets": buckets.into_values().collect::<Vec<_>>(),
            "sources": [source],
            "pricing": {"status":"unavailable", "source":"LiteLLM pricing unavailable", "fetchedAt":null, "knownModels":0},
            "scanDurationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }))
    }
}

fn day(value: &str) -> Result<NaiveDate, Value> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| read_error("invalidWindow", "reporting days must use YYYY-MM-DD"))
}

fn read_error(reason: &str, detail: impl Into<String>) -> Value {
    json!({"_tag":"UsageReadError", "reason":reason, "detail":detail.into()})
}

fn collect_jsonl(root: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() { collect_jsonl(&path, found)?; }
        else if path.extension().and_then(|v| v.to_str()) == Some("jsonl") { found.push(path); }
    }
    found.sort();
    Ok(())
}

struct Record { day: String, model: String, session: String, dedupe: String, totals: Totals }

fn parse_claude(line: &str, zone: Tz) -> Option<Record> {
    if !line.contains("\"usage\"") { return None; }
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type")?.as_str()? != "assistant" { return None; }
    let message = value.get("message")?;
    let usage = message.get("usage")?;
    let model = message.get("model")?.as_str()?.trim();
    if model.is_empty() { return None; }
    let stamp = DateTime::parse_from_rfc3339(value.get("timestamp")?.as_str()?).ok()?.with_timezone(&Utc);
    let message_id = message.get("id").and_then(Value::as_str).unwrap_or("");
    let request_id = value.get("requestId").or_else(|| value.get("request_id")).and_then(Value::as_str).unwrap_or("");
    if message_id.is_empty() && request_id.is_empty() { return None; }
    let session = value.get("sessionId").or_else(|| value.get("session_id")).and_then(Value::as_str).unwrap_or("").to_string();
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    Some(Record {
        day: stamp.with_timezone(&zone).format("%Y-%m-%d").to_string(),
        model: model.to_string(), session,
        dedupe: format!("{message_id}:{request_id}"),
        totals: Totals { uncached_input_tokens: count("input_tokens"), cached_input_tokens: count("cache_read_input_tokens"), cache_creation_tokens: count("cache_creation_input_tokens"), output_tokens: count("output_tokens"), reasoning_tokens: 0 },
    })
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Totals { uncached_input_tokens: u64, cached_input_tokens: u64, cache_creation_tokens: u64, output_tokens: u64, reasoning_tokens: u64 }

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Bucket { day: String, provider: &'static str, model: String, totals: Totals, cost_usd: f64, cache_savings_usd: f64, cost_source: &'static str, records: u64, unpriced_records: u64, sessions: usize, #[serde(skip)] session_ids: HashSet<String> }
impl Bucket {
    fn new(day: String, model: String) -> Self { Self { day, provider:"claude", model, totals:Totals::default(), cost_usd:0.0, cache_savings_usd:0.0, cost_source:"unpriced", records:0, unpriced_records:0, sessions:0, session_ids:HashSet::new() } }
    fn add(&mut self, record: Record) { self.totals.uncached_input_tokens += record.totals.uncached_input_tokens; self.totals.cached_input_tokens += record.totals.cached_input_tokens; self.totals.cache_creation_tokens += record.totals.cache_creation_tokens; self.totals.output_tokens += record.totals.output_tokens; self.records += 1; self.unpriced_records += 1; self.session_ids.insert(record.session); self.sessions = self.session_ids.len(); }
}

#[cfg(unix)]
fn volume_id(path: &Path) -> String { use std::os::unix::fs::MetadataExt; fs::metadata(path).map(|m| format!("{}:{}", m.dev(), m.ino())).unwrap_or_default() }
#[cfg(not(unix))]
fn volume_id(_path: &Path) -> String { String::new() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_usage_metadata_and_buckets_in_the_requested_zone() {
        let line = r#"{"type":"assistant","timestamp":"2026-08-09T23:30:00Z","sessionId":"s1","message":{"id":"m1","model":"claude-opus-5","content":[{"type":"text","text":"secret"}],"usage":{"input_tokens":2,"cache_read_input_tokens":3,"cache_creation_input_tokens":4,"output_tokens":5}}}"#;
        let record = parse_claude(line, "Asia/Tokyo".parse().unwrap()).unwrap();
        assert_eq!(record.day, "2026-08-10");
        assert_eq!(record.totals.output_tokens, 5);
    }
    #[test]
    fn refuses_an_inverted_window_as_the_declared_error() {
        let error = ReadSummary::read(&json!({"sinceDay":"2026-08-10","untilDay":"2026-08-09","timeZone":"UTC"})).unwrap_err();
        assert_eq!(error["_tag"], "UsageReadError");
        assert_eq!(error["reason"], "invalidWindow");
    }
}
