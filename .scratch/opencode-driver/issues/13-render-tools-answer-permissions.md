# 13 — Render tools and answer permissions

**What to build:** OpenCode tool work appears in the shared work log and its
permission requests can be answered from Laplus. Runtime modes become
T3-compatible OpenCode rules, known tool and request kinds get useful shared
representations, and unknown kinds stay visible rather than disappearing.

**Blocked by:** 11 — Normalize streaming, status and titles.

**Status:** ready-for-agent

- [ ] Command, file, web, MCP, image and collaboration tool states map to their
      shared work-log representations from start through completion or failure
- [ ] Unknown tools render as generic dynamic tools and retain diagnostic raw
      state
- [ ] Full access allows every OpenCode permission; the other runtime modes ask
      for sensitive operations while allowing the question capability
- [ ] Bash, read and edit permission requests map to specific request kinds and
      unknown permissions remain answerable
- [ ] Accept, accept-for-session, decline and cancel become once, always or
      reject replies as specified
- [ ] Pending permission identity is explicit and resolved requests disappear
      when OpenCode reports their reply
- [ ] Retuning and resumed-session permission application use the same mapping
- [ ] Socket tests cover tool ordering, every decision and unknown kinds
