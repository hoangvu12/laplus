# Upstream research — why image turns do not reach every provider

Research date: 2026-08-17. Laplus was read at
[`b0cb6b05`](https://github.com/hoangvu12/laplus/tree/b0cb6b0577c85c80e7c34bc1905a81dfc9705969).
Its configured `origin` is `hoangvu12/laplus`; it has no `upstream` remote. The
first-party comparison is `pingdotgg/t3code` at
[`cd096b9a`](https://github.com/pingdotgg/t3code/tree/cd096b9ad5a4156ffeab85de617cbb219057007f),
read from a fresh clone of that repository's default branch. No provider was
run and no application code was changed for this note.

## Verdict

Laplus does not have one generic upload failure. The client upload crosses the
socket, the server decodes it, and a local attachment file is available to the
session. The loss is later and provider-specific:

| Laplus runtime driver | What Laplus sends                            | Result                                       |
| --------------------- | -------------------------------------------- | -------------------------------------------- |
| Claude                | `prompt.text` only                           | images are silently dropped                  |
| Codex                 | `turn/start` containing one text input only  | images are silently dropped                  |
| OpenCode              | text plus one `file` part per resolved image | implemented; local/owned servers should work |

The three runtime drivers are the three arms in Laplus's session registry.
Cursor and Grok occur in the inherited TypeScript model vocabulary, but are not
implemented Laplus runtime drivers; provider support should not be inferred
from those catalogue constants.

The concrete drop sites are small. Attachment normalization happens before the
session is started
([Laplus orchestration](https://github.com/hoangvu12/laplus/blob/b0cb6b0577c85c80e7c34bc1905a81dfc9705969/server/crates/laplus-server/src/orchestration.rs#L1611)),
and `Prompt` carries the resulting files
([Laplus prompt type](https://github.com/hoangvu12/laplus/blob/b0cb6b0577c85c80e7c34bc1905a81dfc9705969/server/crates/laplus-server/src/threads.rs#L317-L338)).
Claude nevertheless calls its agent with only `prompt.text`
([Laplus Claude send](https://github.com/hoangvu12/laplus/blob/b0cb6b0577c85c80e7c34bc1905a81dfc9705969/server/crates/laplus-server/src/turn.rs#L266-L271)),
and Codex constructs `Request::TurnStart` from only the same text
([Laplus Codex send](https://github.com/hoangvu12/laplus/blob/b0cb6b0577c85c80e7c34bc1905a81dfc9705969/server/crates/laplus-server/src/codex.rs#L393-L409)).
OpenCode is the exception: it turns each resolved path into a `file:` URL and a
`file` prompt part
([Laplus OpenCode send](https://github.com/hoangvu12/laplus/blob/b0cb6b0577c85c80e7c34bc1905a81dfc9705969/server/crates/laplus-server/src/opencode.rs#L1829-L1852)).

## What T3 upstream does differently

T3 first normalizes an inline data URL into an attachment file and canonical
metadata, enforcing a 10 MiB application limit
([contract](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/packages/contracts/src/orchestration.ts#L146-L187),
[normalizer](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/apps/server/src/orchestration/Normalizer.ts#L103-L177)).
This is the same architectural split Laplus is attempting; the difference is
that upstream finishes the conversion at every adapter.

### Claude

Upstream reads each persisted image and emits an SDK user-message content block
of the form `{type:"image", source:{type:"base64", media_type, data}}`, beside
the text block
([T3 Claude encoder](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/apps/server/src/provider/Layers/ClaudeAdapter.ts#L1218-L1310)).
That is the format in Anthropic's official Agent SDK streaming-input example:
the user message's `content` is an array containing text and a base64 image
block ([Anthropic Agent SDK](https://code.claude.com/docs/en/agent-sdk/streaming-vs-single-mode)).
Anthropic documents JPEG, PNG, GIF and WebP for vision, as well as image and
request-size constraints ([Anthropic vision guide](https://docs.anthropic.com/en/docs/build-with-claude/vision),
[API errors and limits](https://code.claude.com/docs/en/errors)).

Laplus already launches Claude with streaming JSON, so the missing behavior is
not a new upload service: its driver must send the SDK user-message object that
upstream sends instead of passing the bare string into `Agent::send`.

### Codex

Upstream resolves each file, re-encodes it as a data URL, and supplies
`{type:"image", url:"data:..."}` to its runtime
([T3 Codex adapter](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/apps/server/src/provider/Layers/CodexAdapter.ts#L1771-L1834)).
The runtime appends those structured image inputs to `turn/start.input` rather
than flattening them into prompt text
([T3 Codex runtime](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/apps/server/src/provider/Layers/CodexSessionRuntime.ts#L368-L419)).
OpenAI's official Codex manual also exposes image inputs directly (`-i` /
`--image`, repeated or comma-separated), confirming this is a supported Codex
input modality ([Codex image inputs](https://learn.chatgpt.com/docs/image-inputs.md)).

Laplus's own Codex protocol type is therefore too narrow at the exact boundary:
`Request::TurnStart` carries `text`, while the app-server wire expects a list of
structured user inputs. The fix needs a protocol-model change as well as a
different `send` implementation.

### OpenCode

T3 and Laplus currently agree. T3 creates `{type:"file", mime, filename,
url:file://...}` parts and calls the asynchronous prompt endpoint
([T3 conversion](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/apps/server/src/provider/opencodeRuntime.ts#L321-L343),
[T3 send](https://github.com/pingdotgg/t3code/blob/cd096b9ad5a4156ffeab85de617cbb219057007f/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1449-L1480)).
OpenCode's official server docs specify `parts` for both synchronous message
and asynchronous prompt operations
([OpenCode server API](https://opencode.ai/docs/server/)); its official model
documentation describes image as an input capability
([OpenCode models](https://opencode.ai/v2/docs/models)).

One qualification is important: a configured external OpenCode endpoint gets
the same local `file:` URL. It works only when that process sees the same path
mapping. Neither T3 nor Laplus uploads the bytes to a remote OpenCode server.
Thus “OpenCode images fail” can still be true for an external endpoint even
though the adapter is implemented.

## Secondary gaps exposed by the comparison

- Laplus's attachment resolver accepts only exact PNG/JPEG/GIF/WebP MIME names,
  verifies the declared byte count, writes the decoded bytes, and checks that
  the resulting file exists. Unlike current T3, it does not enforce the shared
  10 MiB bound at this server boundary. See
  [Laplus resolver](https://github.com/hoangvu12/laplus/blob/b0cb6b0577c85c80e7c34bc1905a81dfc9705969/server/crates/laplus-server/src/attachments.rs).
- The Claude and Codex behavior is silent: valid attachments are retained in
  `Prompt`, then unused. A provider capability check or explicit error would
  prevent a user from believing an unseen image reached the model.
- OpenCode should be tested separately for owned/local and external endpoints;
  only the former has a reliable filesystem guarantee.

## Scope recommendation

Treat this as two missing encoders plus one deployment caveat, not as a UI
upload rewrite. First add driver-level golden tests asserting the exact Claude
SDK message and Codex `turn/start.input`; then implement those two conversions.
Keep the existing OpenCode file-part path, add an owned-server integration test,
and either document the shared-filesystem requirement for external endpoints
or specify a real upload mechanism before claiming remote attachment support.

## Confirmed design decisions

- Match T3's support boundary: Claude, Codex, and owned/local OpenCode receive
  chat attachments; external OpenCode requires a shared filesystem or equivalent
  path mapping.
- Match T3's failure semantics: reject invalid uploads and Claude/Codex
  attachment-resolution failures; retain OpenCode's existing omission of an
  unresolved file reference.
- Accept PNG, JPEG, GIF, and WebP images, and enforce the 10 MiB decoded-image
  limit at the server boundary rather than trusting client validation alone.
- Match T3's durable conversation behavior: attachment metadata belongs on the
  user message and must survive snapshots/reloads, queued delivery, and retry;
  provider delivery alone is not sufficient.
- Pass first-turn attachments to automatic title generation as image inputs,
  matching T3. Reuse the provider-specific attachment encoding path; do not
  stringify raw upload JSON or its data URL into the title prompt.
- Prefer current T3 behavior wherever attachment policy is otherwise
  ambiguous. In particular, do not add a Laplus-only external-OpenCode warning
  or stricter unresolved-file behavior as part of this work.
- Treat T3's observable behavior and provider wire formats as the specification,
  while preserving Laplus's existing Rust architecture rather than copying the
  upstream TypeScript module structure.
