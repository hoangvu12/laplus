# ADR-0056 — OpenCode turns reconcile after stream loss

Date: 2026-08-13
Status: Accepted

An interrupted OpenCode event stream does not by itself complete, fail, or
repeat a turn. Laplus visibly enters turn recovery, asks OpenCode for the
session's current state and messages, merges only missing output, and
resubscribes while the provider remains busy; recovery continues until the
provider settles or the developer stops it. Missing sessions and terminal
authentication or protocol errors fail visibly while preserving partial work,
and no recovery path resends the developer's prompt. This favors durable,
idempotent recovery over a time limit that could misclassify healthy long work.

An interrupt uses the same recovery machinery but does not trust session
status as proof of completion. Laplus compares bounded snapshots of assistant
message output and settles only after two samples agree. Inspection failures
and a still-changing external server remain supervised instead of ending the
conversation; owned-server escalation is ADR-0058.
