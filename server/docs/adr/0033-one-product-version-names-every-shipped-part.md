# ADR-0033 — One product version names every shipped part

Date: 2026-08-01
Status: Accepted; supersedes ADR-0011 and the split-version parts of ADR-0026

laplus has one release identity across its Rust server and shell, web UI, Tauri
application, npm launcher and platform packages. The release workflow resolves
that effective version before any artifact is built, aligns every build input to
it, and then builds; prerelease suffixes are part of the identity and therefore
appear everywhere too. A repository check keeps the committed base versions in
sync. This replaces the previous model in which `environment.serverVersion`
could name the served UI rather than the server and prevents the new CLI from
preserving a version distinction the product no longer has.
