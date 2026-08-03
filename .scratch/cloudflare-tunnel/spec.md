Status: ready-for-agent

# Cloudflare Tunnel setup and management

## Problem Statement

Laplus already works when an operator routes a Cloudflare tunnel to its loopback HTTP server, but creating, verifying, advertising, and maintaining that route requires terminal work and knowledge that the Connections UI does not expose. Developers cannot tell whether `cloudflared` is usable, whether a connector is merely ready or the public endpoint truly reaches this environment, who owns an existing tunnel, or what will happen when they stop or remove it.

That gap is especially risky because a Cloudflare hostname can expose laplus to the public Internet. A convenient setup flow must not blur the distinction between laplus authentication and Cloudflare Access, retain broad Cloudflare account authority unnecessarily, let an ordinary paired client administer public exposure, or let laplus compete with an externally managed connector.

## Solution

Add a Cloudflare Tunnel row to Settings → Connections. The compact row summarizes setup progress, endpoint verification, connector health, ownership, and the next useful action. A resumable modal wizard handles tool selection or installation, Cloudflare browser authorization, new dedicated-tunnel creation, inactive-tunnel adoption, connector-token setup, and hostname-only registration of an external tunnel endpoint.

Laplus supervises connectors it starts, persists stable setup state, verifies the public HTTPS and WebSocket paths independently of local connector readiness, advertises only a verified endpoint for pairing, and makes destructive Cloudflare actions separate and explicit. Existing active connectors remain externally managed. All Cloudflare state is readable only by an administrative session with `access:read`, and every mutation or executable/network lifecycle action requires `access:write`.

## User Stories

