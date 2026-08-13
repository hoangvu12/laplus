# ADR-0055 — Provider catalogues are remembered per instance

Date: 2026-08-13
Status: Accepted

Laplus remembers the last successfully discovered model catalogue for each
provider instance and shows it provisionally while a fresh check runs. A failed
check retains that remembered catalogue with a warning; a successful check is
authoritative, including removals and an empty result. Changing the instance's
executable or server connection invalidates only that instance's memory. This
trades briefly stale choices for immediate and resilient startup, without ever
silently substituting another model when a remembered choice is rejected.
