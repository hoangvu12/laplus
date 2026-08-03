# 03 — Verified app-managed cloudflared installation

**What to build:** When no compatible executable is available, let an administrative developer explicitly approve and install an identified official Cloudflare release into laplus's private data, then use it for the existing connector-token setup path.

**Blocked by:** 02 — Connector-token setup and supervision.

**Status:** done

- [x] The wizard offers app-managed installation only when appropriate and previews the exact version, platform, architecture, source, and ownership before download.
- [x] Installation and retry require `access:write`; read-only and ordinary paired sessions cannot learn download or local tool state beyond the scoped refusal.
- [x] Laplus downloads an identified official Cloudflare artifact and its published checksum, verifies both identity and digest, and rejects missing, malformed, mismatched, or unsupported artifacts.
- [x] Partial downloads are never executable; successful verification is atomically promoted into a private laplus-owned location without PATH changes or elevation.
- [x] Interrupted and failed downloads are resumable or safely retryable and leave a truthful wizard state after restart.
- [x] The installed executable passes the same compatibility check and can complete the connector-token setup, supervision, verification, and pairing path.
- [x] Laplus distinguishes its app-managed executable from system/user-selected executables and removes or replaces only the copy it owns.
- [x] The feature does not implement a separate cloudflared updater and remains correct if cloudflared's own behavior replaces the original process or executable.
- [x] Local fake release and checksum endpoints cover success, corruption, interruption, unsupported platform, retry, atomicity, permissions, and cleanup without contacting or executing live Cloudflare artifacts.

**Delivered:** `d15bab4`.

Two limits worth knowing rather than discovering:

- The install integration tests are `#![cfg(unix)]`, as ticket 02's are, because
  the fake release artifact is a script. On Windows only the unit coverage of the
  asset mapping and checksum parsing runs, so the download, promotion and
  permission behaviour is unproven there.
- macOS is not offered an app-managed installation at all. Cloudflare ships it
  only as a `.tgz` and unpacking an archive would be a second supply chain; the
  wizard says so and points at the executable picker.