1. As a developer, I want to see Cloudflare Tunnel beside other connection options, so that remote access is discoverable without a terminal.
2. As a developer, I want the Connections page to remain compact, so that an occasional setup flow does not dominate routine settings.
3. As a developer, I want to see the current wizard step in the compact row, so that an interrupted setup is visibly resumable.
4. As a developer, I want to reopen an incomplete setup after restarting laplus, so that a browser login or failed command does not force me to begin again.
5. As a developer, I want laplus to detect a compatible `cloudflared`, so that an existing system installation is reused.
6. As a developer, I want to choose a specific compatible executable, so that I can use an installation outside standard discovery locations.
7. As a developer, I want incompatible executables rejected with their detected version, so that failures are actionable.
8. As a developer, I want laplus to offer installation when no compatible executable exists, so that setup can remain terminal-free.
9. As a security-conscious developer, I want to approve the exact Cloudflare release before download, so that executable installation is intentional.
10. As a security-conscious developer, I want the downloaded executable identified and checksum-verified, so that laplus does not run an unverified artifact.
11. As a developer, I want an app-managed executable isolated in laplus data, so that setup does not change PATH or require elevation.
12. As a developer, I want laplus never to overwrite or remove my system or selected executable, so that ownership remains clear.
13. As a developer, I want a failed or interrupted download to be retryable, so that no partial executable is treated as installed.
14. As a developer, I want Cloudflare browser authorization launched from the wizard, so that I do not need to copy terminal commands.
15. As a security-conscious developer, I want a warning that the Cloudflare account certificate has account-wide authority, so that consent is informed.
16. As a developer, I want laplus to use a pre-existing account certificate only after consent, so that merely detecting it grants no authority.
17. As a developer, I want laplus never to copy, replace, move, or delete the Cloudflare account certificate, so that cloudflared-owned account state stays intact.
18. As a developer, I want a timed-out or cancelled browser authorization to remain resumable, so that no tunnel mutation occurs accidentally.
19. As a developer, I want to create a stable dedicated tunnel in the wizard, so that laplus has a durable public hostname.
20. As a developer, I want to preview the tunnel name, public hostname, DNS change, and loopback target before creation, so that I understand the mutation.
21. As a developer, I want setup to record the exact tunnel and DNS resources it created, so that recovery and cleanup target only those resources.
22. As a developer, I want the narrow tunnel credential stored privately, so that steady-state connector operation does not require account-wide authority.
23. As a developer, I want to adopt an inactive existing tunnel only after explicit confirmation that it is dedicated to this environment, so that laplus has one lifecycle owner.
24. As an operator, I want an active existing tunnel treated as external, so that laplus cannot start a competing connector with different ingress.
25. As a developer, I want to configure a remotely managed tunnel using a connector-token file, so that Cloudflare retains control-plane ownership and laplus receives only run authority.
26. As a security-conscious developer, I want connector secrets passed by file rather than command-line argument, so that they do not appear in process listings.
27. As an operator, I want to register only an HTTPS hostname for an externally managed connector, so that laplus can verify and advertise it without taking lifecycle ownership.
28. As a developer, I want hostnames normalized and restricted to HTTPS, so that the advertised endpoint always derives a secure WebSocket URL.
29. As a security-conscious developer, I want verification restricted to the configured public hostname, so that the health checker cannot become an arbitrary URL or SSRF probe.
30. As a developer, I want local connector readiness shown separately from public endpoint verification, so that “connected” never overstates what works.
31. As a developer, I want public verification to prove the environment identity, an authenticated HTTP challenge, and a WebSocket upgrade, so that pairing is offered only for the real end-to-end path.
32. As a developer, I want verification to use a one-time diagnostic credential, so that a durable administrator credential is never sent through a probe.
33. As a developer, I want a Cloudflare Access interception reported distinctly, so that an HTML login redirect is not mistaken for a tunnel or laplus failure.
34. As a developer, I want the last successful verification time and stale status shown, so that transient failures are distinguishable from a never-working endpoint.
35. As a developer, I want a Test now action, so that I can recheck the endpoint after changing Cloudflare or DNS configuration.
36. As a developer, I want bounded background verification with backoff and jitter, so that degraded connectivity does not create a request storm.
37. As a developer, I want a verified tunnel endpoint advertised with its ownership and layered health, so that the connection list tells the truth.
38. As a developer, I want to mint and copy or display a pairing URL and QR code for the verified endpoint, so that I can connect another device immediately.
39. As a paired phone user, I want to use a verified endpoint without gaining Cloudflare administration access, so that pairing follows least privilege.
40. As a non-administrative paired user, I want Cloudflare status and account details withheld, so that public exposure metadata is not leaked.
41. As a non-administrative paired user, I want Cloudflare mutations refused with only an administrator-required message, so that lack of authority is clear without disclosing state.
42. As an administrator with `access:read`, I want to inspect setup, ownership, and health without mutating them, so that observation is independently authorized.
43. As an administrator with `access:write`, I want to install, authenticate, create, adopt, configure, start, stop, retry, and remove exposure, so that every dangerous operation is explicitly authorized.
44. As a developer, I want a laplus-managed connector to start with its owning shell or server, so that a stable tunnel survives application or service restart.
45. As a developer, I want connector readiness preserved across cloudflared self-replacement, so that supervision does not assume the original PID lives forever.
46. As a developer, I want bounded restarts and actionable logs after connector failure, so that crashes neither stay silent nor loop forever.
47. As a developer, I want graceful connector shutdown when its owning laplus process stops, so that no orphan process continues exposure.
48. As an operator, I want an external connector left running when laplus stops, so that laplus does not interfere with another supervisor.
49. As a developer, I want to turn off a laplus-managed connector without deleting its tunnel or DNS route, so that exposure can be paused safely.
50. As a developer, I want to forget local setup separately from deleting Cloudflare resources, so that local cleanup never implies remote destruction.
51. As a developer, I want deletion of a laplus-created tunnel and its exact DNS record to require a separate confirmation and sufficient Cloudflare authority, so that destructive scope is explicit.
52. As a developer, I want adopted tunnels and external endpoints never offered for Cloudflare deletion, so that laplus deletes only resources it created.
53. As a developer, I want partial setup and partial cleanup failures journaled, so that retry resumes from observed state rather than repeating completed mutations.
54. As a developer, I want cancellation to leave a truthful recovery state, so that the wizard never claims rollback it could not complete.
55. As a developer, I want logs and errors to redact account certificates, tunnel credentials, connector tokens, pairing credentials, and diagnostic credentials, so that troubleshooting does not leak authority.
56. As a headless operator, I want stable connector supervision owned by `laplus-server`, so that the same setup works when laplus runs under systemd without installing a second cloudflared service.
57. As a desktop user, I want the shell to own connector supervision when it owns the server, so that closing the application has one predictable lifecycle.
58. As a developer, I want setup to explain that an unprotected Cloudflare hostname is public and laplus authentication remains mandatory, so that the security boundary is understood.
59. As a developer, I want Cloudflare Access described as potentially intercepting laplus flows rather than as guaranteed protection, so that the UI does not claim unsupported compatibility.

