# ADR-0042 — OpenCode chat attachments assume a shared filesystem

Date: 2026-08-01
Status: Accepted

Laplus will back chat attachments with its local attachment store and send each
resolved file to OpenCode as a `file://` URL, matching T3 Code; unresolved files
are omitted. The same representation is used for owned and external OpenCode
servers. Consequently an external server can read an attachment only when it
shares the Laplus host's filesystem or an equivalent path mapping. Uploading
bytes to remote servers would require a different protocol and ownership model
that neither T3 nor OpenCode's prompt-part integration supplies.
