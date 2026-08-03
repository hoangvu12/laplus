# 04 — Cloudflare sign-in and existing-tunnel discovery

**What to build:** Let an administrative developer authorize Cloudflare in the browser, knowingly use a cloudflared-owned account certificate, discover existing tunnels, and choose the correct next path based on whether a tunnel is active or inactive.

**Blocked by:** 02 — Connector-token setup and supervision.

**Status:** ready-for-agent

- [ ] The modal wizard launches and tracks cloudflared browser authorization without requiring terminal input, and cancellation, timeout, process failure, or restart leaves setup resumable.
- [ ] Before using a detected account certificate, the wizard explains its broad, long-lived account authority and requires explicit consent.
- [ ] Laplus uses an account certificate only in place for the requested account-management action and never copies, replaces, moves, deletes, or exposes it.
- [ ] Sign-in, certificate use, listing, and refresh require `access:write`; Cloudflare state remains hidden from sessions without `access:read`.
- [ ] Authenticated discovery parses structured tunnel identifiers, names, timestamps, and connection state without inferring a hostname or management mode the output does not provide.
- [ ] The wizard asks for and verifies the public hostname rather than inventing it from tunnel metadata.
- [ ] An active existing tunnel is classified as an external tunnel endpoint and can follow the external verification/pairing path without any laplus lifecycle or configuration action.
- [ ] An inactive existing tunnel is offered for explicit dedication/adoption but is not treated as laplus-managed until that later confirmation succeeds.
- [ ] Repeated login/list operations and interrupted discovery reconcile current state without duplicating Cloudflare mutations.
- [ ] Fake cloudflared integration and UI coverage prove consent, structured parsing, active/inactive branching, restart recovery, refusal behavior, and certificate secrecy.