## Implementation Decisions

- The feature uses the existing Connections page and endpoint vocabulary. Its structure is the selected prototype variant: a compact Settings row opens a modal wizard; the row retains resumable progress, layered connector/HTTPS/WebSocket health, pairing, and ownership.
- A single public-exposure service owns Cloudflare contracts, durable setup state, command execution, verification, and connector supervision. Desktop shell and headless server supply lifecycle ownership to that service rather than implementing separate Cloudflare behavior.
- The client receives a closed snapshot/state-machine contract rather than interpreting raw cloudflared output. The snapshot distinguishes tool availability, setup phase, ownership, desired connector state, actual readiness, endpoint verification, last success, recoverable failure, and permitted next actions.
- Mutations are idempotent commands with explicit intent. Repeating a command after disconnect or restart reads the persisted journal and reconciles observed Cloudflare/local state before acting.
- Durable setup records include the configured hostname, loopback target, ownership classification, tunnel identifier where known, exact DNS resource where laplus created one, credential/config locations, executable selection and ownership, desired running state, setup/cleanup journal, and verification timestamps. Secrets are stored only in private files; contracts and ordinary persistence expose references and redacted metadata, never secret contents.
- Tool resolution prefers a compatible system executable, then a compatible user-selected executable, then an app-managed executable. Compatibility is versioned policy rather than “command exists.”
- App-managed installation requires explicit approval, an identified official release for the current platform and architecture, verification against Cloudflare's published checksum, atomic promotion from a partial download, private placement, and no PATH or elevation changes. Laplus never overwrites or removes executables it does not own.
- Laplus does not provide a cloudflared update product. It tolerates the app-managed executable's built-in replacement behavior and re-resolves readiness/process identity during supervision.
- Cloudflare browser login and the account certificate are used only for explicit account-management actions. A detected certificate is used in place only after consent. Laplus never copies, replaces, moves, or deletes it.
- A new laplus-created tunnel is locally managed, dedicated to one environment, configured by a laplus-owned YAML file, and run with its tunnel-specific credential. Creation previews and journals the tunnel and DNS mutations before execution.
- An inactive existing tunnel may become an adopted tunnel only after explicit dedication confirmation. An active tunnel becomes an external tunnel endpoint and cannot be reconfigured, started, restarted, stopped, or deleted by laplus.
- A remotely managed tunnel uses a tunnel-specific connector token supplied through a private token file. The token never appears in command arguments, logs, snapshots, or errors.
- Hostname-only registration creates an external tunnel endpoint. It grants verification and advertisement behavior only and never process or Cloudflare-resource ownership.
- Connector readiness uses a dedicated loopback metrics address and `/ready`. Readiness proves only an active connector-to-Cloudflare connection.
- Endpoint verification is a separate layered operation: fetch the configured HTTPS hostname without redirects and match its public descriptor's environment identity; then complete one-time authenticated HTTP and WebSocket challenges. The verifier accepts no arbitrary URL, rejects non-public destinations after resolution, and uses only short-lived diagnostic authority.
- Cloudflare Access redirects or HTML interception are a distinct verification outcome. Endpoint verification also distinguishes DNS/TLS/identity/authentication/WebSocket failures, stale last success, and local connector failure.
- Only verified endpoints are advertised as available pairing endpoints. The endpoint derives `wss://` from the fixed `https://` origin and carries tunnel ownership and layered health for presentation.
- Background public verification is less frequent than local readiness polling and uses bounded exponential backoff with jitter. Test now requests an immediate bounded verification without creating concurrent probe storms.
- `access:read` gates every Cloudflare snapshot and status response. `access:write` gates executable download or selection, browser authorization, Cloudflare mutation, secret import, connector start/stop, verification trigger, forget, and deletion. A refusal reveals only the required administrative scope, not Cloudflare state. Desktop and headless administrative boot grants carry these scopes; ordinary pairing does not gain them by default.
- Stable laplus-managed connectors start with their owner, restart with a bounded policy, expose redacted logs and failure state, and shut down gracefully with the owner. A connector that remains unhealthy after the budget is exhausted requires an explicit retry. External connectors are never supervised.
- Stopping exposure changes desired connector state but preserves tunnel, DNS, configuration, and credential state. Forget removes only laplus-owned local configuration and secrets after stopping its connector. Delete everywhere is offered only for a laplus-created tunnel, separately confirms the exact tunnel and DNS record, reacquires/uses sufficient account and DNS authority, journals each remote deletion, and reports partial cleanup honestly.
- The UI does not describe Cloudflare Access as supported end-to-end. It states that the hostname is public unless independently protected and that laplus authentication remains required.

