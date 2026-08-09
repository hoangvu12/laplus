//! Historical provider usage read from provider-owned transcript homes.
//!
//! Only aggregate-safe metadata is retained. Prompt, response, reasoning and
//! tool content have no representation in this module's output types.

mod claude;
mod codex;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::NaiveDate;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Settings;

pub const GET_SUMMARY: &str = "server.getUsageSummary";
pub const CONTRACT_VERSION: u8 = 3;

#[derive(Debug, Clone)]
pub struct UsageScan {
    since_day: String,
    until_day: String,
    time_zone: String,
    zone: Tz,
}

impl UsageScan {
    pub fn from_payload(payload: &Value) -> Result<Self, Value> {
        let field = |name: &str| {
            payload
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| read_error("invalidWindow", format!("{name} is required")))
        };
        let since_day = field("sinceDay")?.to_string();
        let until_day = field("untilDay")?.to_string();
        let requested_zone = field("timeZone")?;
        let since = day(&since_day)?;
        let until = day(&until_day)?;
        if since > until {
            return Err(read_error(
                "invalidWindow",
                "sinceDay must not be after untilDay",
            ));
        }
        let zone = requested_zone.parse::<Tz>().unwrap_or(chrono_tz::UTC);
        Ok(Self {
            since_day,
            until_day,
            time_zone: zone.name().to_string(),
            zone,
        })
    }

    pub fn run(
        self,
        settings: Settings,
        host_id: String,
        preferences: PathBuf,
    ) -> Result<Value, Value> {
        let started = Instant::now();
        let since = day(&self.since_day).expect("the reporting day was validated");
        let cache_path = preferences.join("usage-scan-cache.json");
        let mut cache = ScanCache::load(&cache_path);
        let claude =
            crate::provider::claude_instance(&settings, crate::provider::CLAUDE_INSTANCE_ID);
        let codex = crate::provider::codex_instance(&settings, crate::provider::CODEX_INSTANCE_ID);

        let mut records = Vec::new();
        let mut sources = Vec::new();
        if let Some(instance) = claude.filter(|instance| instance.settings.enabled) {
            let home = crate::catalogue::config_dir(&instance.settings);
            let (source, mut parsed) = scan_claude(&home, self.zone, since, &host_id, &mut cache);
            sources.push(source);
            records.append(&mut parsed);
        }
        if let Some(instance) = codex.filter(|instance| instance.settings.enabled) {
            let home = codex_home(&instance.settings.home_path);
            let (source, mut parsed) = scan_codex(&home, self.zone, since, &host_id, &mut cache);
            sources.push(source);
            records.append(&mut parsed);
        }

        let mut buckets: BTreeMap<(String, String, String), Bucket> = BTreeMap::new();
        for record in records {
            if record.day < self.since_day || record.day > self.until_day {
                continue;
            }
            let key = (
                record.day.clone(),
                record.provider.clone(),
                record.model.clone(),
            );
            buckets
                .entry(key)
                .or_insert_with(|| Bucket::new(&record))
                .add(record);
        }

        cache.prune();
        cache.save(&cache_path);
        Ok(json!({
            "contractVersion": CONTRACT_VERSION,
            "readAt": crate::clock::now_iso(),
            "timeZone": self.time_zone,
            "sinceDay": self.since_day,
            "untilDay": self.until_day,
            "buckets": buckets.into_values().collect::<Vec<_>>(),
            "sources": sources,
            "pricing": {"status":"unavailable", "source":"LiteLLM pricing unavailable", "fetchedAt":null, "knownModels":0},
            "scanDurationMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }))
    }
}

fn scan_claude(
    home: &Path,
    zone: Tz,
    since: NaiveDate,
    host_id: &str,
    cache: &mut ScanCache,
) -> (Value, Vec<Record>) {
    let root = claude_projects(home);
    let Ok(files) = transcript_files(&root) else {
        return (
            failed_source(
                host_id,
                "claude",
                home,
                "Claude transcripts could not be enumerated",
            ),
            Vec::new(),
        );
    };
    if !root.exists() {
        return (
            missing_source(
                host_id,
                "claude",
                home,
                "Claude transcript directory was not found",
            ),
            Vec::new(),
        );
    }

    let mut records = Vec::new();
    let mut dedupe = HashSet::new();
    let mut malformed = 0_u64;
    let mut skipped = 0_u64;
    let mut scanned = 0_u64;
    let mut sessions = HashSet::new();
    let cache_provider = format!("claude@{}", zone.name());
    for file in files {
        if !file_could_contribute(&file, since) {
            continue;
        }
        let Some((identity, key)) = file_identity(&file) else {
            skipped += 1;
            continue;
        };
        let parsed = if let Some((records, cached_malformed)) =
            cache.reuse(&key, &cache_provider, identity)
        {
            malformed += cached_malformed;
            records
        } else {
            let Ok(text) = fs::read_to_string(&file) else {
                skipped += 1;
                continue;
            };
            let mut parsed = Vec::new();
            let mut file_malformed = 0;
            for line in text.lines() {
                match claude::parse_line(line, zone) {
                    claude::ParseOutcome::Record(record) => {
                        parsed.push(Record::from_claude(record))
                    }
                    claude::ParseOutcome::Malformed => file_malformed += 1,
                    claude::ParseOutcome::Irrelevant => {}
                }
            }
            malformed += file_malformed;
            cache.replace(
                key,
                &cache_provider,
                identity,
                parsed.clone(),
                file_malformed,
            );
            parsed
        };
        scanned += 1;
        for record in parsed {
            if record
                .dedupe_key
                .as_ref()
                .is_some_and(|key| !dedupe.insert(key.clone()))
            {
                continue;
            }
            if !record.session.is_empty() {
                sessions.insert(record.session.clone());
            }
            records.push(record);
        }
    }
    let partial = skipped > 0 || malformed > 0;
    (
        source(
            host_id,
            "claude",
            home,
            if partial { "partial" } else { "ok" },
            scanned,
            skipped,
            malformed,
            sessions.len(),
            partial.then_some("Some Claude transcript records could not be read"),
        ),
        records,
    )
}

fn scan_codex(
    home: &Path,
    zone: Tz,
    since: NaiveDate,
    host_id: &str,
    cache: &mut ScanCache,
) -> (Value, Vec<Record>) {
    let root = home.join("sessions");
    let Ok(files) = transcript_files(&root) else {
        return (
            failed_source(
                host_id,
                "codex",
                home,
                "Codex transcripts could not be enumerated",
            ),
            Vec::new(),
        );
    };
    if !root.exists() {
        return (
            missing_source(
                host_id,
                "codex",
                home,
                "Codex transcript directory was not found",
            ),
            Vec::new(),
        );
    }
    let mut records = Vec::new();
    let mut malformed = 0_u64;
    let mut skipped = 0_u64;
    let mut scanned = 0_u64;
    let mut sessions = HashSet::new();
    let cache_provider = format!("codex@{}", zone.name());
    for file in files {
        if !file_could_contribute(&file, since) {
            continue;
        }
        let Some((identity, key)) = file_identity(&file) else {
            skipped += 1;
            continue;
        };
        let parsed = if let Some((records, cached_malformed)) =
            cache.reuse(&key, &cache_provider, identity)
        {
            malformed += cached_malformed;
            records
        } else {
            let Ok(text) = fs::read_to_string(&file) else {
                skipped += 1;
                continue;
            };
            let parsed = codex::parse_rollout(&text, zone, &key);
            let file_malformed = parsed.malformed_records;
            malformed += file_malformed;
            let records = parsed
                .records
                .into_iter()
                .map(Record::from_codex)
                .collect::<Vec<_>>();
            cache.replace(
                key,
                &cache_provider,
                identity,
                records.clone(),
                file_malformed,
            );
            records
        };
        scanned += 1;
        for record in parsed {
            if !record.session.is_empty() {
                sessions.insert(record.session.clone());
            }
            records.push(record);
        }
    }
    let partial = skipped > 0 || malformed > 0;
    (
        source(
            host_id,
            "codex",
            home,
            if partial { "partial" } else { "ok" },
            scanned,
            skipped,
            malformed,
            sessions.len(),
            partial.then_some("Some Codex transcript records could not be read"),
        ),
        records,
    )
}

fn claude_projects(home: &Path) -> PathBuf {
    let nested = home.join(".claude").join("projects");
    if nested.exists() {
        nested
    } else {
        home.join("projects")
    }
}

fn codex_home(configured: &str) -> PathBuf {
    if !configured.trim().is_empty() {
        return crate::projects::expand_home(configured.trim());
    }
    if let Some(home) = std::env::var_os("CODEX_HOME").filter(|home| !home.is_empty()) {
        return PathBuf::from(home);
    }
    for variable in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(variable).filter(|home| !home.is_empty()) {
            return PathBuf::from(home).join(".codex");
        }
    }
    PathBuf::from(".codex")
}

fn transcript_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    if root.exists() {
        collect_jsonl(root, &mut found)?;
    }
    found.sort();
    Ok(found)
}

