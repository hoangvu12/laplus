# 02 — A registry of drivers, and a conversation that names the one it ran under

**What to build:** Today one constant is both the driver slug and the only
instance id, so every conversation publishes that constant as the provider it ran
under — not because it is true but because there is no other answer to give.
That constant becomes a registry, and a conversation records and publishes which
driver it actually ran under.

A developer reopening a conversation sees the provider it was held under rather
than a default. That is the whole user-visible face of this ticket, and the
socket conformance suite already reads the field.

Settings gain a `codex` section alongside the existing one — a binary path, a
`CODEX_HOME`, and launch arguments — all of which the contract already declares.
Nothing reads them yet; ticket 03 is the first thing that does.

**`shadowHomePath` is refused rather than stored**, with a refusal naming the
reason. It is an account-selection setting and this server runs one Codex
account, so storing it would let a developer believe they are on an account they
are not — which is the exact failure ADR-0009's rule exists to prevent, even
though the field itself is one the contract knows. Refused at the moment of save,
not silently dropped: a developer who spends a week on the wrong account has been
failed by the setting that accepted their input.

**Blocked by:** 01.

**Status:** done

- [x] Driver slug and provider instance id are separate concepts; neither is a
      single compiled-in constant standing for both.
- [x] A conversation stores which driver ran it, and publishes that rather than a
      default — on the snapshot and on every rendering of a thread that carries a
      provider.
- [x] A conversation created before this ticket still reads correctly.
- [x] `settings.providers.codex` round-trips: a saved binary path, `CODEX_HOME`
      and launch arguments come back on the next read.
- [x] Saving `shadowHomePath` is refused, and the refusal says it is an
      account-selection setting this server cannot honour — not a generic
      "unknown field".
- [x] The existing socket conformance suite passes against the real field rather
      than a constant.

**Where it landed.** `crate::provider` now owns a registry whose entries separate
instance identity from driver kind. That identity is stored on every thread by a
v9 migration, with pre-existing rows backfilled to Claude, and the session fold
publishes the stored pair on snapshots, shell summaries and session events.
Dispatch binds the registered driver and its settings before publishing a turn,
then takes the model and runtime mode from the exact `TurnRequested` fold so a
second window cannot retune a turn in the gap.

`Settings.providers` now carries Codex's binary path, `CODEX_HOME`, launch
arguments and custom models. `shadowHomePath` is deliberately absent from the
stored shape and is refused as unsupported account selection. The conformance
declaration narrowed from the entire Codex section to that one absent field.
