# 02 — Regenerate an existing thread title

**What to build:** A developer can ask Laplus to regenerate the current thread's
title from its conversation history. The action shows canonical pending state,
commits the generated result through the ordinary thread metadata stream, and
preserves the current title on failure. Concurrent requests and manual edits
resolve predictably: only the newest still-valid generation may win.

**Blocked by:** 01 — Automatically improve first-turn titles.

**Status:** ready-for-human

- [x] The shared thread action policy offers Regenerate title for an eligible thread.
- [x] The request generates from the existing conversation and current title rather than only the first prompt.
- [x] Pending state is visible and consistent in every connected client.
- [x] A successful result updates every title surface and survives restart.
- [x] Failure clears the matching pending request, retains the current title, and reports the problem.
- [x] Starting a newer regeneration prevents an older completion from winning.
- [x] A manual rename or provider-native title update supersedes an in-flight regeneration.
- [x] Blank generated results do not replace the current title.
- [x] Socket coverage drives success, failure, overlapping requests, and manual-rename supersession through public commands and subscriptions.
- [x] The workflow is driven in a real window, including a manual rename while regeneration is pending.
