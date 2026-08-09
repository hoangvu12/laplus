Status: ready-for-agent

# Historical provider usage report

## Problem Statement

Laplus shows how full one active conversation's context window is, but it does
not answer how much Claude Code and Codex a developer has used over time. A
developer cannot compare providers or models, see daily trends, understand the
effect of prompt caching, or estimate the API-equivalent value of work performed
through subscription-backed CLIs. The missing history is broader than Laplus's
own threads: both CLIs may also be used directly or through other tools.

T3 Code now provides this answer by scanning the providers' local transcripts.
Laplus cannot copy only that page because its server is Rust and deliberately no
longer synchronizes with T3 Code's TypeScript server. It needs the same contract,
observable behavior, privacy boundary, and web experience implemented through
Laplus's own server and authorization architecture.

## Solution

Add a responsive Usage report to the shared web application used in browsers
and by the desktop shell. Each connected environment reads its own Claude Code
and Codex transcript directories, deduplicates provider records, and returns
aggregated daily provider/model buckets for the selected 7-, 30-, or 90-day
window. Raw transcripts never cross the WebSocket.

The page merges summaries across environments without double-counting the same
physical transcript source. It presents processed tokens, cache behavior,
output and reasoning tokens, provider and model breakdowns, daily charts, and
estimated API-equivalent cost. Cost uses provider-reported figures when present
and a cached LiteLLM rate table otherwise; missing pricing degrades honestly to
unpriced token usage. The feature copies the observable behavior and wording of
T3 Code at upstream commit `1a003e38`, while the scanning service, caches,
deferred RPC work, and authorization seam are native to Laplus.

## User Stories

