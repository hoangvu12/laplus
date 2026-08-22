Status: ready-for-agent

# 02 — Part-keyed merge for interrupt & stream-loss reconcile

**What to build:** When laplus reconciles an interrupted turn — or otherwise
merges provider history it missed — missing text blocks are inserted into
their **own** messages in provider order, extending the matching part-keyed
message rather than appending onto one accumulated string. A recovered
transcript looks like a live one: text lands between the tools where it was
said, and what was already on screen is never duplicated or rewritten.

This builds on ticket 01's per-part model: today's reconcile compares REST
history against a single accumulated string; here it addresses parts instead.

**Blocked by:** 01 — One assistant message per OpenCode text part.

**Status:** ready-for-agent

- [ ] Against the scripted peer: reconcile after a lost suffix inserts the
      missing block as a new message positioned after the tool row, not glued
      to the pre-tool message.
- [ ] Text already shown is left byte-identical; a divergent snapshot cannot
      retract on-screen text (existing rule preserved, now per part).
- [ ] Reconcile is idempotent: running it twice against the same history
      produces one copy of each block.
- [ ] Parts absent locally are inserted in provider order between existing
      rows; ordinals keep reload consistent with live.
- [ ] An interrupted turn whose partial last block gains a REST suffix closes
      with the extended text under the same part identity.
- [ ] Focused tests pass: the interrupt-reconcile scenarios in the OpenCode
      socket/protocol suites, extended for multi-part histories.
