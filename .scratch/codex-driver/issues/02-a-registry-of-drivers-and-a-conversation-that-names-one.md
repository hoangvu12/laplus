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

**Status:** ready-for-agent

- [ ] Driver slug and provider instance id are separate concepts; neither is a
      single compiled-in constant standing for both.
- [ ] A conversation stores which driver ran it, and publishes that rather than a
      default — on the snapshot and on every rendering of a thread that carries a
      provider.
- [ ] A conversation created before this ticket still reads correctly.
- [ ] `settings.providers.codex` round-trips: a saved binary path, `CODEX_HOME`
      and launch arguments come back on the next read.
- [ ] Saving `shadowHomePath` is refused, and the refusal says it is an
      account-selection setting this server cannot honour — not a generic
      "unknown field".
- [ ] The existing socket conformance suite passes against the real field rather
      than a constant.
