# Upstream feature roadmap

This roadmap turns the findings in [research.md](./research.md) into durable
follow-up work. It is an index, not an implementation spec: each feature gets
its own `.scratch/<feature>/` directory once its design is ready to leave this
roadmap.

Statuses use the repository's canonical tracker roles. `needs-info` means a
named product decision or measurement is still required; `needs-triage` means
the feature has not yet been selected for design.

| Priority | Feature                                                             | Status            | Next flow                                           | Dependency or open question                                                                              |
| -------- | ------------------------------------------------------------------- | ----------------- | --------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| 1        | Saved-draft shelf                                                   | `ready-for-agent` | `/implement`                                        | None; behavior is settled below.                                                                         |
| 2        | [Pinned threads and manual pinned order](../pinned-threads/spec.md) | `ready-for-agent` | `/implement` tickets 01–04                          | Behavior follows pinned T3 snapshot; implementation tickets are under `.scratch/pinned-threads/issues/`. |
| 3        | Chat-header thread actions                                          | `ready-for-agent` | Implement with pinned-thread ticket 02              | Shared action-menu policy and the header entry point are included in the pinned-thread delivery.         |
| 4        | Configurable UI and monospace fonts                                 | `needs-triage`    | `/grill-with-docs`                                  | Decide whether font controls are a product goal before considering the larger theme editor.              |
| 5        | Project-icon picker                                                 | `needs-triage`    | `/grill-with-docs`                                  | Must include safe handling of user-provided SVGs in the same delivery slice.                             |
| 6        | Dedicated agents/workflows panel                                    | `needs-info`      | `/grill-with-docs`                                  | Define value beyond Laplus's existing first-class subagent work rows.                                    |
| 7        | Browser-panel recent sites                                          | `needs-triage`    | `/grill-with-docs`                                  | Small local-persistence feature with no known blocker.                                                   |
| 8        | Paginated thread history                                            | `needs-info`      | Measure first; `/grill-with-docs` only if justified | Requires evidence that large histories cause meaningful payload or rendering cost.                       |
| 9        | Make Sidebar V2 the default                                         | `needs-info`      | UI parity audit, then `/implement` if small         | Wait until saved drafts work in both sidebars and remaining parity gaps are known.                       |

## Settled design: saved-draft shelf

A **saved draft** is the term defined in the repository
[glossary](../../CONTEXT.md). The accepted behavior is:

- Match T3 upstream's saved-draft behavior.
- A draft becomes eligible only after the user adds text or an attachment.
- The draft is shown after the user leaves it; the fresh draft currently being
  typed is not immediately duplicated in the sidebar.
- Keep multiple saved drafts per project and order them newest first.
- Show the project and the first line of text, or an attachment count when
  there is no text.
- Clicking a row restores the full draft and its settings.
- The discard button deletes the draft immediately, without confirmation.
- Render the shelf in both `Sidebar` and `SidebarV2`.
- Keep drafts in the existing local UI store. Do not add server persistence or
  cross-device synchronization.

The upstream evidence and the corresponding Laplus seams are recorded in
[research.md](./research.md#2-surface-unsent-new-thread-drafts-in-the-sidebar).

## Features already present

Do not reopen these as upstream gaps without new evidence:

- Usage reporting
- Choosing a worktree or the current checkout
- Project icons configured through `t3.json`
- Basic subagent visibility in first-class work rows

## Maintenance

When a feature moves forward, link its spec or issue directory from its row and
update the status here. Do not expand undecided rows into implementation tickets:
run their listed design flow first.
