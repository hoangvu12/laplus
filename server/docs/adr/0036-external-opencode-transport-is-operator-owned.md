# ADR-0036 — External OpenCode transport is operator-owned

Date: 2026-08-01
Status: Accepted

Laplus accepts a configured external OpenCode server URL with either HTTP or
HTTPS and, when supplied, sends the OpenCode password using Basic
authentication. It neither restricts plaintext HTTP to loopback nor owns the
endpoint's lifetime or transport security. This matches T3 Code and preserves
LAN, VPN and reverse-proxy deployments; requiring HTTPS inside laplus would
reject valid operator-controlled networks without making the external service
itself secure. The owned OpenCode server remains loopback-only by default.