1. As a developer, I want a Usage entry in the sidebar, so that historical provider consumption is discoverable.
2. As a developer, I want to open the Usage report from either a browser or the desktop shell, so that both Laplus surfaces show the same information.
3. As a developer on a narrow browser, I want the report to remain usable, so that I can inspect usage from a phone without a native mobile app.
4. As a developer, I want the report to include Claude Code activity, so that I can understand how much Claude work my local transcripts represent.
5. As a developer, I want the report to include Codex activity, so that I can understand how much Codex work my local transcripts represent.
6. As a developer, I want activity created outside Laplus included, so that the report describes provider usage rather than only Laplus usage.
7. As a privacy-conscious developer, I want raw prompts, responses, tool output, and transcript rows to remain on their source environment, so that viewing totals does not copy conversation content to another device.
8. As a developer, I want to choose 7-, 30-, or 90-day windows, so that I can inspect recent changes or longer trends.
9. As a developer, I want the displayed dates bucketed in my browser's time zone, so that work near midnight appears on the day I experienced it.
10. As a developer, I want the inclusive reporting range shown explicitly, so that I know which days contribute to every total.
11. As a developer, I want repeated Claude transcript rows counted once, so that content-block repetition, resumed sessions, and forks do not inflate usage.
12. As a developer, I want repeated Codex token-count notifications counted once, so that stream boundaries do not inflate usage.
13. As a developer, I want Codex usage attributed to the model active for that turn, so that sessions which switch models produce truthful breakdowns.
14. As a developer, I want malformed or irrelevant transcript rows skipped without losing valid rows, so that one damaged line does not destroy the report.
15. As a developer, I want missing transcript directories shown as missing sources rather than failures, so that an uninstalled provider does not make the page look broken.
16. As a developer, I want partially readable transcript sources identified, so that totals never pretend to be complete when files were skipped.
17. As a developer, I want a source failure reported without hiding successful environments, so that one offline or incompatible machine does not erase all usage.
18. As a developer with several connected environments, I want the page to wait for terminal answers before revealing totals, so that numbers do not jump as machines report.
19. As a developer with several servers reading the same provider home, I want the physical source counted once, so that worktrees or duplicate connections do not double usage.
20. As a developer with genuinely distinct machines, I want identical-looking home paths counted independently, so that fleet machines do not collapse into one source.
21. As a developer, I want processed-token totals to distinguish uncached input, cached input, cache creation, output, and reasoning, so that I can understand workload composition.
22. As a developer, I want reasoning tokens treated as part of output rather than added twice, so that Codex totals reconcile correctly.
23. As a developer, I want usage grouped by provider, model, and day, so that I can compare where consumption comes from.
24. As a developer, I want the daily chart switchable between tokens and cost, so that the graph and headline answer the same selected question.
25. As a developer, I want provider shares ranked by the selected metric, so that the largest contributor is immediately visible.
26. As a developer, I want model and recent-day breakdown tables, so that I can inspect the detail behind the headline.
27. As a developer, I want explicit-cost data from a provider preferred when available, so that the estimate uses the strongest evidence present.
28. As a developer, I want known models priced against the same LiteLLM table as T3 Code, so that Laplus reports comparable API-equivalent cost.
29. As a subscription user, I want cost labeled as API-equivalent rather than money billed, so that the report does not misrepresent my subscription invoice.
30. As a developer, I want unrecognized models included in token totals and marked unpriced, so that incomplete pricing never becomes missing usage.
31. As an offline developer, I want a cached pricing table used when refresh fails, so that cost estimates remain available without network access.
32. As an offline developer without a pricing cache, I want token reporting to remain fully usable, so that a network dependency cannot break the report.
33. As a developer, I want estimated cache savings shown, so that the value of cached input is visible.
34. As a developer, I want report refresh to rescan every connected environment, so that newly written transcripts appear on demand.
35. As a developer, I want warm refreshes to reuse unchanged transcript parsing, so that repeatedly opening the page does not reread gigabytes of history.
36. As a developer, I want changed or truncated transcript files reparsed safely, so that cache reuse cannot preserve stale or deleted records.
37. As a developer, I want old scan-cache entries pruned beyond the longest supported window, so that the report cache does not grow without bound.
38. As a developer, I want the page to show when some environments are still reporting, stale, duplicated, or failed, so that coverage is explicit.
39. As a developer, I want a back control on the report, so that I can return to the page I came from.
40. As a developer who opens the route directly, I want Back to return to the main application, so that the control always has a useful destination.
41. As a paired user with `orchestration:read`, I want to read the Usage report remotely, so that legitimate read access works across devices.
42. As a paired user without `orchestration:read`, I want the Usage RPC refused without transcript-derived details, so that deliberately limited sessions do not gain financial or activity metadata.
43. As a maintainer, I want transcript format drift isolated to provider parsers, so that one provider change does not corrupt the other's report.
44. As a maintainer, I want the contract version carried with every environment summary, so that mixed Laplus versions can report partial compatibility honestly.
45. As a maintainer, I want the feature verified through the real WebSocket contract and real web route, so that passing unit tests do not conceal an unusable application.

## Implementation Decisions

