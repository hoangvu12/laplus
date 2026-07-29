# 06 — Every laplus calls itself `local`, so a client can hold only one

**What to build:** an environment id that is generated once per data directory
and persisted, so a client can hold this server and another one at the same
time.

**Status:** done — every acceptance criterion met, the last by a drive that held
three environments at once. "What landed" is at the bottom, and it carries one
finding the drive turned up that this ticket had assumed away.

**Depends on:** 02, which landed. Nothing else. This is the blocker 02's drive
found, and it is the one standing between the desktop application and the thing
this effort was started for — the user's own laplus and a remote one, side by
side, several at once.

## Why

`crate::config` says it plainly, at the field itself:

```rust
/// v1 has exactly one environment and no remote ones — cloud and remote
/// environments are out of scope — so this is a constant rather than a
/// generated id that would have to be persisted to stay stable across
/// restarts. The contract types it as a non-empty string, not a UUID.
pub environment_id: String,
```

That premise was correct when it was written and ticket 02 retired it. Remote
environments are no longer out of scope: a page on another origin can now read
this server's answers, so the desktop application walks the whole pairing chain
against a second laplus. **It then has nowhere to put it.**

Observed, not deduced — driving `ConnectionsSettings.tsx` against a second
server on 2026-07-30 (`tools/ui-driver/remote-pairing.mjs` is the harness):
`GET /.well-known/t3/environment` answered 200, `POST /oauth/token` answered 200
with an `access_token`, the dialog closed on its success path — and "Remote
environments" still read **"No saved remote environments"**.

## What is actually wrong

Four facts, and the collision falls out of them:

1. **`config.rs:404` is the constant.** `environment_id: "local".to_string()`,
   for every laplus ever started. The whole crate mentions the field three times
   — the struct, that line, and one test's JSON pointer — so nothing here reads
   it and nothing here depends on its value.
2. **The client's registry is one slot per id.**
   `ReadonlyMap<EnvironmentId, ConnectionCatalogEntry>`
   (`packages/client-runtime/src/connection/registry.ts:65`).
3. **The remote registers under the descriptor's id.**
   `connectionId = ` `bearer:${descriptor.environmentId}` and
   `environmentId: descriptor.environmentId`
   (`packages/client-runtime/src/connection/onboarding.ts:100-110`).
4. **So does the primary.** `loadPrimaryConnectionRegistration` builds
   `PrimaryConnectionTarget` from `descriptor.environmentId`
   (`apps/web/src/connection/platform.ts:251`).

The desktop's own backend is therefore already sitting in the `local` slot when
the remote's registration arrives, and `savedEnvironments` —
`ConnectionsSettings.tsx:1474` — lists only entries whose target is _not_
`PrimaryConnectionTarget`. The registration succeeded and had no slot to occupy,
which is why the failure is a silent empty list rather than an error the dialog
could show.

**Upstream already assumes ids differ per backend.** `platform.ts` carries
`loadSecondaryConnectionRegistration` for "a desktop-local secondary backend
(e.g. a parallel WSL backend)" on its own loopback origin — two backends on one
machine, which only works if they do not answer with the same id.

## What is not wrong

- **The contract wants no particular shape.** `EnvironmentId` is
  `TrimmedNonEmptyString.pipe(Schema.brand("EnvironmentId"))`
  (`packages/contracts/src/baseSchemas.ts:26-35`). Not a UUID, not a format. Any
  stable non-empty string is a legal id, so this needs no new type on either
  side.
- **Nothing pins `"local"`.** No test in `crates/laplus-server/tests/`, no
  capture in `fixtures/`, and nothing in `crates/laplus-shell/` asserts or
  depends on the value. The `"local"` literals in the UI are a different word —
  `ThreadEnvMode` (`local` against `worktree`) and git's `refKind`.
- **The label is not the id and does not change.** `machine_label()` stays what
  the UI shows.

## Decisions taken, so this is not re-argued while implementing

**The id is generated and persisted for every laplus, with no grandfathering.**
A data directory that already holds `state.sqlite` gets a generated id like any
other. The alternative — existing directories keep `local`, only fresh ones
generate — loses nothing and leaves the trap armed: any box that was tried
before this ticket, _including the Oracle instance this effort was driven
against_, keeps `local` and still collides, and the failure is the same silent
empty list. One invariant that is true everywhere beats two rules and an
exception.

**Its one-time cost is real and is accepted.** The local environment's id
changes once, and three client-side things are keyed to the old one: composer
drafts (`composerDraftStore`, `localStorage`, scoped per environment — see
`hasDraftThreadsInEnvironment`), any bookmarked thread URL (the id is a route
segment: `apps/web/src/routes/_chat.$environmentId.$threadId.tsx`), and the
IndexedDB profile records keyed `bearer:…` (`apps/web/src/connection/storage.ts`).
Drafts and last-open-thread are orphaned once, silently. Nothing durable is
lost — threads, projects and sessions live in `state.sqlite` and are not keyed
by environment — and re-keying the client's stores is deliberately **not** in
this ticket: it is UI work this effort has not touched, for a one-off cost of a
draft nobody sent.

