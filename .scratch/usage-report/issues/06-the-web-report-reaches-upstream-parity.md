# 06 — The web report reaches upstream parity

**What to build:** The responsive web and desktop Usage report reaches the
observable layout, wording, interaction, and coverage behavior of T3 Code at the
selected upstream snapshot. A developer can move from headline cost or tokens
through provider trends and model/day detail, understand incomplete coverage,
refresh every environment, and use the complete report at desktop or narrow web
widths.

**Blocked by:** 04 — Usage receives API-equivalent pricing; 05 — Connected environments merge without double counting.

**Status:** ready-for-human

- [x] The page matches upstream's header, inclusive date subtitle, Back, Refresh, and 7/30/90-day controls.
- [x] Direct navigation has a useful Back fallback, ordinary Back returns through history, and the sidebar entry behaves correctly on desktop and mobile-width navigation.
- [x] Pending or partially settled environments show the upstream loading skeleton and device reporting strip rather than jumping totals.
- [x] The headline follows the selected cost/token chart metric and uses upstream's API-equivalent cost disclaimer and token/session wording.
- [x] Provider rows are ranked by the selected metric and show the correct mark, label, share bar, complementary metric, and stable color.
- [x] The daily provider chart supports cost and token metrics, readable axes/tooltips, empty days, all offered windows, and the upstream legend.
- [x] The metric strip shows processed tokens, cached input, uncached input, output/reasoning, and cache savings with reconciled details.
- [x] The Breakdown control switches between the upstream model table and recent-day table with correct provider, cost, share, and token values.
- [x] Empty activity, unavailable pricing, unknown models, duplicate sources, missing/partial sources, stale versions, and failed environments use upstream-compatible copy and do not imply complete data.
- [x] Layout and interaction remain usable in an ordinary browser, the desktop shell, and a narrow responsive viewport without adding a native mobile application.
- [x] Shared formatting and merge behavior live behind explicit package subpath exports, contracts remain schema-only, and presentation does not parse raw transcripts or pricing documents.
- [x] Focused component/chart tests cover interactions and representative values without asserting incidental DOM structure or implementation details.
- [x] The final UI-driver walkthrough runs against a real fixture server containing both providers, multiple pricing sources, and representative coverage; it verifies rendered values against the wire and proves raw transcript text is absent.
- [x] The current web bundle is rebuilt before desktop verification, and every development server or watcher is stopped afterward.
- [x] Targeted TypeScript and Rust formatting, lint, type, and test checks pass; the full workspace suite remains CI's responsibility.
- [x] Final behavior is compared against upstream commit `1a003e38`, with any necessary Rust-only implementation difference documented without changing observable semantics.
