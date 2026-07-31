# `codex app-server` captures

Committed JSON-RPC exchanges used by the provider socket and protocol-golden
tests. The stand-in app-server replays the received half and records every
message laplus sends; the test compares that recording with the fixture's send
half. CI therefore needs neither a Codex install nor an OpenAI account, and a
change to methods, ids, cursors, capabilities, workspace roots or server-request
replies fails against the capture.

`01-provider-probe.jsonl` is hand-written from the v0.146.0 schemas and the raw
captures in `.scratch/codex-driver/captures/`. It combines cases that a healthy
provider emits independently: startup notifications, overlapping client/server
request ids, out-of-order responses, model pagination, account data and skills.
