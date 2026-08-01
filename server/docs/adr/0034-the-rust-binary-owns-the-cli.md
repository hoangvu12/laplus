# ADR-0034 — The Rust binary owns the CLI

Date: 2026-08-01
Status: Accepted

`laplus-server` owns one typed, hierarchical command interface: parsing,
validation, contextual help, version, output and exit behavior. The npm
`laplus` executable is only a launcher that selects the platform binary,
supplies its dynamically discovered UI bundle through the child environment,
and forwards streams, signals and status. Bare `laplus` remains the default
spelling of `laplus serve`; supported administrative operations are exposed as
`auth pairing {create,list,revoke}` and `service {install,status,uninstall}`.
This boundary avoids duplicated help and prevents launcher metadata such as a
UI path from becoming an irrelevant option every administrative parser must
accept and ignore.