fn collect_jsonl(root: &Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_jsonl(&path, found)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            found.push(path);
        }
    }
    Ok(())
}

fn day(value: &str) -> Result<NaiveDate, Value> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| read_error("invalidWindow", "reporting days must use YYYY-MM-DD"))
}

fn read_error(reason: &str, detail: impl Into<String>) -> Value {
    json!({"_tag":"UsageReadError", "reason":reason, "detail":detail.into()})
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ScanCache {
    #[serde(default)]
    files: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    provider: String,
    size: u64,
    modified_ns: u128,
    records: Vec<Record>,
    #[serde(default)]
    malformed: u64,
}

impl ScanCache {
    fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn reuse(
        &self,
        key: &str,
        provider: &str,
        identity: (u64, u128),
    ) -> Option<(Vec<Record>, u64)> {
        let entry = self.files.get(key)?;
        (entry.provider == provider && entry.size == identity.0 && entry.modified_ns == identity.1)
            .then(|| (entry.records.clone(), entry.malformed))
    }

    fn replace(
        &mut self,
        key: String,
        provider: &str,
        identity: (u64, u128),
        records: Vec<Record>,
        malformed: u64,
    ) {
        self.files.insert(
            key,
            CacheEntry {
                provider: provider.to_string(),
                size: identity.0,
                modified_ns: identity.1,
                records,
                malformed,
            },
        );
    }

    fn prune(&mut self) {
        let oldest = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .saturating_sub(92 * 24 * 60 * 60 * 1_000_000_000_u128);
        self.files
            .retain(|path, entry| entry.modified_ns >= oldest && Path::new(path).exists());
    }

    fn save(&self, path: &Path) {
        if let Ok(encoded) = serde_json::to_vec(self) {
            let _ = fs::create_dir_all(path.parent().unwrap_or_else(|| Path::new(".")));
            let _ = fs::write(path, encoded);
        }
    }
}

fn file_identity(path: &Path) -> Option<((u64, u128), String)> {
    let metadata = fs::metadata(path).ok()?;
    let modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Some((
        (metadata.len(), modified_ns),
        key.to_string_lossy().into_owned(),
    ))
}

fn file_could_contribute(path: &Path, since: NaiveDate) -> bool {
    let cutoff = since
        .and_hms_opt(0, 0, 0)
        .expect("midnight exists")
        .and_utc()
        .timestamp()
        .saturating_sub(36 * 60 * 60);
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|modified| i64::try_from(modified.as_secs()).ok())
        .is_none_or(|modified| modified >= cutoff)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    day: String,
    provider: String,
    model: String,
    session: String,
    totals: Totals,
    reported_cost_usd: Option<f64>,
    dedupe_key: Option<String>,
}
impl Record {
    fn from_claude(record: claude::ParsedRecord) -> Self {
        Self {
            day: record.day,
            provider: "claude".into(),
            model: record.model,
            session: record.session_id,
            totals: Totals {
                uncached_input_tokens: record.totals.uncached_input_tokens,
                cached_input_tokens: record.totals.cached_input_tokens,
                cache_creation_tokens: record.totals.cache_creation_tokens,
                output_tokens: record.totals.output_tokens,
                reasoning_tokens: record.totals.reasoning_tokens,
            },
            reported_cost_usd: record.reported_cost_usd,
            dedupe_key: record.dedupe_key,
        }
    }
    fn from_codex(record: codex::ParsedRecord) -> Self {
        Self {
            day: record.day,
            provider: "codex".into(),
            model: record.model,
            session: record.session,
            totals: Totals {
                uncached_input_tokens: record.totals.uncached_input_tokens,
                cached_input_tokens: record.totals.cached_input_tokens,
                cache_creation_tokens: record.totals.cache_creation_tokens,
                output_tokens: record.totals.output_tokens,
                reasoning_tokens: record.totals.reasoning_tokens,
            },
            reported_cost_usd: None,
            dedupe_key: None,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Totals {
    uncached_input_tokens: u64,
    cached_input_tokens: u64,
    cache_creation_tokens: u64,
    output_tokens: u64,
    reasoning_tokens: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    day: String,
    provider: String,
    model: String,
    totals: Totals,
    cost_usd: f64,
    cache_savings_usd: f64,
    cost_source: &'static str,
    records: u64,
    unpriced_records: u64,
    sessions: usize,
    #[serde(skip)]
    session_ids: HashSet<String>,
}
impl Bucket {
    fn new(record: &Record) -> Self {
        Self {
            day: record.day.clone(),
            provider: record.provider.clone(),
            model: record.model.clone(),
            totals: Totals::default(),
            cost_usd: 0.0,
            cache_savings_usd: 0.0,
            cost_source: "unpriced",
            records: 0,
            unpriced_records: 0,
            sessions: 0,
            session_ids: HashSet::new(),
        }
    }
    fn add(&mut self, record: Record) {
        self.totals.uncached_input_tokens += record.totals.uncached_input_tokens;
        self.totals.cached_input_tokens += record.totals.cached_input_tokens;
        self.totals.cache_creation_tokens += record.totals.cache_creation_tokens;
        self.totals.output_tokens += record.totals.output_tokens;
        self.totals.reasoning_tokens += record.totals.reasoning_tokens;
        self.records += 1;
        self.unpriced_records += 1;
        if !record.session.is_empty() {
            self.session_ids.insert(record.session);
        }
        self.sessions = self.session_ids.len();
    }
}

fn source(
    host: &str,
    provider: &str,
    home: &Path,
    status: &str,
    scanned: u64,
    skipped: u64,
    malformed: u64,
    sessions: usize,
    message: Option<&str>,
) -> Value {
    json!({"fingerprint":{"hostId":host,"provider":provider,"resolvedHomePath":home.to_string_lossy(),"volumeId":volume_id(home)},"status":status,"scannedFiles":scanned,"skippedFiles":skipped,"malformedRecords":malformed,"distinctSessions":sessions,"message":message})
}
fn missing_source(host: &str, provider: &str, home: &Path, message: &str) -> Value {
    source(host, provider, home, "missing", 0, 0, 0, 0, Some(message))
}
fn failed_source(host: &str, provider: &str, home: &Path, message: &str) -> Value {
    source(host, provider, home, "failed", 0, 0, 0, 0, Some(message))
}

#[cfg(unix)]
fn volume_id(path: &Path) -> String {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path)
        .map(|metadata| format!("{}:{}", metadata.dev(), metadata.ino()))
        .unwrap_or_default()
}
#[cfg(not(unix))]
fn volume_id(_path: &Path) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_an_inverted_window_as_the_declared_error() {
        let error = UsageScan::from_payload(
            &json!({"sinceDay":"2026-08-10","untilDay":"2026-08-09","timeZone":"UTC"}),
        )
        .unwrap_err();
        assert_eq!(error["reason"], "invalidWindow");
    }
}
