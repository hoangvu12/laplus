# `codex app-server` captures

Committed JSON-RPC exchanges used by the provider socket tests. The stand-in
app-server replays the received half while the real server sends the recorded
requests, so CI needs neither a Codex install nor an OpenAI account.

`01-provider-probe.jsonl` is hand-written from the v0.146.0 schemas and the raw
captures in `.scratch/codex-driver/captures/`. It combines cases that a healthy
provider emits independently: startup notifications, overlapping client/server
request ids, out-of-order responses, model pagination, account data and skills.
