# ADR-0043 — OpenCode text generation shares an idle-reaped server

Date: 2026-08-01
Status: Accepted

OpenCode participates in T3-compatible background text generation for commit
messages, pull requests, branch names and thread titles. Local requests share a
server outside conversation lifetimes and reap it after thirty idle seconds;
each operation still gets a temporary session with all tool permissions denied,
validated structured output and operation-specific sanitization. External
instances use their configured server. Starting a new process for every small
request would add avoidable startup cost, while attaching this work to an
arbitrary conversation would mix unrelated history and lifetime ownership.
