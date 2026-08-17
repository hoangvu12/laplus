Status: ready-for-agent

# Provider-complete chat attachments

## Problem Statement

A developer can paste an image into Laplus and see it in the composer, but that
does not mean the selected agent receives it. Laplus normalizes the upload into
a stored chat attachment and carries it as far as the shared prompt, then the
Claude and Codex drivers silently send only the prompt text. OpenCode already
sends file parts, although an external OpenCode server can read those parts only
when it shares Laplus's filesystem or an equivalent path mapping.

The incomplete path has further user-visible consequences. The durable user
message does not retain attachment metadata, so the transcript can forget an
image after optimistic state is replaced or the application reloads. Queued and
retried turns need to retain the image paired with the prompt that introduced
it. First-turn title generation receives serialized upload metadata rather than
the image, preventing image-only requests from being titled from their actual
subject. Invalid or oversized uploads can also pass client validation without
an equivalent server-side limit.

The result is a dangerous false claim: the interface shows an attached image
while the model may never have seen it and the saved conversation may no longer
show that it was sent.

## Solution

Make current T3 attachment behavior the observable specification for Laplus.
Every supported runtime driver receives a stored chat attachment in its native
wire representation: Claude receives base64 image content blocks, Codex
receives structured data-URL image inputs, and OpenCode retains its existing
local file-URL parts. Preserve the attachment's durable metadata on the user
message and through queues, retries, snapshots, and reloads. Give first-turn
title generation the resolved image through the same provider-specific path.

Accept PNG, JPEG, GIF, and WebP images and enforce the existing 10 MiB decoded
image limit at the server boundary. Match T3's failure behavior: reject invalid
uploads and Claude/Codex attachment-resolution failures, while OpenCode keeps
omitting unresolved file references. Preserve OpenCode's shared-filesystem
assumption for external endpoints without adding a Laplus-only warning or a new
upload protocol.

## User Stories

1. As a Claude user, I want pasted images delivered to Claude, so that its answer is based on the visual context I supplied.
2. As a Codex user, I want pasted images delivered to Codex, so that it can inspect screenshots, diagrams, and other visual evidence.
3. As an owned OpenCode user, I want existing image delivery to keep working, so that provider parity does not regress a functioning path.
4. As an external OpenCode user with shared storage, I want Laplus to send the same local file references as T3, so that my configured path mapping continues to work.
5. As a developer, I want the image shown beside my sent message to be the image the provider received, so that the transcript does not make a false claim.
6. As a developer, I want image-only turns to work, so that I do not have to add meaningless text to ask about a screenshot.
7. As a developer, I want text and multiple images to retain their order and association, so that the provider can interpret the complete request.
8. As a developer, I want PNG images accepted, so that screenshots work without conversion.
9. As a developer, I want JPEG images accepted, so that photographs and common compressed images work.
10. As a developer, I want GIF images accepted, so that supported GIF inputs behave like T3.
11. As a developer, I want WebP images accepted, so that images produced by the existing composer compression path work.
12. As a developer, I want unsupported image formats rejected visibly, so that I know the provider did not receive them.
13. As a developer, I want malformed data URLs rejected visibly, so that corrupted uploads are not silently omitted.
14. As a developer, I want empty decoded images rejected visibly, so that an empty file is not presented as useful context.
15. As a developer, I want a decoded image larger than 10 MiB rejected by the server, so that remote or outdated clients cannot bypass the supported limit.
16. As a developer, I want an image exactly at the 10 MiB boundary accepted, so that the published limit is precise.
17. As a developer, I want a declared byte count that disagrees with the decoded image rejected, so that stored attachment metadata remains trustworthy.
18. As a developer, I want attachment persistence failures to reject the turn, so that text is not sent after its required image was lost.
19. As a Claude user, I want an unreadable stored attachment to refuse provider dispatch, so that Claude never receives an incomplete request without notice.
20. As a Codex user, I want an unreadable stored attachment to refuse provider dispatch, so that Codex never receives an incomplete request without notice.
21. As an OpenCode user, I want unresolved file references handled exactly as T3 handles them, so that Laplus does not invent provider-specific behavior.
22. As a developer, I want attachment metadata retained in the durable user message, so that reopening the thread still shows what I sent.
23. As a developer, I want saved attachment previews resolvable after a reload, so that the transcript remains useful evidence.
24. As a developer, I want a queued prompt to retain its own attachments, so that an image cannot move to or disappear into another turn.
25. As a developer, I want several queued prompts to preserve each prompt's text-image pairing, so that dispatch order does not corrupt context.
26. As a developer, I want retryable OpenCode turns to retain attachment metadata, so that retry sends the original request.
27. As a developer, I want merged queued prompts to retain every attachment in message order, so that batching does not discard visual context.
28. As a developer, I want first-turn title generation to inspect attached images, so that image-only UI work receives a meaningful title.
29. As a developer, I want title generation to avoid embedding raw data URLs in text prompts, so that large encoded payloads are not misrepresented or leaked as prose.
30. As a developer, I want title generation to use the configured provider's native image representation, so that it follows the same compatibility rules as ordinary turns.
31. As a maintainer, I want attachment behavior locked down at real socket and provider boundaries, so that a text-only encoder cannot silently return.
32. As a maintainer, I want Claude's exact outbound image block asserted, so that protocol drift is visible.
33. As a maintainer, I want Codex's exact structured input asserted, so that app-server protocol drift is visible.
34. As a maintainer, I want OpenCode's existing file-part behavior protected, so that parity work does not rewrite a working adapter.
35. As a maintainer, I want attachment validation tested independently of a real provider account, so that the suite remains deterministic and offline.
36. As a maintainer, I want current T3 behavior to resolve ambiguous policy questions, so that Laplus does not accumulate accidental divergences.

