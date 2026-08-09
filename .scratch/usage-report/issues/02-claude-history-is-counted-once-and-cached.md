# 02 — Claude history is counted once and cached

**What to build:** The Usage report becomes a truthful and repeatable history
of Claude Code consumption for 7-, 30-, and 90-day windows, including Claude
sessions created outside Laplus. Repeated transcript representations count
once, damaged or missing sources are described honestly, and warm reads reuse
persisted parsed records while changed files appear after refresh.

**Blocked by:** 01 — An authorized Claude usage reaches a minimal report.

**Status:** ready-for-human

- [x] Claude transcript discovery honors the same configured and default home resolution as the Claude driver and scans the provider's project transcripts recursively.
- [x] Valid assistant records separate uncached input, cache reads, cache creation, and output while accepting provider-reported cost for later pricing.
- [x] Repeated content blocks and copies carried through resumed or forked transcripts are globally deduplicated by upstream's message/request identity.
- [x] Records without a deduplication identity remain independently countable rather than collapsing unrelated activity.
- [x] Malformed, irrelevant, timestamp-less, model-less, and usage-less rows do not discard valid rows in the same file or source.
- [x] The caller's inclusive range and IANA time zone decide calendar-day buckets, including work around midnight and a DST boundary; unknown zones degrade as upstream does.
- [x] Missing, partially readable, and failed Claude sources are distinguished from an ok source, with bounded messages and truthful scanned/skipped/malformed/session counts.
- [x] Files too old to contribute are filtered conservatively before their contents are read.
- [x] Parsed per-file records persist by size and modification metadata, survive a process restart, and are reused only while the file is observably unchanged.
- [x] Appends, rewrites, truncation, removal, and corrupt persisted cache data cannot retain stale usage or prevent valid records from being recovered.
- [x] Cache entries beyond the longest report window are pruned, and manual Refresh invalidates the environment query so new records become visible.
- [x] The report exposes 7-, 30-, and 90-day controls and visibly reports Claude source coverage without exposing transcript content.
- [x] A real WebSocket fixture and focused parser, aggregation, and cache tests cover the behavior without elapsed-time assertions or live provider/network dependencies.
