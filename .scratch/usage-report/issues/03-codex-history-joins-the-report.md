# 03 — Codex history joins the report

**What to build:** A developer's Codex transcript history appears beside Claude
in the Usage report, including sessions created outside Laplus. Rolling Codex
token notifications are converted into non-duplicated deltas and attributed to
the model active for each turn, producing provider/model/day totals that agree
with the transcript's final usage.

**Blocked by:** 01 — An authorized Claude usage reaches a minimal report.

**Status:** ready-for-human

- [x] Codex transcript discovery honors the same shared/configured home layout as the Codex driver and scans session rollout files recursively.
- [x] Session metadata and turn context establish the session and current model carried into later token-count events.
- [x] Usage is taken from `last_token_usage` deltas, and identical consecutive token notifications are not counted twice.
- [x] A token notification arriving before its model context cannot poison deduplication of a later eligible copy.
- [x] Model switches inside one rollout attribute subsequent records to the new model.
- [x] Input, cached input, cache creation, output, and reasoning counts map to the common Usage report vocabulary.
- [x] Reasoning remains a subset of output and is never added on top of processed-token totals.
- [x] Missing session identifiers remain countable without causing unrelated sessions to collapse.
- [x] Malformed or irrelevant rows are skipped without losing later valid records, and Codex source status is reported independently from Claude.
- [x] The selected local-day window filters and buckets Codex records by the same rules as Claude.
- [x] The report visibly distinguishes Claude and Codex provider/model/day contributions and remains truthful when either provider source is missing or partial.
- [x] The real WebSocket fixture includes both provider formats and proves that only aggregates cross the wire.
- [x] Focused parser tests cover duplicates, pre-context events, model changes, reasoning semantics, missing session ids, and malformed rows without invoking a real Codex process.
