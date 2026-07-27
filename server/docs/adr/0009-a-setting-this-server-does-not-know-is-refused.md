# ADR-0009 — A setting this server does not know is refused, and one it cannot decode is never stored

Date: 2026-07-27
Status: Accepted

## Context

Ticket 22 gives the developer a settings panel and a keybindings file. Both are
*written* by this server and *read* by upstream's client, and that asymmetry is
where the design pressure is: a value this server is happy with but the client
cannot decode does not degrade gracefully. Effect's schemas fail whole.

Three fields make that concrete.

**`ServerConfig.issues`** is `Schema.Array(ServerConfigIssue)`, and
`ServerConfigIssue` is a **closed union of two literals** —
`keybindings.malformed-config` and `keybindings.invalid-entry`. An issue with a
`kind` of our own invention does not render oddly; it fails the decode of the
entire `server.getConfig` payload. On a broken keybindings file. Which is the one
case the field exists for.

**`ResolvedKeybindingRule.command`** is a closed union of forty-one — twenty-one
literals, two families of nine, and `script.<id>.run` — and `keybindings` is an
array of those. One unrecognised command is not an inert shortcut: it costs the
developer all forty-one.

**`ServerSettings.textGenerationModelSelection.instanceId`** is a branded slug.
It is *stored*, and read back inside `ServerSettings` — so a bad value written
once poisons every `server.getSettings` **and** every `server.getConfig` until
somebody edits the file by hand.

Against that sits a real cost: refusing an unknown field means a **newer UI**
gets a refusal where a shrug would have let it carry on.

## Decision

**A value that could fail the client's decode is refused before it is stored,
and an unknown field in a patch is refused rather than ignored.**

Concretely:

- Every `kind` this server emits is one of the contract's two literals. A
  settings problem has no member and is therefore **logged, not sent** — the one
  place ticket 22's "with a warning" is a log line rather than a UI row.
- Every `command` is checked against the closed union, on the way in from the
  socket *and* on the way in from the file. A file entry that fails is dropped
  with an `invalid-entry` issue naming its index.
- `instanceId` is checked against the slug pattern, because it is the one field
  here whose badness outlives its own call.
- An unrecognised patch field is a refusal with a sentence.

There is one deliberate exception, and it is not softness: a patch that
**repeats** a value this server reports but cannot change —
`enableAssistantStreaming`, `providerInstances` — succeeds. Only a *change* is
refused. Without that, `settings.json` would be a file this server writes and
then refuses to read back, and every setting would be forgotten at the next
restart.

## Consequences

- **A newer UI's new setting is refused, not absorbed.** This is the cost, and it
  is accepted on two grounds: the criterion asks for invalid settings to be
  rejected *with a message*, and the alternative — accepting and dropping — is a
  panel that reports success and changes nothing, which is the worse failure to
  debug. The sentence names the field, so the developer can see what happened.
- **A file may be from another build; a patch may not.** Reading `settings.json`
  is per-field, so an unknown key costs itself and the twenty beside it still
  apply. Applying a patch is all-or-nothing, so a refusal changes nothing. The
  two rules look inconsistent and are not: one is a downgrade, the other is this
  client, now.
- **The refusal has to be legible.** `KeybindingsConfigError`'s own `message`
  getter composes "Unable to parse keybindings config at {configPath}", and
  `ServerSettingsError`'s composes from `operation` and `settingsPath` — so both
  are always filled with the real file, and `operation` distinguishes a request
  that never reached a disk (`normalize`) from a disk that would not take it
  (`write-file`). An error with a blank path is a sentence that says nothing.
- **Two mirrored lists have to stay in step**: the default keybindings and the
  set of commands that may be bound, both transcribed from
  `t3code/packages/{shared,contracts}/src/keybindings.ts`. A command in the first
  and not the second would silently drop a default the developer never touched,
  so a test checks them against each other.
- **Hand-editing `keybindings.json` still needs a restart.** The file is read at
  startup and when this server writes it; there is no watcher on it. Changes made
  *through the app* reach every open window immediately, which is what the
  criterion asks for — but the path is advertised as editable, so this is a gap
  rather than a decision, and `crate::watcher` already exists to close it.