- Observable behavior, copy, interaction, coverage semantics, aggregation rules, and visual structure follow T3 Code upstream commit `1a003e38`. Laplus does not add synchronization machinery; the port becomes maintained Laplus code.
- The canonical feature name is **Usage report**. It means historical provider consumption. It is distinct from an active conversation's context-window reading and from an account's current usage-limit **standing**.
- The report supports Claude Code and Codex only. OpenCode, Cursor, Grok, and other provider kinds are not silently represented as zero-usage providers.
- Each environment scans the provider CLIs' own transcript homes rather than Laplus thread projections. Provider-home resolution honors the same configured/default Claude and Codex homes used by the corresponding drivers.
- Raw transcript records remain inside the environment that owns them. The RPC returns only versioned, pre-aggregated day/provider/model buckets, source coverage and fingerprints, pricing provenance, and scan diagnostics.
- The shared contract introduces the upstream usage schemas, contract version, typed read error, and `server.getUsageSummary` unary RPC. Invalid windows and scan failures are declared errors rather than connection defects.
- The RPC is deferred disk work so a large scan cannot block the WebSocket read loop, subscription acknowledgements, pings, or unrelated calls.
- `server.getUsageSummary` requires `orchestration:read`, matching upstream. A reusable method-authorization seam carries the authenticated grant to RPC dispatch. Per ADR-0054, this does not retroactively assign scopes to unrelated methods.
- Claude parsing accepts assistant records with valid timestamps, models, and usage objects; separates uncached input, cache reads, cache creation, and output; accepts provider-reported cost; and globally deduplicates the message/request identity used by T3 Code.
- Codex parsing carries session and model state forward through a rollout, consumes `last_token_usage` deltas from token-count events, skips consecutive duplicate payloads, and keeps reasoning tokens as a subset of output.
- Records are bucketed to inclusive calendar days in the caller's IANA time zone. An unknown zone degrades consistently with upstream rather than making all transcript usage unreadable.
- Aggregation is by day, provider kind, and model. Buckets include token categories, cost, cache savings, pricing source, record counts, unpriced counts, and distinct contributing sessions.
- Source coverage distinguishes ok, missing, partial, and failed. A fingerprint combines host identity, provider kind, resolved provider home, and filesystem volume identity so the client can remove duplicate physical sources without merging distinct machines that share a path.
- Transcript files are filtered conservatively by modification time before opening. Parsed per-file records are cached by size and modification time, persisted in Laplus state, invalidated when a file changes or shrinks, and pruned beyond the longest supported reporting window.
- Pricing uses the upstream LiteLLM model-price document. A fresh document is cached in memory and on disk for 24 hours; a stale disk copy is an explicit cached source; failure with no usable copy produces unavailable pricing without failing token aggregation.
- Provider-reported cost wins for records that carry it. Otherwise exact or upstream-compatible model matching uses the LiteLLM rates. Unknown rates yield unpriced records whose tokens remain in every token total and whose cost contribution is zero.
- Cost is always described as raw API-equivalent token cost, not an amount paid under Claude or ChatGPT subscriptions. Cache savings compares cached-input pricing with full input pricing and is also an estimate.
- The client queries every connected environment for the same window and merges compatible summaries. It waits until every non-failed environment reaches a terminal result before revealing totals, preventing headline values from jumping as devices answer.
- Mixed contract versions, duplicate source fingerprints, missing sources, partial sources, stale environments, and failed environments produce coverage notices rather than silently complete-looking totals.
- The report route is a root web route reachable from the sidebar. It includes upstream's Back and Refresh controls, range control, cost/token chart control, provider legend, metrics, provider/model/day breakdowns, coverage notices, and loading skeleton.
- The web implementation is responsive and is the only mobile-width experience. The Tauri desktop shell embeds the same built web route. No native mobile application is introduced.
- Existing explicit subpath-export and schema-only package boundaries remain intact: contracts contain schemas, shared code contains pure merge/format behavior, client runtime contains environment query atoms, the web app owns presentation, and the Rust server owns disk/network runtime behavior.
- The contract-parity ledger is re-derived when the method lands; no new method count is copied into prose elsewhere.

## Testing Decisions

