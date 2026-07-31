# 07 — The access mode picker means something on Codex

**What to build:** The access mode a developer picks — supervised, auto-accept
edits, auto, full access — becomes something concrete on Codex rather than a
decorative control. On supervised the agent is held to a read-only sandbox and
asked about escapes, so supervision is enforced rather than promised. On full
access there are no permission questions, which is the whole reason to choose it.

The four runtime modes translate to Codex's approval policy and sandbox as
upstream translates them, with one declared divergence:

| runtime mode      | approval policy | sandbox            | approvals reviewer |
| ----------------- | --------------- | ------------------ | ------------------ |
| approval-required | untrusted       | read-only          | user               |
| auto-accept-edits | on-request      | workspace-write    | user               |
| auto              | on-request      | workspace-write    | **user**           |
| full-access       | never           | danger-full-access | user               |

**The divergence is `auto`.** Upstream routes it to an OpenAI subagent that
decides approvals on the developer's behalf. laplus keeps the developer as the
reviewer, so `auto` and `auto-accept-edits` behave identically on Codex for now.
The reason is what the developer would otherwise see: the subagent's work is
reported through two notification kinds a v1 does not handle, so the agent would
pause, something invisible would decide, and it would carry on with nothing in
the work log. Routing to the subagent becomes available once those are rendered —
it is out of scope here, not rejected.

**The reviewer is always sent explicitly rather than omitted**, because omitting
it on resume leaves whatever the thread last used.

Note from ticket 05's capture: what triggers an approval request is the sandbox
escape, not the policy name. A read-only sandbox under `untrusted` still runs a
command that stays inside it, with no question asked. That is correct behaviour
and the tests should assert it rather than treat it as a gap.

**Blocked by:** 06.

**Status:** ready-for-human

- [x] Each of the four runtime modes sends the approval policy and sandbox the
      table above names.
- [x] The approvals reviewer is sent explicitly on every thread start and every
      resume, never omitted.
- [x] On supervised, the agent runs under a read-only sandbox and an attempt to
      escape it opens an approval panel.
- [x] On supervised, an action that stays inside the sandbox runs without a
      question.
- [x] On full access, no permission question is asked.
- [x] `auto` behaves as `auto-accept-edits`, and the divergence from upstream is
      recorded with its reason where the mapping lives.
