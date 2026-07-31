# `codex app-server` captures

Committed JSON-RPC exchanges used by the provider socket and protocol-golden
tests. The stand-in app-server replays the received half and records every
message laplus sends; the test compares that recording with the fixture's send
half. The adjacent `.expected.json` is the pure fold of the received half. CI
therefore needs neither a Codex install nor an OpenAI account, and a change to
methods, ids, cursors, capabilities, workspace roots, server-request replies or
the decoded provider snapshot fails against the capture.

Every capture does both jobs: its received half is a fresh-state protocol
golden, and its exchange is input to a scripted app-server socket test. Re-record
after a Codex release even when every golden still matches; the same recording
also checks that the real session loop still speaks the exchange end to end.
`01-provider-probe.probe.expected.json` is the probe decoder's additional
expected snapshot, beside that capture's ordinary fresh conversation fold.

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

`02-command-execution.jsonl` reduces
`.scratch/codex-driver/captures/02-command-execution.jsonl`. It preserves the
commentary before the command, the command's running and completed items, its
process result, and the final answer. Its captured `thread/start` deliberately
still says `approvalPolicy: untrusted` with a read-only sandbox: `ls` ran with no
approval request under that handshake, establishing that a sandbox escape, not
the policy's name alone, triggers the request. The socket stand-in replays the
received half; the socket test runs it as `approval-required` and separately
pins every runtime mode's outbound handshake.

`03-write-approval.jsonl` reduces
`.scratch/codex-driver/captures/03-write-approval.jsonl`. It preserves the
sandbox-escaping command's `item/started` before its approval request, the
request's accept/execpolicy-amendment/cancel decision list, and the response
that releases the turn. The socket stand-in stops when it sends the request and
does not replay anything after it until laplus answers; the structured amendment
is recognized but never offered or sent by laplus.

`04-interrupt.jsonl` reduces
`.scratch/codex-driver/captures/04-interrupt.jsonl`. It preserves the streamed
message beginning, the outbound `turn/interrupt`, more deltas arriving after
that request, and the acknowledgement as the final message. There is no
completed assistant item, completed turn or idle notification. The stand-in
stops at the outbound request, reads and records laplus's real interrupt, then
replays the late deltas and pauses immediately before answering with the real
request id. Its second turn replays `01-plain-turn` through the same app-server
and Codex thread to prove they can take an immediate correction.

`05-resume.jsonl` reduces
`.scratch/codex-driver/captures/05-resume.jsonl`. A new app-server resumes the
thread created by `01-plain-turn` from its id alone, then answers by quoting the
prompt from that earlier process. The resume request adds laplus's explicit
access envelope, whose omission in the hand-driven capture was the finding that
became the access-mode ticket.

`06-resume-missing.jsonl` reduces
`.scratch/codex-driver/captures/06-resume-missing.jsonl`. It preserves the
current Codex error wording verbatim, with only its id normalized: `no rollout
found for thread id ...`. The socket stand-in answers a resume with that
captured error and pins the initialize/initialized/resume prefix to this fixture,
not to the successful resume capture. The fixture itself ends at the refusal
because the real capture did; the stand-in adds only the minimal fresh thread
and completed turn needed to exercise laplus's fallback after that point.

`07-synthetic-drift.jsonl` is **synthetic, not recorded**. A healthy Codex
cannot emit the future and malformed traffic it exists to test. Against the
v0.146.0 `ThreadItem` union it covers all twelve item kinds this driver does not
handle: `hookPrompt`, `plan`, `dynamicToolCall`, `collabAgentToolCall`,
`subAgentActivity`, `webSearch`, `imageView`, `sleep`, `imageGeneration`,
`enteredReviewMode`, `exitedReviewMode`, and `contextCompaction`. It also carries
two unhandled notification methods, a parsed `item/started` whose item has the
wrong shape, a `turn/start` result without its required turn id, an unknown
notification before the thread response, and one line that is not JSON.
Recognized output and `turn/completed` follow all of them, so both the fresh fold
and socket replay prove drift is counted without ending the session.
