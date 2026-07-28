# Artifact size

Written by `cargo xtask release`. The project exists for these numbers, so
they are produced by the build that makes the thing rather than checked by
hand afterwards.

## What a developer downloads and what it costs them

|                    | size         | against 20–30 MB            |
| ------------------ | ------------ | --------------------------- |
| Installer (NSIS)   | **5.06 MB**  | 14.94 MB under the range    |
| Installed on disk  | **24.34 MB** | inside, 5.66 MB of headroom |
| Application binary | **24.26 MB** | inside, 5.74 MB of headroom |

The installed figure covers 3 files, and is what the installer left on disk, measured by running it and weighing the directory it made — which holds this artifact and nothing else, since ticket 30 moved the install out of the one a developer's own state is in. Upstream's Windows installer is **318 MB**, so this one is **62.9× smaller** and what it installs is **13.1× smaller**.

## How much Rust the server is

|                           | lines      |
| ------------------------- | ---------- |
| total                     | 32,412     |
| comments                  | 8,452      |
| `#[cfg(test)]` unit tests | 10,176     |
| blank                     | 1,620      |
| **production code**       | **12,164** |

The spec's signal to stop and re-evaluate is roughly 20,000 lines of Rust, and the figure it is about is **production code: 12,164**, 7,836 lines inside it. The total above it is mostly prose and unit tests — a third of this server by line is its own tests — and reporting _that_ against the signal would write a false alarm into every build.
