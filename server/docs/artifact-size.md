# Artifact size

Written by `cargo xtask release`. The project exists for these numbers, so
they are produced by the build that makes the thing rather than checked by
hand afterwards.

## What a developer downloads and what it costs them

|                    | size         | against 20–30 MB            |
| ------------------ | ------------ | --------------------------- |
| Installer (NSIS)   | **5.82 MB**  | 14.18 MB under the range    |
| Installed on disk  | **26.86 MB** | inside, 3.14 MB of headroom |
| Application binary | **26.86 MB** | inside, 3.14 MB of headroom |

The installed figure covers 2 files, and is what the bundle ships, weighed where it was built without installing it, so it does not count the uninstaller NSIS writes. Upstream's Windows installer is **318 MB**, so this one is **54.6× smaller** and what it installs is **11.8× smaller**.

## How much Rust the server is

|                           | lines      |
| ------------------------- | ---------- |
| total                     | 38,435     |
| comments                  | 10,170     |
| `#[cfg(test)]` unit tests | 12,058     |
| blank                     | 1,905      |
| **production code**       | **14,302** |

The spec's signal to stop and re-evaluate is roughly 20,000 lines of Rust, and the figure it is about is **production code: 14,302**, 5,698 lines inside it. The total above it is mostly prose and unit tests — a third of this server by line is its own tests — and reporting _that_ against the signal would write a false alarm into every build.