## Implementation Decisions

- Current T3 observable behavior and provider wire formats are the
  specification. Laplus retains its existing Rust architecture rather than
  copying upstream's TypeScript module boundaries.
- A chat attachment is normalized once into safe durable metadata and a stored
  file before the turn is committed or dispatched. A prompt carries resolved
  attachments beside its text until a driver encodes them.
- The upload boundary accepts only PNG, JPEG, GIF, and WebP. MIME values are
  canonicalized consistently with T3, decoded content must be non-empty, the
  declared size must agree with the decoded bytes, and decoded content must not
  exceed 10 MiB.
- Upload decoding, safe identifier/path creation, directory creation, and file
  persistence are required operations. Failure rejects the command before an
  incomplete user turn is committed.
- Durable user-message events and snapshots include attachment identity, name,
  MIME type, and byte size. Inline data URLs are upload transport only and are
  not retained in conversation metadata.
- Claude encodes text and images as one streaming user message. Each image is a
  base64 content block with its supported media type. Unsupported MIME types,
  invalid attachment identities, and unreadable files refuse the provider
  request as they do in T3.
- Codex models a turn start as a list of structured inputs rather than a single
  text field. Text becomes a text input and each resolved image becomes an image
  input whose URL is a data URL. Invalid identities and unreadable files refuse
  provider dispatch.
- OpenCode continues to encode each resolved chat attachment as a file part
  containing MIME type, filename, and local file URL. Unresolved references are
  omitted independently, matching T3 and current Laplus behavior.
- External OpenCode keeps the shared-filesystem/path-mapping assumption. This
  effort adds neither a remote byte-upload protocol nor a Laplus-only warning.
- Prompt queueing, merging, interruption recovery, and retry preserve attachment
  ordering and keep attachments paired with the user messages that introduced
  them.
- First-turn title generation receives resolved attachments as image inputs via
  the same provider-specific encoding behavior as other text-generation work.
  Raw upload JSON and data URLs are not substituted into title prompt text.
- Cursor and Grok remain inherited contract vocabulary rather than Laplus
  runtime drivers. This work does not add those drivers.
- Provider-specific encoding remains owned by each driver. The shared session
  layer carries provider-neutral stored attachments and does not select a
  universal provider wire representation.

## Testing Decisions

- Good tests assert behavior visible at the socket, durable conversation, or
  provider wire boundary. They do not assert private helper structure and do not
  invoke paid or installed provider services.
- The primary tests use the existing socket-level scripted-provider harnesses.
  The same real composer-shaped upload is sent through Claude, Codex, and
  OpenCode, and each harness asserts its exact outbound provider representation
  together with the resulting user-message event.
- Claude coverage extends the existing scripted CLI harness so a normal turn's
  streaming-input message can be inspected. The assertion distinguishes a
  structured image block from the current text-only line.
- Codex coverage extends the existing app-server request capture and asserts the
  complete ordered `turn/start` input list, including text and image data URLs.
- OpenCode coverage retains the existing attachment socket test and protects
  file-part ordering, metadata, and independent omission of unresolved
  references.
- Persistence coverage reads a fresh conversation snapshot after the optimistic
  client state is gone and after a server restart. It asserts durable attachment
  metadata and a resolvable attachment asset.
- Queue and retry coverage uses the existing session and OpenCode queue
  harnesses to prove that multiple prompts retain attachment order and
  ownership through merge, interruption, delivery failure, and retry.
- Title-generation coverage uses the existing first-turn title harness and
  asserts that an image-only request reaches the configured generator as an
  image input, without serialized upload JSON in the text.
- Focused normalizer tests cover every supported MIME type, an unsupported MIME
  type, malformed base64/data URLs, empty bytes, byte-count disagreement,
  unsafe identity/path cases, persistence failure, exactly 10 MiB, and one byte
  above the limit.
- Focused protocol golden tests may supplement the socket tests where the
  provider wire type itself changes, but they do not replace the higher socket
  seam.
- User-visible completion requires driving a running Laplus window: paste an
  image, send it through each configured runtime driver available in the test
  environment, reload the conversation, and verify the preview remains. Dev
  servers and watchers are stopped afterwards.

## Out of Scope

- Adding Cursor, Grok, ACP, or any other new runtime driver.
- Uploading attachment bytes to an external OpenCode server.
- Detecting whether an external OpenCode endpoint shares Laplus's filesystem.
- Adding a Laplus-only warning, capability gate, or stricter OpenCode
  unresolved-file policy.
- Supporting PDFs, SVG, AVIF, video, audio, or arbitrary files.
- Changing the 10 MiB product limit or the composer's image-compression UX.
- Copying T3's TypeScript architecture into the Rust server.
- Redesigning the general asset service beyond what durable chat attachments
  require.

## Further Notes

The upstream comparison and primary-source links are captured in
[upstream-research.md](./upstream-research.md). Research compared Laplus at
`b0cb6b05` with T3 at `cd096b9a` on 2026-08-17.

The existing OpenCode attachment path is working prior art, not the source of
the general failure. Its shared-filesystem limitation is an accepted domain
constraint. The decisive missing behavior is at the Claude and Codex encoder
boundaries, accompanied by missing durable message metadata and title-generation
propagation.
