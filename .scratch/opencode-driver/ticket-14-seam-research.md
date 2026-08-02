# Ticket 14 seam research: questions and chat attachments

Research date: 2026-08-02. The implementation target is OpenCode `v1.18.10`
(`7902e04c3a67f7c69726bc955efb46e29214c797`), and the first-party comparison is
T3 Code `0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62`. This note uses only those
projects' source/generated API plus Laplus's current source.

## Conclusions for ticket 14

- Keep OpenCode questions in a dedicated pending-question table keyed by the
  upstream request id. Never infer whether a pending id is a permission or a
  question from its spelling.
- Derive each UI question id as `question-${index}-${slug(header)}` (or
  `question-${index}` for an empty slug), preserve array order, and use that
  same derivation when converting Laplus's answer map back into OpenCode's
  ordered `string[][]`.
- An answer is not resolved when the reply HTTP call returns. Send
  `POST /question/{requestID}/reply`, retain the pending entry, then close it on
  `question.replied`. Rejection follows the same lifecycle through the distinct
  no-body `/reject` operation and `question.rejected` event.
- Laplus currently has no user-input rejection verb or UI action. Ticket 14
  therefore cannot truthfully satisfy its rejection criterion by encoding an
  empty answer map: the contract needs an explicit reject/cancel operation (or
  an explicitly specified extension to the existing command) and the composer
  needs a trigger for it.
- Chat image bytes enter in the client form of `thread.turn.start`, are decoded
  and persisted before the canonical command is emitted, and thereafter travel
  only as attachment metadata. OpenCode gets one `file` prompt part per
  successfully resolved metadata record, using a `file:` URL. Resolution
  failures omit only that part.
- Owned and external OpenCode servers use the identical `file:` URL. This is
  valid for an external server only when it sees the same filesystem/path
  mapping; there is no remote upload in this integration.
- `question.v2.*` is a separate event family in OpenCode 1.18.10. Ticket 14 is
  scoped to original `question.asked/replied/rejected`, so v2 events must remain
  observable unknown events rather than being decoded as the original shape.

## Attachment lifecycle and start-turn representation

