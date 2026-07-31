# `codex app-server` captures

Committed JSON-RPC exchanges used by the provider socket and protocol-golden
tests. The stand-in app-server replays the received half and records every
message laplus sends; the test compares that recording with the fixture's send
half. The adjacent `.expected.json` is the pure fold of the received half. CI
therefore needs neither a Codex install nor an OpenAI account, and a change to
methods, ids, cursors, capabilities, workspace roots, server-request replies or
the decoded provider snapshot fails against the capture.

`01-provider-probe.jsonl` is a deterministic reduction of the recorded v0.146.0
provider exchange at
`.scratch/codex-driver/captures/07-provider-probe.jsonl`. It keeps that capture's
out-of-order account, model and skill answers and startup noise, adds the
independently recorded server-request id overlap from `03-write-approval.jsonl`,
and splits the model response into two pages to exercise the schema's cursor.
Account, model and skill data are minimized so upgrades produce a reviewable
golden diff rather than one dominated by the developer's installed skills.

`01-plain-turn.jsonl` is the same kind of reduction of
`.scratch/codex-driver/captures/01-plain-turn.jsonl`. It preserves the empty
capabilities handshake, thread and turn requests, reasoning lifecycle, streamed
assistant deltas, authoritative completed message, and the recorded order where
idle arrives immediately before `turn/completed`. The socket stand-in reads this
fixture, rewrites response ids to the requests it actually received, and replays
the received half; the adjacent expected file is its fresh conversation fold.