**The id reads as `<machine>-<suffix>`** — `desktop-19eumeb-8f2a`, not
`8f2a41c9d3e7`. It appears in URLs, in logs and in a settings list, and the
point of this ticket is that there will be _several_; an opaque id makes every
one of them unidentifiable at exactly the moment that starts to matter. The
suffix is what keeps two data directories on one machine apart, which the
hostname alone cannot do — and which is the case upstream's WSL note and this
effort's own two-server drive are both about.

**The suffix is not a credential and must not be built like one.** Do not
truncate [`crate::pairing::pairing_code`]: a shortened credential generator is
indistinguishable, at a glance, from a weakened credential. Write a small
neighbour in `crate::pairing` that says what it is for. It may share the
alphabet — that alphabet already omits `0O1I`, which is what makes an id safe to
read off a screen and retype.

## What to build

1. **A durable id in `state.sqlite`, get-or-create.**
   [`crate::store::Database::secret_or_create`] is the pattern to follow and its
   doc comment carries the reasoning: one statement with
   `ON CONFLICT DO NOTHING` and then a read, because two windows opening
   together would otherwise both find nothing, both insert, and one would lose.
   Append a table to `MIGRATIONS` — one entry per `user_version`, and
   `opening_an_existing_database_applies_no_migration_twice` is the test that
   keeps that honest.

   **Not `server_secrets`, and not a file beside it.** Not the secrets table
   because this value is published in the descriptor, and that column's own
   comment says it "is never read by anything but the code that wrote it". Not a
   JSON file, despite `remote-access.json` being the precedent for reading one
   at `detect`, because the database is the thing whose lifetime the id should
   share: if `state.sqlite` is gone, every session and every pairing is gone
   with it and every client must re-pair anyway — so a new id costs nothing it
   was not already costing. A separate file could outlive the database or be
   restored without it, and either way the id would then name a server whose
   sessions it no longer matches.

2. **Settle it in `Server::bind_with`, exactly where the UI version is
   settled.** That function already does this once:

   ```rust
   let config = match ui.version() {
       Some(version) => config.serving_ui_version(version),
       None => config,
   };
   ```

   The same shape, for the same reason its comment gives — it is the one place
   the config and the thing that knows better are together. `Server::bind` calls
   `bind_with`, and both binaries call `bind` (`laplus-server/src/main.rs:69`,
   `laplus-shell/src/main.rs:111`), so neither needs touching.

3. **`ServerConfig::detect` generates one rather than leaving it empty.** A
   config that was never bound still has to serialize —
   `no_required_string_is_empty` walks `/environment/environmentId` and an empty
   string decodes as a schema failure the UI reports as a broken server. So
   `detect` mints a fresh unpersisted id and `bind_with` replaces it with the
   durable one. **This is the shape `server_version` already has** — `detect`
   answers the crate's version and `bind_with` overrides it with the bundle's —
   so it is a property this file already relies on rather than a new trap.

4. **A slug that is safe in a URL.** Lowercase; `[a-z0-9]` kept; every run of
   anything else collapsed to a single `-`; no leading or trailing `-`; capped
   so a long hostname does not make an unusable route segment; and a hostname
   that slugs to nothing falls back to what `machine_label` already falls back
   to. `DESKTOP-19EUMEB` → `desktop-19eumeb`.

## Acceptance criteria

- Two servers with different data directories report different `environmentId`s.
- One data directory reports the **same** id across a restart — open, read,
  drop, reopen.
- Two `Database` handles on one file agree on it, which is what the
  get-or-create statement is for.
- The id matches `^[a-z0-9][a-z0-9-]*$` and begins with the machine's slug.
- `ServerConfig::detect()` alone still reports a non-empty id, and
  `no_required_string_is_empty` still passes.
- The descriptor over HTTP still agrees with the one `server.getConfig` reports
  — `http_boot.rs` already asserts this and must keep passing, because the id is
  now read from a second place.
- **A drive, not a test:** a desktop laplus adds a remote environment, it appears
  under "Remote environments", and both are usable at once.
  `tools/ui-driver/remote-pairing.mjs` gets to pairing; the Add Environment form
  is what has to be driven past it. **Then add a third**, because "several at
  once" is the actual requirement and the id is only the first thing that could
  have prevented it — whether the sidebar, the supervisors and the route hold up
  beyond two is unknown, and this is the drive that would find out.

## Out of scope

- **Re-keying the client's drafts, URLs and IndexedDB records.** Decided above:
  the loss is one-off and silent, the work is UI-side, and paying for it here
  would be the largest part of this ticket by far.
- **An operator-settable id** (`--environment-id oracle-box`). Attractive, and a
  separate change: it adds a flag through `crate::launch` and one more way for
  two machines to be given the same value by hand. Worth revisiting once several
  environments are actually in use and the generated names have been read a few
  times.
- **Two servers sharing one data directory.** They would report the same id and
  collide exactly as before — and they are already sharing one `state.sqlite`,
  which nothing here supports. `LOCALAPPDATA` per instance is the existing answer
  (`tools/ui-driver/README.md`).