The public/client start-turn contract carries uploaded images inline as
`{type:"image", name, mimeType, sizeBytes, dataUrl}`. T3's normalizer parses the
base64 data URL, verifies an image MIME and the decoded byte bound, creates a
thread-derived UUID id, writes the bytes below `attachmentsDir`, and replaces
the upload object with canonical metadata `{type, id, name, mimeType,
sizeBytes}` before emitting the canonical `thread.turn.start`. The canonical
command and provider `sendTurn` contract therefore contain references, not
bytes. [T3 contract](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/packages/contracts/src/orchestration.ts#L145-L181),
[start-turn client/canonical split](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/packages/contracts/src/orchestration.ts#L685-L723),
[normalization and write](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/orchestration/Normalizer.ts#L103-L178).

The store derives a safe filename from the attachment id and inferred image
extension. `resolveAttachmentPath` confines that relative name beneath
`attachmentsDir`; resolution by id probes only the known safe extensions and
requires the file to exist. [T3 attachment store](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/attachmentStore.ts#L13-L97).

At OpenCode send time T3 resolves each canonical attachment independently.
Unresolved entries are skipped. Each resolved entry becomes
`{type:"file", mime, filename, url:pathToFileURL(path).href}`; text, when
present, precedes all file parts in `session.promptAsync.parts`. A turn is
rejected only if both trimmed text and the successfully resolved file-part list
are empty. [T3 file-part conversion](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts#L294-L326),
[T3 send-turn assembly](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1440-L1493).
OpenCode's generated prompt contract names the same file-part fields (`type`,
`mime`, optional `filename`, and `url`). [OpenCode generated file-part type](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L2561-L2568),
[prompt-async input](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L10137-L10184).

Laplus already mirrors the TypeScript contract split in
[`packages/contracts/src/orchestration.ts`](../../packages/contracts/src/orchestration.ts):
the client command has `dataUrl`, while the canonical start command has an id
and metadata. The missing Rust seam is explicit in
[`server/crates/laplus-server/src/orchestration.rs`](../../server/crates/laplus-server/src/orchestration.rs):
`TurnMessage.attachments` is still `Vec<Value>` and its comment says the values
are dropped on the way to the agent. The current asset service also explicitly
refuses attachment resources in
[`server/crates/laplus-server/src/assets.rs`](../../server/crates/laplus-server/src/assets.rs).
Ticket 14 therefore needs both byte persistence during command normalization
and a path resolver usable by the OpenCode driver; signed browser preview URLs
are a separate concern.

## Original question shape, ids, and ordered answers

The original OpenCode `QuestionRequest` is `{id, sessionID, questions, tool?}`.
Its event lifecycle is `question.asked` carrying that request, then either
`question.replied` carrying `{sessionID, requestID, answers}` or
`question.rejected` carrying `{sessionID, requestID}`. Answers are an array in
question order, and every element is itself an array of selected labels.
[OpenCode request type](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L2448-L2456),
[OpenCode original question events](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L6944-L6975),
[reply operation](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L9159-L9196).

T3 derives a stable UI id from the zero-based index plus a lowercase sanitized
header. It maps questions and options without sorting, so both upstream orders
are preserved. On response it looks up the original request in
`pendingQuestions` and converts the keyed Laplus answers back into the original
question order before calling `question.reply`. [T3 id derivation](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts#L294-L303),
[T3 inbound normalization](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L390-L400),
[T3 ordered answer conversion](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/opencodeRuntime.ts#L361-L378),
[T3 response](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1581-L1600).

Laplus's existing UI payload is compatible: `UserInputQuestion` has `id`,
`header`, `question`, ordered `options`, and `multiSelect`, while
`ProviderRespondToUserInputInput.answers` is a keyed record. See
[`packages/contracts/src/providerRuntime.ts`](../../packages/contracts/src/providerRuntime.ts)
and [`packages/contracts/src/provider.ts`](../../packages/contracts/src/provider.ts).
The adapter must retain the original request because the keyed record alone
cannot reconstruct OpenCode's required order.

## Exact response lifecycle and event names

T3 handles `question.asked` by storing `event.properties` in
`pendingQuestions[event.properties.id]`, then emits shared
`user-input.requested` with the same request id. It does **not** remove the
entry after the HTTP reply succeeds. On `question.replied`, it reads the saved
questions to reconstruct a keyed resolution payload, deletes the pending entry,
and emits `user-input.resolved`. On `question.rejected`, it deletes the same
entry and emits `user-input.resolved` with `{answers:{}}`.
[T3 event handling](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L981-L1032).

That upstream-event boundary matters in Laplus. Its current generic session
loop removes a user-input request before `driver.answer` and always appends
`user-input.resolved` immediately after the call; see
[`server/crates/laplus-server/src/session.rs`](../../server/crates/laplus-server/src/session.rs).
For an event-resolved OpenCode driver, removal/resolution must be deferred to the
matching upstream event (as permission handling already distinguishes with a
driver capability), or duplicate/out-of-order resolution will result.

## Rejection: operation versus current Laplus UI

OpenCode rejection is a distinct `POST /question/{requestID}/reject` with no
body, returning boolean; it is not `question.reply` with `[]`, `[[]]`, or an
empty map. [OpenCode reject contract](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L9198-L9230),
[generated client](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/sdk.gen.ts#L3052-L3081).
OpenCode's own TUI binds Escape to this rejection call, proving the intended UI
trigger. [OpenCode question UI](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/tui/src/routes/session/question.tsx#L48-L62),
[Escape binding](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/tui/src/routes/session/question.tsx#L244-L281).
Internally rejection removes the pending entry, publishes `question.rejected`,
and fails the waiting question with `QuestionRejectedError`; it is therefore a
real control outcome, not an answer value. [OpenCode question service](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/opencode/src/question/index.ts#L114-L147).

T3 Code's adapter at the pinned commit implements `question.reply` but exposes
no `question.reject` call from `respondToUserInput`; it only consumes a
`question.rejected` event if another OpenCode client rejects the request.
[T3 response surface](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1581-L1600),
[T3 rejected-event handling](https://github.com/pingdotgg/t3code/blob/0ad91b6e7fc1fcb6d5f4bc736d84c337e912bc62/apps/server/src/provider/Layers/OpenCodeAdapter.ts#L1020-L1032).
Laplus is the same at present: `thread.user-input.respond` and
`ProviderRespondToUserInputInput` require `answers`, and the composer callback
only submits answers. There is no reject discriminator or command. Ticket 14's
rejection checkbox therefore requires a deliberately named contract/UI seam;
the most direct shape is a separate `thread.user-input.reject` command that the
driver maps only for a pending question, plus an explicit Dismiss/Reject action
in the pending-question UI.

## Original versus question-v2

OpenCode 1.18.10 also declares `question.v2.asked` (and corresponding v2
resolution events) with distinct schemas. That is not an alias for the original
events above. [OpenCode v2 event type](https://github.com/anomalyco/opencode/blob/v1.18.10/packages/sdk/js/src/v2/gen/types.gen.ts#L6805-L6815).
Laplus's current known-event list names only `question.asked`,
`question.replied`, and `question.rejected` in
[`server/crates/laplus-server/src/opencode_protocol.rs`](../../server/crates/laplus-server/src/opencode_protocol.rs),
which already gives ticket 14 the desired behavior: v2 events pass through the
observable unknown-event path until a later ticket specifies them.
