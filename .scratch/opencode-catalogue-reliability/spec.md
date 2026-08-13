Status: done

# Reliable OpenCode catalogue at startup

Evidence: `.scratch/opencode-upstream-audit/research.md` and ADR-0055.

## Problem Statement

OpenCode's local catalogue can be slow on a cold start and can fail transiently
while its database is busy. Laplus starts each process without any previously
discovered models, runs model, agent, and skill discovery sequentially, and has
no retry for local commands. The model picker can therefore initially contain
only authored fallbacks or defaults even though connected models worked in the
previous run.

## Solution

Persist the last successful model catalogue separately for each provider
instance. Hydrate it before live discovery, mark it as provisional while
checking, and replace it exactly when a successful discovery answers. Retain it
with a visible warning when an installed provider's check fails. Local model
and agent discovery run concurrently; only failed calls retry once after a
short backoff, while skills remain best-effort enrichment.

## User Stories

1. I see my previously connected OpenCode models immediately after launch.
2. I can tell when the visible list is remembered and still being checked.
3. A transient check failure does not erase a previously working menu.
4. Disconnecting a model removes it after the next successful check.
5. Changing one OpenCode setup never shows models remembered from another.
6. A disappeared selected model produces a clear refusal and is never silently
   replaced by a different model.

## Decisions

- Cache only models and their model-specific capability options, including
  variants and agents. Authentication state, skills, messages, passwords, and
  maintenance state are not hydrated from this cache.
- Correlate cache entries with provider instance ID, driver, and a fingerprint
  of the executable or external-server connection identity. A changed identity
  invalidates that instance only.
- Remembered catalogues do not expire by age. They remain explicitly
  provisional until an authoritative answer arrives.
- Pending checks show a small `Checking OpenCode…` state without disabling the
  remembered models.
- An installed provider's transient discovery failure keeps remembered models
  and publishes an actionable warning. Disabled, missing, or successfully
  checked providers authoritatively replace/remove remembered models.
- A successful inventory is authoritative even when empty. Authored custom
  fallback models continue to come from current settings rather than the cache.
- Cache writes are schema-versioned, atomic, bounded, and contain no secret.
- `models --verbose` and `agent list` run concurrently. Failed calls retry once
  after approximately one second. Persistent model failure fails discovery;
  persistent agent failure keeps models without agent enrichment; skill
  discovery is best effort.

## Testing

- Cache tests cover missing, malformed, wrong-version, wrong-instance,
  wrong-driver, changed-connection, and valid files without exposing secrets.
- Merge tests cover pending, transient failure, successful replacement,
  authoritative empty/logout, removed models, disabled/missing providers, and
  current custom fallback settings.
- Discovery tests prove concurrency, selective one-time retry, authoritative
  model failure, and best-effort agent/skill behavior.
- A real socket/startup test proves remembered models are in the first config
  snapshot and later converge to live discovery.
- Drive the model picker in a rebuilt running application through remembered,
  checking, successful replacement, and failed-refresh states.

## Out of Scope

- Remembering authentication, skills, slash commands, prompts, or passwords.
- Automatically choosing another model when one disappears.
- A shared cache across provider instances or machines.
- OpenCode conversation stream recovery, tracked separately.
