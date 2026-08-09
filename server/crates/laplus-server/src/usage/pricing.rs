//! LiteLLM rate parsing, usage pricing, and the private rate-table cache.
//!
//! Network I/O is deliberately injected. The Usage service supplies the real
//! fetcher while tests can prove freshness and fallback without contacting a
//! provider or LiteLLM.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const RATES_TTL_MS: i64 = 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ModelRate {
    pub(crate) input_cost_per_token: f64,
    pub(crate) output_cost_per_token: f64,
    pub(crate) cache_read_cost_per_token: f64,
    pub(crate) cache_creation_cost_per_token: f64,
}

pub(crate) type RateTable = BTreeMap<String, ModelRate>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CostSource {
    ProviderReported,
    ModelPriced,
    Unpriced,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PricedUsage {
    pub(crate) cost_usd: f64,
    pub(crate) source: CostSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PricingStatus {
    Fresh,
    Cached,
    Unavailable,
}

impl PricingStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Cached => "cached",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PricingCache {
    rates: RateTable,
    fetched_at_ms: Option<i64>,
    status: PricingStatus,
    loaded_disk: bool,
}

impl Default for PricingCache {
    fn default() -> Self {
        Self {
            rates: RateTable::new(),
            fetched_at_ms: None,
            status: PricingStatus::Unavailable,
            loaded_disk: false,
        }
    }
}

impl PricingCache {
    pub(crate) fn rates(&self) -> &RateTable {
        &self.rates
    }

    pub(crate) fn status(&self) -> PricingStatus {
        self.status
    }

    pub(crate) fn fetched_at_ms(&self) -> Option<i64> {
        self.fetched_at_ms
    }

    /// Ensures the cache has the freshest usable table available.
    ///
    /// A fresh memory or disk table requires no fetch. A stale usable table is
    /// retained when fetching fails or returns an invalid/empty document.
    pub(crate) fn ensure<F, E>(&mut self, path: &Path, now_ms: i64, fetch: F)
    where
        F: FnOnce() -> Result<Value, E>,
    {
        if self.is_fresh(now_ms) {
            self.status = PricingStatus::Fresh;
            return;
        }

        if !self.loaded_disk {
            self.loaded_disk = true;
            if let Some(cached) = load_cache(path) {
                let rates = parse_rate_table(&cached.document);
                if !rates.is_empty() {
                    self.rates = rates;
                    self.fetched_at_ms = Some(cached.fetched_at_ms);
                    self.status = if is_fresh(cached.fetched_at_ms, now_ms) {
                        PricingStatus::Fresh
                    } else {
                        PricingStatus::Cached
                    };
                    if self.status == PricingStatus::Fresh {
                        return;
                    }
                }
            }
        }

        if let Ok(document) = fetch() {
            let rates = parse_rate_table(&document);
            if !rates.is_empty() {
                self.rates = rates;
                self.fetched_at_ms = Some(now_ms);
                self.status = PricingStatus::Fresh;
                save_cache(
                    path,
                    &DiskCache {
                        fetched_at_ms: now_ms,
                        document,
                    },
                );
                return;
            }
        }

        self.status = if self.rates.is_empty() {
            PricingStatus::Unavailable
        } else {
            PricingStatus::Cached
        };
    }

    fn is_fresh(&self, now_ms: i64) -> bool {
        !self.rates.is_empty()
            && self
                .fetched_at_ms
                .is_some_and(|fetched| is_fresh(fetched, now_ms))
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiskCache {
    fetched_at_ms: i64,
    document: Value,
}

fn is_fresh(fetched_at_ms: i64, now_ms: i64) -> bool {
    now_ms.saturating_sub(fetched_at_ms) < RATES_TTL_MS
}

fn load_cache(path: &Path) -> Option<DiskCache> {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn save_cache(path: &Path, cache: &DiskCache) {
    let Ok(encoded) = serde_json::to_vec(cache) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, encoded);
}

pub(crate) fn parse_rate_table(document: &Value) -> RateTable {
    let mut rates = RateTable::new();
    let Some(entries) = document.as_object() else {
        return rates;
    };
    for (name, value) in entries {
        let Some(entry) = value.as_object() else {
            continue;
        };
        let Some(input) = finite(entry.get("input_cost_per_token")) else {
            continue;
        };
        let Some(output) = finite(entry.get("output_cost_per_token")) else {
            continue;
        };
        rates.insert(
            normalize_model_name(name),
            ModelRate {
                input_cost_per_token: input,
                output_cost_per_token: output,
                cache_read_cost_per_token: finite(entry.get("cache_read_input_token_cost"))
                    .unwrap_or(input),
                cache_creation_cost_per_token: finite(entry.get("cache_creation_input_token_cost"))
                    .unwrap_or(input),
            },
        );
    }
    rates
}

fn finite(value: Option<&Value>) -> Option<f64> {
    value
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite())
}

pub(crate) fn normalize_model_name(model: &str) -> String {
    model
        .trim()
        .to_lowercase()
        .rsplit_once('/')
        .map_or_else(|| model.trim().to_lowercase(), |(_, name)| name.to_string())
}

pub(crate) fn lookup_rate<'a>(table: &'a RateTable, model: &str) -> Option<&'a ModelRate> {
    let normalized = normalize_model_name(model);
    if normalized.is_empty()
        || matches!(
            normalized.as_str(),
            "<synthetic>" | "synthetic" | "opus" | "sonnet" | "haiku" | "fable"
        )
    {
        return None;
    }
    table.get(&normalized)
}

/// Prices token categories. Reasoning is intentionally absent because it is a
/// subset of output rather than an additional billable category.
pub(crate) fn price_usage(
    table: &RateTable,
    model: &str,
    uncached_input: u64,
    cached_input: u64,
    cache_creation: u64,
    output: u64,
    reported_cost_usd: Option<f64>,
) -> PricedUsage {
    if let Some(cost) = reported_cost_usd.filter(|cost| cost.is_finite()) {
        return PricedUsage {
            cost_usd: cost,
            source: CostSource::ProviderReported,
        };
    }
    let Some(rate) = lookup_rate(table, model) else {
        return PricedUsage {
            cost_usd: 0.0,
            source: CostSource::Unpriced,
        };
    };
    PricedUsage {
        cost_usd: uncached_input as f64 * rate.input_cost_per_token
            + cached_input as f64 * rate.cache_read_cost_per_token
            + cache_creation as f64 * rate.cache_creation_cost_per_token
            + output as f64 * rate.output_cost_per_token,
        source: CostSource::ModelPriced,
    }
}

pub(crate) fn cache_savings_usd(table: &RateTable, model: &str, cached_input: u64) -> f64 {
    lookup_rate(table, model).map_or(0.0, |rate| {
        cached_input as f64 * (rate.input_cost_per_token - rate.cache_read_cost_per_token)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "anthropic/Claude-Fable-5": {
                "input_cost_per_token": 0.00001,
                "output_cost_per_token": 0.00005,
                "cache_read_input_token_cost": 0.000001,
                "cache_creation_input_token_cost": 0.0000125
            },
            "fallback-cache-rates": {
                "input_cost_per_token": 0.01,
                "output_cost_per_token": 0.02
            },
            "half-priced": { "input_cost_per_token": 0.01 },
            "not-finite": {
                "input_cost_per_token": 0.01,
                "output_cost_per_token": "NaN"
            }
        })
    }

    #[test]
    fn parses_complete_rates_and_defaults_optional_cache_rates_to_input() {
        let table = parse_rate_table(&document());
        assert_eq!(table.len(), 2);
        assert_eq!(
            lookup_rate(&table, "FALLBACK-CACHE-RATES"),
            Some(&ModelRate {
                input_cost_per_token: 0.01,
                output_cost_per_token: 0.02,
                cache_read_cost_per_token: 0.01,
                cache_creation_cost_per_token: 0.01,
            })
        );
    }

    #[test]
    fn matching_is_exact_after_case_and_provider_prefix_normalization() {
        let table = parse_rate_table(&document());
        assert!(lookup_rate(&table, "openai/claude-fable-5").is_some());
        assert!(lookup_rate(&table, "claude-fable").is_none());
        for generic in [
            "",
            "<synthetic>",
            "synthetic",
            "opus",
            "sonnet",
            "haiku",
            "fable",
        ] {
            assert!(lookup_rate(&table, generic).is_none(), "{generic}");
        }
    }

    #[test]
    fn provider_cost_wins_otherwise_rates_price_each_non_reasoning_category() {
        let table = parse_rate_table(&document());
        let reported = price_usage(&table, "unknown", 100, 1_000, 10, 50, Some(1.25));
        assert_eq!(reported.source, CostSource::ProviderReported);
        assert_eq!(reported.cost_usd, 1.25);

        let priced = price_usage(&table, "claude-fable-5", 100, 1_000, 10, 50, None);
        assert_eq!(priced.source, CostSource::ModelPriced);
        assert!((priced.cost_usd - 0.004625).abs() < 1e-12);
        assert!((cache_savings_usd(&table, "claude-fable-5", 1_000) - 0.009).abs() < 1e-12);

        let unknown = price_usage(&table, "unknown", 100, 1_000, 10, 50, None);
        assert_eq!(
            unknown,
            PricedUsage {
                cost_usd: 0.0,
                source: CostSource::Unpriced
            }
        );
    }

    #[test]
    fn a_fresh_fetch_is_persisted_and_reused_from_disk_without_fetching() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("private/usage-model-rates.json");
        let mut first = PricingCache::default();
        first.ensure(&path, 1_000, || Ok::<_, ()>(document()));
        assert_eq!(first.status(), PricingStatus::Fresh);
        assert_eq!(first.fetched_at_ms(), Some(1_000));

        let mut restarted = PricingCache::default();
        restarted.ensure(&path, 2_000, || -> Result<Value, ()> {
            panic!("fresh disk cache fetched")
        });
        assert_eq!(restarted.status(), PricingStatus::Fresh);
        assert_eq!(restarted.rates().len(), 2);
    }

    #[test]
    fn stale_disk_rates_survive_failed_malformed_and_empty_refreshes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage-model-rates.json");
        save_cache(
            &path,
            &DiskCache {
                fetched_at_ms: 1,
                document: document(),
            },
        );
        let stale_now = RATES_TTL_MS + 2;

        for response in [Err(()), Ok(json!(null)), Ok(json!({}))] {
            let mut cache = PricingCache::default();
            cache.ensure(&path, stale_now, || response);
            assert_eq!(cache.status(), PricingStatus::Cached);
            assert_eq!(cache.rates().len(), 2);
            assert_eq!(cache.fetched_at_ms(), Some(1));
        }
    }

    #[test]
    fn failure_without_a_usable_table_is_unavailable_and_memory_obeys_the_ttl() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("usage-model-rates.json");
        let mut cache = PricingCache::default();
        cache.ensure(&path, 1_000, || Err::<Value, _>(()));
        assert_eq!(cache.status(), PricingStatus::Unavailable);

        cache.ensure(&path, 2_000, || Ok::<_, ()>(document()));
        assert_eq!(cache.status(), PricingStatus::Fresh);
        cache.ensure(&path, 2_000 + RATES_TTL_MS - 1, || -> Result<Value, ()> {
            panic!("fresh memory cache fetched")
        });
        assert_eq!(cache.status(), PricingStatus::Fresh);
        cache.ensure(&path, 2_000 + RATES_TTL_MS, || Err::<Value, _>(()));
        assert_eq!(cache.status(), PricingStatus::Cached);
    }
}