- **The label.** Two machines with the same hostname show the same label and
  that is fine; they are now told apart by the id, which is what the id is for.
  **The drive found this reasoning to be wrong** — see "What landed".

## What landed

Four small pieces, no new dependency, and 11 tests. The shape is the one the
ticket specified; what it did not anticipate is in the last section, and it came
from the drive rather than from the code.

`pairing::identifier_suffix` — four characters, lowercased, sharing
`PAIRING_CODE_ALPHABET` and **not** truncating `pairing_code`, as decided. It
uses a bare `%` where its neighbour keeps `PAIRING_CODE_REJECTION_LIMIT`, and
says why in a comment: 32 divides 256 so it is uniform today, the existing
`nothing_is_rejected_while_the_alphabet_divides_the_byte_range` fails loudly the
moment that stops being true, and a biased suffix costs a collision chance rather
than a weakened credential.

`config::machine_slug` and `config::slug_of` — the prefix, split the way
`machine_label`/`hostname_in` already split, so the parsing is testable without
setting a process-global environment variable. Capped at 28 characters, trimmed
**after** the cap so a name cut mid-run does not keep the dash it was cut
through. `DESKTOP-19EUMEB` → `desktop-19eumeb`.

`store`: migration **v7**, a one-row `environment` table shaped like
`orchestration`, and `Database::environment_id_or_create` — `ON CONFLICT DO
NOTHING` then a read, following `secret_or_create` exactly. Not `server_secrets`,
whose column comment says it is never read by anything but its writer, and this
value is published unauthenticated in the descriptor.

`config::fresh_environment_id` mints `<machine>-<suffix>` for both callers, so an
id read from the database and an id minted by an unbound config are not
distinguishable — a reader of a log line cannot tell which they have.
`ServerConfig::with_environment_id` is settled in `Server::bind_with` beside the
UI version, and a database failure there is logged and survived like
`mint_boot_grant` above it: the process keeps the id `detect` minted, which is
legal and unique to this run but does not outlive it.

**The field's own doc comment was rewritten**, since the ticket quoted it as the
Why and leaving it would have left the file arguing for the constant it no longer
holds. `docs/running-headless.md` had the same problem in prose — it told the
reader the desktop case does not finish and to use an `ssh -L` tunnel — and now
documents the id, the data-directory-per-server requirement, and the one-time
re-key cost.

### The drive

`add-remote-environment.mjs` exited 1 before this landed and exits 0 after, with
nothing else changed, which is what the ticket asked for. It also does more than
it used to: 9 cross-origin calls per remote rather than 4, because a registration
that is actually adopted goes on to trade the bearer for a socket ticket and
fetch the orchestration shell. Four calls and an empty list was the symptom; nine
and a listed row is the environment being _used_.

The driver now takes **one or more** `<remote-url> <code>` pairs and insists the
list grows by one for each, because three separate runs would prove three servers
each pair once rather than that one client holds three at a time. Driven against
three servers on 5773/5774/5775 with a data directory each: two remotes added in
one session, both connected, 0 refused calls, and the ids
`desktop-19eumeb-xj6d`, `-xy4x`, `-v6wy` — which survived a restart of all three
unchanged.

### What the drive found, which this ticket got wrong

**"They are now told apart by the id" is false as written**, and Out of scope
above says it. `SavedBackendListRow` renders `environment.label` and nothing else
identifying — `ConnectionsSettings.tsx:1330` — so the id this ticket generates is
never shown to the user anywhere in that list. Three data directories on one
machine share a hostname by design, so the drive's own output is two rows both
reading `DESKTOP-19EUMEB` with nothing to choose between them:

```
Remote environments
Add environment
DESKTOP-19EUMEB
Disconnect
DESKTOP-19EUMEB
Disconnect
```

This is not a regression and it did not block anything — the environments are
distinct, connected and usable, which is what the ticket was for, and the id does
its work in the registry where the collision was. But the argument for a
_legible_ id over an opaque one was that it "lands in URLs and a settings list",
and half of that was not true.

**Fixed afterwards, in a commit of its own**, once the user had seen the finding
and asked what upstream does about it. It does nothing: `pingdotgg/t3code`'s row
builds the same metadata line from an SSH target and a `relayManaged` flag, so
its remotes read `SSH user@host` or `T3 Connect` and only a bare bearer remote is
left blank — a gap laplus cannot inherit its way out of, because removing the
relay surface (`94da6be`) left the unlabelled shape as the only remote shape it
has.

The row now carries the **host it was paired with** under the label —
`formatRemoteBackendHost` in `ConnectionsSettings.logic.ts`, following the
`*.logic.ts` split this directory already uses. The host rather than the id
because the port is what differs between two servers on one machine, and because
it is the value the user typed rather than one the server invented. Driven the
same way, and the section now reads:

```
DESKTOP-19EUMEB
127.0.0.1:5774
Disconnect
DESKTOP-19EUMEB
127.0.0.1:5775
Disconnect
```
