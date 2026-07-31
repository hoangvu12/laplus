# `codex app-server` captures

Raw evidence, recorded 2026-07-31 against **codex-cli 0.146.0** on Linux
aarch64, driven by hand from a Python harness rather than by any laplus code.
`server/CLAUDE.md` is why they are here rather than in `server/fixtures/`: this
is capture evidence, and the fixture format is a decision the ticket that builds
the golden tests should make.

Each line is `{"dir": "send" | "recv" | "recv-raw", "msg": …}`. Both directions
are recorded, which is the difference from `fixtures/claude-cli/`: that protocol
is a stream the CLI writes, this one is JSON-RPC and half the traffic is ours.

The workspace was `/tmp/cap/ws` — a `README.md` and a `main.rs`, nothing else —
and the model was `gpt-5.4-mini` throughout, to keep the spend small. The wire
shape does not depend on which model answered.

| File                         | What it holds                                                                        |
| ---------------------------- | ------------------------------------------------------------------------------------ |
| `01-plain-turn.jsonl`        | A turn with no tools: `thread/start`, deltas, `turn/completed`                       |
| `02-command-execution.jsonl` | `ls` under `approvalPolicy: untrusted` — runs **without** an approval                |
| `03-write-approval.jsonl`    | A sandbox-escaping write: `item/commandExecution/requestApproval`, answered `accept` |
| `04-interrupt.jsonl`         | `turn/interrupt` mid-stream                                                          |
| `05-resume.jsonl`            | `thread/resume` of `01`'s thread from a **new process**, and it remembers            |
| `06-resume-missing.jsonl`    | `thread/resume` of a thread id that has no rollout                                   |

`spike-findings.md`, beside this directory, is what these recordings establish
and which of them contradict what upstream's source suggested.

## Redactions

Two values are replaced, because `.scratch/` is committed and this repository is
public: `installationId` is zeroed and `serverName` reads `redacted-host`. Both
identify the machine rather than the protocol, and nothing reads them. There are
no credentials in these files — every `…Token…` key in them is a token _count_
from `thread/tokenUsage/updated`.

## Re-recording

The harness is not committed — it is thirty lines of `subprocess` and `select`,
and rewriting it is cheaper than maintaining it. What matters is the handshake
these were recorded under, because it changes what the server sends:

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "initialize",
  "params": {
    "clientInfo": { "name": "laplus", "title": "laplus", "version": "0.0.1" },
    "capabilities": {}
  }
}
```

**Empty capabilities, deliberately.** With `{"experimentalApi": true}` — which is
what upstream sends — `turn/completed` is never emitted and a turn ends at
`thread/status/changed: {"type":"idle"}` instead. `01-plain-turn.jsonl` was
re-recorded for exactly this reason; an earlier take under the experimental flag
had no terminal notification in it at all.
