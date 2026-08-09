# 04 — Usage receives API-equivalent pricing

**What to build:** The Usage report attaches upstream-compatible API-equivalent
cost and cache-savings estimates to Claude and Codex history without claiming
that subscription users paid those amounts. Provider-reported evidence wins,
known models use a cached LiteLLM rate table, and unavailable or unknown pricing
never prevents complete token reporting.

**Blocked by:** 02 — Claude history is counted once and cached; 03 — Codex history joins the report.

**Status:** ready-for-human

- [x] A provider-reported finite cost is used for its record and identified separately from model-priced cost.
- [x] Otherwise a valid LiteLLM model-price document supplies input, cached-input, cache-creation, output, and reasoning-compatible rates using upstream's model matching rules.
- [x] Unknown models remain in all token, record, session, provider, model, and day totals while contributing zero cost and an unpriced count.
- [x] Aggregated bucket cost sources correctly distinguish provider-reported, model-priced, and unpriced evidence.
- [x] Cache savings compares cached input at the full input rate with its actual cached-input rate and aggregates without affecting token totals.
- [x] A fetched rate table is cached in memory and privately on disk with upstream's 24-hour freshness rule and provenance timestamp.
- [x] A fresh table reports fresh provenance; an older usable disk table reports cached provenance when refresh fails; no usable table reports unavailable provenance.
- [x] A failed, timed-out, malformed, or empty pricing response does not fail transcript scanning or erase previously usable cached rates.
- [x] Cost is consistently labeled as raw API-equivalent token cost, not invoice, subscription spend, credits, or remaining quota.
- [x] The report can switch its headline, provider ranking, daily values, and chart metric between cost and processed tokens without mixing units.
- [x] Provider shares, model shares, cache savings, and unpriced coverage reconcile with the underlying buckets.
- [x] Tests use deterministic cached/local pricing documents and controlled failures; no focused test contacts LiteLLM or a provider service.
- [x] A real WebSocket fixture and rendered route demonstrate provider-reported, model-priced, and unpriced records together.
