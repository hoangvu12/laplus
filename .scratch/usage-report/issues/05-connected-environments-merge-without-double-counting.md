# 05 — Connected environments merge without double counting

**What to build:** The Usage report combines every connected environment into
one stable answer while making coverage explicit. Two servers reading the same
physical provider transcript home contribute once, genuinely distinct machines
remain distinct even when their paths look alike, and totals do not jump while
non-failed environments are still answering.

**Blocked by:** 02 — Claude history is counted once and cached; 03 — Codex history joins the report.

**Status:** ready-for-human

- [x] Every provider source carries a fingerprint containing host identity, provider kind, resolved home, and filesystem volume identity when available.
- [x] Two environment summaries naming the same physical provider source are deduplicated before totals are merged.
- [x] Sources on distinct hosts or filesystem volumes remain distinct even when hostname or resolved path text is identical.
- [x] Claude and Codex sources are fingerprinted independently and cannot suppress one another.
- [x] Compatible summaries merge buckets by day, provider, and model while preserving token categories, costs, savings, records, and correct distinct-session totals.
- [x] A session spanning days or models is not overcounted by summing per-bucket session cardinalities.
- [x] Environments with an older incompatible Usage contract version are identified as stale/incompatible rather than poisoning or silently joining current totals.
- [x] Missing, partial, failed, duplicate, and stale coverage is visible with the affected environment/source labels and bounded messages.
- [x] The report withholds merged headline values while any non-failed environment is still answering, then reveals one settled set of totals.
- [x] A failed environment does not leave the page perpetually loading or hide successful environments.
- [x] Refresh requests a fresh summary from every presented environment rather than refreshing only the derived merged atom.
- [x] Client merge tests cover duplicate physical sources, identical paths on distinct machines, mixed contract versions, failures, settling, provider shares, daily totals, model totals, and session cardinality.
- [x] A multi-environment WebSocket/browser fixture demonstrates settled totals and coverage notices without exposing raw records from either environment.
