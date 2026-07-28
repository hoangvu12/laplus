# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |
| —                          | `done`               | Delivered; acceptance criteria all met   |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Because this repo uses the local-markdown tracker, a "label" is the value of the `Status:` line near the top of an issue file — not a tracker-native label object.

`done` is local to this repo and has no counterpart in the upstream five. It exists because the five are _triage_ roles, all of which describe work not yet delivered; without it a finished ticket has to wear a label that misdescribes it. Reserve `ready-for-human` for its documented meaning — work that requires human implementation — not for "finished, please review".

Edit the right-hand column to match whatever vocabulary you actually use.
