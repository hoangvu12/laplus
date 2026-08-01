# ADR-0039 — Checkpoint revert moves the tree before provider history

Date: 2026-08-01
Status: Accepted

An OpenCode checkpoint revert follows T3 Code's ordering: restore the
filesystem, refresh the workspace index, roll back the provider conversation by
the removed turn count, prune later checkpoint refs, then publish completion.
These operations cannot be atomic across Git and an external agent server. If
provider rollback fails, the filesystem therefore remains restored while agent
history remains ahead; Laplus reports failure and keeps the later checkpoint
refs instead of publishing a false completion. Reversing the order would merely
move the same irreducible partial-failure risk to provider history.