- Good tests assert behavior visible at a public seam: RPC values and errors, persisted cache behavior across calls or restarts, and browser-visible state. They do not assert private helper calls, internal collection shapes, or elapsed wall-clock speed.
- The primary seam is a real Laplus WebSocket integration test with temporary Claude and Codex transcript homes plus a deterministic cached pricing document. It calls `server.getUsageSummary` and covers discovery, parsing, deduplication, local-day aggregation, model/provider attribution, pricing, source status, cache reuse and invalidation, authorization, typed errors, and wire decoding.
- Authorization coverage uses real grants: a session with `orchestration:read` succeeds and a session without it receives the contract's scope-required error without any summary data. Existing pairing and public-exposure authorization tests are prior art for grant construction and non-disclosing refusals.
- Focused pure parser tables cover combinatorial provider records that would make the integration fixture unreadable: malformed values, repeated Claude content blocks and cross-file duplicates, Claude reported cost, Codex duplicate token events, token events before model context, model changes, missing session ids, and reasoning-as-output semantics.
- Focused aggregation tests cover inclusive window edges, IANA time-zone day boundaries including DST, global deduplication, session cardinality, mixed pricing sources, cache savings, unknown models, and stable bucket ordering.
- Cache tests use temporary files and controlled metadata changes to cover warm reads, appends, same-size rewrites where observable metadata changes, truncation, missing files, corrupt persisted cache data, and retention pruning. Assertions concern returned records and persisted public cache behavior, not timing.
- Contract tests decode success summaries, source states, pricing states, and both declared error reasons. Client merge tests cover duplicate physical sources, genuinely distinct hosts with identical paths, mixed contract versions, partial/failed environments, provider shares, daily/model totals, and cost-quality arithmetic.
- The user-visible seam drives `/usage` in the real web application against the fixture server at desktop and narrow web widths. It verifies sidebar navigation, direct-route fallback, Back, 7/30/90-day ranges, refresh, cost/token toggles, model/day breakdowns, loading/coverage/error states, and representative chart/table values.
- The browser walkthrough inspects the RPC payload or captured wire alongside rendered labels to prove that raw transcript text never leaves the server and that the displayed totals derive from the fixture records.
- User-visible verification follows the repository rule that a green suite is insufficient: build the current web bundle, run the server and UI driver, exercise the route for at least one Claude and one Codex source, and stop every dev server or watcher afterward.
- Network access is not required by focused tests. Pricing tests use a deterministic local or cached document; fresh/cached/unavailable behavior is controlled without contacting LiteLLM or provider services.
- Verification remains focused: contract/shared/client/web tests for changed modules, targeted TypeScript formatting/lint/type checks, targeted Rust tests, and the UI-driver walkthrough. The full workspace suite remains CI's responsibility.

## Out of Scope

- OpenCode, Cursor, Grok, ACP agents, or any provider beyond Claude Code and Codex.
- Subscription quota, remaining allowance, rate-limit reset windows, invoices, credits, or actual money charged.
- Replacing the active conversation context-window meter or account usage-limit standing activities.
- A native mobile application or any new application surface beyond the responsive web client and its desktop-shell embedding.
- Uploading, synchronizing, retaining, searching, or displaying raw transcript content through the Usage feature.
- Reconstructing usage solely from Laplus's database, thread projections, or currently running sessions.
- An ongoing upstream remote, subtree, vendored server, automated sync, or general T3 Code merge.
- Expanding usage reporting to project-, repository-, thread-, user-, or provider-instance-level attribution not present upstream.
- Retrofitting method-specific scope enforcement onto every existing WebSocket RPC as part of this feature.
- Guaranteeing cost accuracy for unknown models or representing API-equivalent estimates as subscription spend.
- Turning the LiteLLM rate source into a general package-management or pricing-administration feature.

## Further Notes

- Upstream implementation provenance is T3 Code feature commit `8101cd04`, its web fixes `a20923ce`, `70c423a5`, and `886195ec`, and the selected current snapshot `1a003e38` from 2026-08-09.
- ADR-0014 and the server documentation establish why the TypeScript server is evidence rather than a dependency. The Usage service must be re-expressed in Rust while preserving the upstream contract and behavior.
- ADR-0054 records why the transcript-derived Usage report becomes Laplus's first method-scoped WebSocket read.
- The glossary defines **Usage report**, **standing**, provider instance, and environment-adjacent vocabulary. Tickets should use those terms rather than overloading “usage” to mean context-window fill or remaining quota.
- The first implementation ticket should be a narrow tracer bullet through contract, authorized deferred RPC, one deterministic transcript record, and a minimal rendered route. Later tickets deepen provider parsing, caching/pricing, multi-environment merging, and complete visual parity without building disconnected layers first.
