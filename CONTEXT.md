# Laplus

Laplus is a desktop interface for working with coding agents across projects and conversations.

## Language

**Saved draft**:
An unsent new-thread draft containing user-authored text or an attachment. Merely opening a new thread or changing its settings does not create a saved draft.
_Avoid_: Empty draft, draft thread

**Subagent**:
A child agent delegated work by an agent in a thread. It remains identifiable within its parent thread and has its own inspectable work stream.
_Avoid_: Tool call, background session, when referring to the delegated child itself

**Subagent work stream**:
The ordered, replayable conversation and work of one subagent, rendered with the same message and work vocabulary as the main agent. It includes the assignment, prose, tool activity, errors, and terminal result.
_Avoid_: Progress log, result view
