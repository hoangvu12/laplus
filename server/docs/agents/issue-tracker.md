# Issue tracker: Local Markdown

Issues and specs (you may know a spec as a PRD) for this repo live as markdown files in `.scratch/`, at the **repository root** — not under `server/`. The tickets cover the UI as much as the server.

`.scratch/` is deliberately **not** gitignored: tracker files are committed and version alongside the code. That is the point of them, and it is why they are markdown rather than GitHub issues — a ticket and the commit that closes it move together.

This repository has one remote, `origin`, so `gh` is available. It is still not how tickets are tracked here. Do not open GitHub issues for this work.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The spec is `.scratch/<feature-slug>/spec.md`
- Implementation issues are one file per ticket at `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01` — never a single combined tickets file
- Triage state is recorded as a `Status:` line near the top of each issue file (see `triage-labels.md` for the role strings)
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

Note: `.scratch/` also holds loose protocol captures from the STEP 1 spike
(`bidi.ndjson`, `stream-sample.ndjson`). Those are raw evidence, not tickets —
leave them where they are. Tracker content always lives one level down, inside a
`<feature-slug>/` directory.

The one exception to "`.scratch/` is not gitignored" is
`.scratch/wire-capture/raw/`. Those are unredacted proxy recordings that contain
live session tokens; the redacted, committable versions live in
`server/fixtures/socket-wire/` (see `server/docs/socket-wire-format.md`).
Anything else added under `.scratch/` is still committed by default.

That rule lives in the **repository root** `.gitignore` since ADR-0014 — it used
to be in the server's, which no longer sits above `.scratch/`. It matters more
than it did: this repository is public.

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.

## Wayfinding operations

Used by `/wayfinder`. The **map** is a file with one **child** file per ticket.

- **Map**: `.scratch/<effort>/map.md` — the Notes / Decisions-so-far / Fog body.
- **Child ticket**: `.scratch/<effort>/issues/NN-<slug>.md`, numbered from `01`, with the question in the body. A `Type:` line records the ticket type (`research`/`prototype`/`grilling`/`task`); a `Status:` line records `claimed`/`resolved`.
- **Blocking**: a `Blocked by: NN, NN` line near the top. A ticket is unblocked when every file it lists is `resolved`.
- **Frontier**: scan `.scratch/<effort>/issues/` for files that are open, unblocked, and unclaimed; first by number wins.
- **Claim**: set `Status: claimed` and save before any work.
- **Resolve**: append the answer under an `## Answer` heading, set `Status: resolved`, then append a context pointer (gist + link) to the map's Decisions-so-far in `map.md`.
