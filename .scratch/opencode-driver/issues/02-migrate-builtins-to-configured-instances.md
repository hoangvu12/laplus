# 02 — Migrate built-in providers to configured instances

**What to build:** Claude and Codex run through the generic provider-instance
path introduced by ticket 01. Their default instance identities, settings,
catalogues, snapshots, refresh behavior and existing thread routing remain
compatible while the old registry still exists as a fallback.

**Blocked by:** 01 — Expand the provider-instance registry.

**Status:** done

- [x] The default Claude and Codex instances are represented by the generic
      registry with their existing durable instance ids
- [x] Existing settings decode into equivalent default-instance configuration
      without user migration work
- [x] Provider snapshots and targeted refreshes for both built-ins travel
      through the generic instance path
- [x] Existing threads reopen under the same driver and instance they recorded
- [x] Claude and Codex turns, catalogues and runtime retuning retain their
      current behavior
- [x] Compatibility tests prove old persisted settings and threads still work

**Where it landed.** The durable default ids, `claudeAgent` and `codex`, may now
name explicit `providerInstances` envelopes. Resolution prefers those envelopes
and falls back to the legacy `providers` buckets, so old settings and threads
need no rewrite while an edit in the current settings UI takes the generic
path. Claude and Codex snapshots, full and targeted refreshes, catalogues and
turn preparation all read the same resolved instance settings. Socket tests pin
both the explicit-default path and the legacy compatibility path. The existing
restart cases `a_conversation_and_its_transcript_survive_a_restart` and
`a_codex_thread_id_survives_a_restart_and_resumes_the_captured_context` remain
the compatibility proof that legacy Claude and Codex threads reopen with their
recorded driver and durable instance id.