## Testing Decisions

- Tests assert externally visible behavior and ownership boundaries, not implementation details. The primary seam is a running server exercised through its public HTTP/RPC contracts with a fake cloudflared executable and local fake Cloudflare, download, public-HTTPS, and WebSocket peers.
- Contract tests cover every closed snapshot state, command input, ownership kind, layered health outcome, refusal, and redaction rule shared between server and client.
- Server integration tests cover scope enforcement, compatible executable selection, verified atomic installation, browser-login progress, new creation, inactive adoption, active external classification, token-file use, hostname registration, idempotent resume, persistence across restart, supervision/restart budget, graceful shutdown, and ownership-safe cleanup.
- Verification integration tests cover readiness independently from public identity, redirects disabled, public-address restrictions, wrong environment identity, one-time HTTP challenge, WebSocket upgrade, Cloudflare Access interception, stale success, manual retry, and backoff behavior without wall-clock assertions.
- Existing pairing/auth HTTP tests are prior art for real session scopes and refusal bodies. Existing owned-versus-external OpenCode process tests are prior art for restart, adoption, shutdown, and non-interference boundaries. Existing server restart tests are prior art for persistence.
- Focused Connections-page logic and component tests cover the compact summary, resumable wizard transitions, permitted actions, ownership language, warnings, errors, destructive confirmations, and pairing selection from a verified endpoint.
- One UI-driver walkthrough exercises the compact row and modal wizard against the server harness, including an interrupted/resumed setup, verified endpoint advertisement, pairing, and stop/forget separation. The browser reads server state or wire traffic for assertions rather than trusting labels alone.
- Download and command fixtures contain no live credentials. Tests assert that command arguments, snapshots, logs, errors, and persisted non-secret records do not contain supplied secret values.

## Out of Scope

- Cloudflare Quick Tunnels or other temporary-tunnel UX.
- A hosted relay/control plane, Cloudflare billing, account creation, or zone purchase.
- General Cloudflare account, DNS, Zero Trust, or Access policy management.
- Claiming or implementing transparent Cloudflare Access compatibility for browser pairing, RPC, and long-lived WebSockets.
- Editing the user's default cloudflared configuration or installing a system service.
- Managing, restarting, reconfiguring, or deleting active externally managed connectors.
- Automatically updating system or user-selected cloudflared installations, or building a separate updater for app-managed copies.
- Automatically revoking Cloudflare account tokens or deleting account certificates.
- Adding Tailscale daemon management or changing existing generic external HTTPS endpoint behavior beyond integrating verified Cloudflare endpoints.
- Promoting code from the throwaway prototype into production.

## Further Notes

- This specification follows ADR-0045 for explicit existing-tunnel ownership, ADR-0046 for the app-managed executable supply chain, ADR-0047 for `access:read`/`access:write` enforcement, and ADR-0048 for connector supervision.
- The UI structure and retained interaction elements trace to the approved Variant A prototype verdict. The prototype commit remains visual evidence only.
- Cloudflare CLI capabilities, credential powers, readiness semantics, verification constraints, and cleanup asymmetry trace to the feature research. Where research recommendations predate ADR-0045, the accepted ADR controls.
- The first implementation slice should prove one narrow, unblocked end-to-end path through real contracts rather than begin with a layer-wide Cloudflare abstraction.
