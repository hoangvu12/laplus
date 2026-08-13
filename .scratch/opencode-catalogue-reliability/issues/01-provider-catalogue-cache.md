# 01 — Per-instance remembered provider catalogue

**What to build:** Persist and hydrate the last successful OpenCode model and
capability catalogue per provider instance, with ADR-0055's provisional and
authoritative replacement rules.

**Blocked by:** None — can start immediately.

**Status:** done

- [x] Add a schema-versioned, atomic, secret-free cache format and identity
      correlation for local and external OpenCode instances.
- [x] Hydrate remembered models into the first provider snapshot while using
      current settings for enabled state and authored custom models.
- [x] Expose checking/unverified state without disabling remembered choices.
- [x] Retain remembered models only for pending/transient installed-provider
      failures; replace exactly on successful, disabled, or missing results.
- [x] Refuse an unavailable remembered selection clearly without substituting.
- [x] Cover cache parsing, invalidation, merge semantics, restart, and the real
      configuration socket boundary with focused tests.

## Comments

- Added `provider-catalogues.json`, bounded and schema-versioned, with atomic
  replacement and per-instance driver/connection fingerprints. External URL
  credentials, passwords, authored custom models, skills, and auth state never
  enter the cache.
- The first socket snapshot publishes remembered models as `checking`; transient
  installed-provider failures retain them as `stale`, while successful discovery,
  disabled providers, and missing local executables remain authoritative.
- The web picker accepts remembered catalogues during checking/failure and avoids
  presenting the ordinary availability-warning banner while checking.
