Status: ready-for-human

# Cloudflare Tunnel and Tailscale remote-access research

Written 2026-08-02. The question this answers is: **does current laplus/T3
support Tailscale, and should laplus add Cloudflare Tunnel support?**

Upstream was read at commit
[`e60821f0e0d82a5d671ca3b94719c49d333921c8`](https://github.com/pingdotgg/t3code/tree/e60821f0e0d82a5d671ca3b94719c49d333921c8)
(2026-08-02). GitHub issue and PR searches were performed with `gh`; Cloudflare
and Tailscale claims below come from their official documentation.

## Answer

**T3 upstream supports Tailscale directly. Laplus supports Tailscale and
Cloudflare Tunnel as external transports, but does not manage either daemon.**

That distinction matters:

- Upstream discovers Tailscale endpoints, exposes a Settings toggle, and runs
  `tailscale serve` itself. Its CLI also has `t3 serve --tailscale-serve` and
  `t3 pair --tailscale`. See the current upstream
  [remote-access guide](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/docs/user/remote-access.md),
  [remote architecture](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/docs/internals/remote.md), and
  [`@t3tools/tailscale` wrapper](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/packages/tailscale/src/tailscale.ts).
- Laplus deliberately removed `packages/tailscale`, returns
  `tailscaleServeEnabled: false`, and starts no `tailscale serve`. A tailnet IP
  can reach a network-bound server, while a MagicDNS HTTPS URL is handled like
  any other external tunnel hostname. This is recorded in
  [`ADR-0018`](../../server/docs/adr/0018-the-fork-stops-being-a-fork.md),
  [`endpoints.rs`](../../server/crates/laplus-server/src/endpoints.rs), and the
  [shell bridge](../../server/crates/laplus-shell/src/main.rs).
- Laplus already works behind `cloudflared`: keep laplus bound to loopback,
  point the tunnel at its local HTTP port, and pair using the public HTTPS URL.
  The server's bearer authentication, CORS, HTTPS-to-WSS URL derivation, and
  externally supplied base/advertised host were built and exercised for this
  case. See [`ADR-0019`](../../server/docs/adr/0019-a-tunnel-dissolves-the-loopback-boundary.md)
  and [running headless](../../server/docs/running-headless.md).

So the immediate gap is **setup and discovery UX**, not protocol compatibility.

## What upstream actually ships

### Tailscale is a local endpoint provider

Upstream treats Tailscale as a transport/provider rather than a distinct saved
environment type. It reads `tailscale status --json`, advertises the 100.x IP
and MagicDNS endpoint, and can acquire/release a Serve mapping for the server's
actual listening port. The mapping is opt-in and uses HTTPS port 443 by default
([architecture](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/docs/internals/remote.md),
[user guide](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/docs/user/remote-access.md)).

That matches Tailscale's own model: Serve proxies a local service over an
automatically provisioned TLS certificate and remains restricted by tailnet
access-control rules. Public exposure is a different product, Funnel
([Tailscale Serve docs](https://tailscale.com/docs/features/tailscale-serve),
[CLI reference](https://tailscale.com/docs/reference/tailscale-cli/serve)).

### Upstream also uses Cloudflare, but as T3 Connect

Current upstream says that direct, bearer-paired, Tailscale, SSH, and
**relay-tunneled** access all exist. The Cloudflare tunnel belongs to T3
Connect's hosted relay control plane; it is not a generic "use my Cloudflare
account" Settings provider
([remote architecture](https://github.com/pingdotgg/t3code/blob/e60821f0e0d82a5d671ca3b94719c49d333921c8/docs/internals/remote.md)).

The original managed-relay change provisions Cloudflare endpoints and bundles
`cloudflared` ([PR #2837](https://github.com/pingdotgg/t3code/pull/2837)). A later
change made the server ask the relay to delete the Cloudflare tunnel at shutdown
while retaining the stable hostname allocation
([PR #4531](https://github.com/pingdotgg/t3code/pull/4531)). That lifecycle
requires upstream's relay APIs, identity, DNS ownership, connector-token
issuance, allocation database, and billing policy. It is not portable as a
small local CLI wrapper.

There is an upstream proposal to let Connect use an independently managed
endpoint rather than a relay-provisioned Cloudflare tunnel
([issue #4783](https://github.com/pingdotgg/t3code/issues/4783)). This reinforces
the separation between a user's existing tunnel and the hosted managed-tunnel
product.

## What Cloudflare requires

A Cloudflare Tunnel is technically a good fit for laplus. `cloudflared` makes
outbound-only connections, so laplus can remain on `127.0.0.1` and no inbound
firewall port is required
([Cloudflare Tunnel overview](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/),
[firewall guidance](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/tunnel-with-firewall/)).
For a named tunnel, the public hostname routes to a service such as
`http://localhost:4773`; Cloudflare terminates public HTTPS and forwards HTTP to
laplus
([create a remotely-managed tunnel](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel/)).

There are three materially different integration levels:

1. **External named tunnel:** the user provisions the tunnel/DNS and runs
   `cloudflared`; laplus only accepts or remembers its `https://` base URL.
   This already works.
2. **Quick Tunnel:** laplus spawns
   `cloudflared tunnel --url http://localhost:4773`, parses the random
   `trycloudflare.com` URL, advertises it, and owns the child process. Cloudflare
   explicitly limits Quick Tunnels to testing/development and recommends a
   remotely managed tunnel for production
   ([Quick Tunnels](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/trycloudflare/)).
3. **Managed named tunnel:** laplus provisions a tunnel, DNS route, connector
   token, and service lifecycle through Cloudflare's API. That needs account and
   zone identifiers plus an API token with tunnel and DNS write privileges
   ([API setup](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/get-started/create-remote-tunnel-api/)).
   Locally managed tunnels instead leave an account-wide `cert.pem` and
   tunnel-specific credentials on disk; Cloudflare warns that the account
   certificate can create, delete, and manage all tunnels in the account
   ([tunnel permissions](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/tunnel-permissions/)).

The third option therefore introduces secret storage, OAuth/API-token setup,
DNS ownership, daemon install/update behavior, crash recovery, and cleanup. It
is much larger and riskier than upstream's Tailscale wrapper.

## Security and compatibility boundaries

Cloudflare Tunnel publishes an application hostname to the Internet unless an
Access policy is added; Cloudflare's setup guide explicitly says that anyone on
the Internet can access the hostname after publishing it. Laplus authentication
must remain enabled regardless. Cloudflare Access may be a useful second gate,
but it is not transparent to every laplus flow: Access checks for a
`CF_Authorization` cookie and redirects unauthenticated browser requests
([authorization cookie](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/)).

Upstream has an open report that remote-environment pairing cannot complete
behind Cloudflare Access OAuth because discovery/fetch requests receive the
Access login page rather than JSON
([issue #3736](https://github.com/pingdotgg/t3code/issues/3736)). Any laplus UI
that recommends Access should first test page load, token exchange, RPC, and
long-lived WebSocket reconnects through it; it should not claim Access support
merely because an unprotected Tunnel works.

Quick Tunnels should never be presented as the secure default. They create a
public random hostname and Cloudflare labels them development-only. A stable
named tunnel plus an explicit access policy is the production shape; a private
Tailscale Serve endpoint remains the simpler default for personal remote access.

## Recommendation

**Do not build a Cloudflare equivalent of the Tailscale toggle yet. Add a small
external-tunnel slice first.**

1. Document the already-supported setup: keep laplus on loopback, route a named
   Cloudflare Tunnel to `http://127.0.0.1:<port>`, then mint/copy a pairing URL
   with the tunnel's `https://` base URL. State clearly that laplus auth remains
   mandatory and that an unprotected hostname is public.
2. Make the existing custom tunnel-host/base-URL UX explicit in Connections:
   label it "External HTTPS tunnel", validate `https://`, derive `wss://`, and
   show it as a `tunnel` advertised endpoint. This is provider-neutral and also
   serves reverse proxies and user-managed Tailscale MagicDNS.
3. Optionally add a **developer-only** `--cloudflare-quick-tunnel` later. Own
   the child process, parse the assigned URL without log scraping if
   `cloudflared` exposes a structured output, terminate it with laplus, and warn
   that its hostname changes and is public. Do not persist it as a production
   endpoint.
4. Defer account-managed named-tunnel provisioning until there is a deliberate
   product decision to own Cloudflare credentials and tunnel/DNS lifecycle.
   If that decision is made, design it as a separate endpoint-provider module,
   not core server auth, and spike Cloudflare Access compatibility first.

This gives users the useful Cloudflare path immediately without turning laplus
into a Cloudflare account control plane. It also preserves the architectural
shape upstream documents: core pairing accepts any HTTPS endpoint; transport
providers contribute discovery and lifecycle only when the project truly owns
them.

## Stable tunnel UX follow-up (2026-08-02)

This follow-up checked the current installed `cloudflared` 2026.7.3 CLI and
upstream source at
[`3a2b45c2`](https://github.com/cloudflare/cloudflared/tree/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc).
The important correction is that **having `cloudflared` installed is not by
itself enough to discover existing tunnels**.

### What the CLI can automate

After `cloudflared tunnel login`, laplus can invoke the complete locally-managed
workflow without showing a terminal:

1. `cloudflared tunnel list --output json` returns the account's active tunnels
   with IDs, names, creation/deletion timestamps, and connections.
2. `cloudflared tunnel create --output json --credentials-file <path> <name>`
   creates a tunnel and emits structured output while writing a tunnel-specific
   credentials file.
3. `cloudflared tunnel route dns <tunnel> <hostname>` creates the CNAME route.
4. A laplus-owned YAML config can route that hostname to
   `http://127.0.0.1:<laplus-port>`, and
   `cloudflared tunnel --config <path> run <uuid>` starts it.

These commands and their roles are documented in Cloudflare's
[useful-command reference](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/tunnel-useful-commands/).
The current CLI source confirms structured `json`/`yaml` output for `list` and
`create`, and that list JSON serializes the tunnel API model
([list implementation](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/cmd/cloudflared/tunnel/subcommands.go),
[JSON model](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/cfapi/tunnel.go)).

The login command opens Cloudflare's browser authorization flow and writes
`cert.pem`. It refuses to overwrite an existing certificate, so laplus must
detect and offer to use it rather than moving or deleting it. A browser-download
fallback can require the user to place the certificate manually
([login implementation](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/cmd/cloudflared/tunnel/login.go)).

### Authentication and discovery limits

`cloudflared tunnel list` is an authenticated account-management API call. It
requires the account certificate (`cert.pem`) from `cloudflared tunnel login`;
neither the installed executable, a tunnel-specific UUID JSON credentials file,
nor a remotely-managed connector token grants account-wide listing. Cloudflare
says `cert.pem` can create, route, delete, and list all tunnels in the account,
while a `<UUID>.json` credential can only run its one tunnel
([tunnel permissions](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/tunnel-permissions/)).

The list result also has no management-mode or hostname field. The current
serialized model contains ID, name, timestamps, and connections, but no
`config_src`; therefore laplus cannot reliably label a listed item as local or
remote, or infer its public hostname, from `tunnel list --output json` alone.
The UI must still ask for or verify the hostname.

There are two different run credentials:

- A locally-managed tunnel gets `<UUID>.json` from `tunnel create`; it is
  tunnel-specific and non-expiring. Its ingress configuration is local YAML.
- A remotely-managed tunnel runs with a connector token, while ingress
  configuration stays at Cloudflare. `--token-file` is supported for remote
  tunnels in cloudflared 2025.4.0 and later
  ([run parameters](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/run-parameters/)).

Consequently, a service installed with only a tunnel token can run that one
tunnel but cannot use `cloudflared tunnel list --output json` to discover the
account's other tunnels.

With a valid account certificate, current `cloudflared` also exposes
`cloudflared tunnel token <name-or-uuid>` and can write credentials with
`--cred-file`; the CLI help warns this only works for tunnels created since
2022.3.0. This retrieves the token from Cloudflare—it does not discover a token
already stored somewhere on the machine. Without account-management auth, an
existing remote tunnel must be imported using a token/token file supplied by
the user. Cloudflare's API likewise requires Tunnel Write permission to fetch a
connector token
([token API](https://developers.cloudflare.com/api/resources/zero_trust/subresources/tunnels/subresources/cloudflared/subresources/token/methods/get/)).

### Local versus remote management

Cloudflare now recommends remotely-managed tunnels for most uses because their
configuration lives at Cloudflare and is manageable through the dashboard,
API, or Terraform. `cloudflared tunnel create` creates the alternative,
locally-managed form, which Cloudflare positions for development, testing, and
legacy configurations
([local-management overview](https://developers.cloudflare.com/tunnel/advanced/local-management/)).
Consequently the CLI-only `login → create → route dns → run` wizard is feasible
and smooth, but it creates the non-preferred management model; the installed CLI
does not offer a command that creates and configures a remotely-managed tunnel
with narrowly scoped authorization.

Creating a remote tunnel directly through Cloudflare's API would require laplus
to collect an API token with Tunnel Write and DNS Write, plus account and zone
IDs, then create the tunnel, upload ingress configuration, and create DNS
([API setup](https://developers.cloudflare.com/tunnel/setup/)). That is more
control-plane authority and secret handling than simply running a connector.

### Existing configs and services

Laplus must not edit `~/.cloudflared/config.yml`. It should pass an explicit
laplus-owned `--config` path and, when creating a local tunnel, an explicit
`--credentials-file` path. Cloudflare documents several default config search
locations and warns that service installation under `sudo` changes `$HOME`,
which commonly makes credentials appear missing
([configuration terms](https://developers.cloudflare.com/tunnel/advanced/local-management/local-tunnel-terms/),
[Linux service guide](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/as-a-service/linux/)).

Before starting a connector, the UI should detect an existing cloudflared
process/service and ask whether it is externally managed. Multiple replicas are
valid, but Cloudflare permits only one installed cloudflared service per host
and advises adding routes to the existing tunnel rather than installing a
second service
([replica guidance](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/configure-tunnels/tunnel-availability/deploy-replicas/),
[service troubleshooting](https://developers.cloudflare.com/cloudflare-one/troubleshooting/tunnel/)).
An externally managed connector should be registered by hostname and verified;
laplus should not restart or rewrite it.

### Recommended stable-tunnel UI

Use progressive choices based on the detected authentication state:

1. **No `cert.pem`:** show **Sign in to discover or create tunnels** and
   **Connect with a tunnel token**. Explain that sign-in grants this computer
   account-wide tunnel and DNS management. Token import is the least-privilege
   existing-tunnel path and should use `--token-file`, never a command-line
   token.
2. **`cert.pem` present:** run `tunnel list --output json`, show selectable
   names/IDs and connection health, then ask for the public hostname. For a
   selected tunnel, retrieve tunnel-specific run credentials only after
   explicit confirmation. Never infer ownership merely because the tunnel was
   listed.
3. **New stable tunnel:** offer a fully in-app browser-login wizard, but label it
   **locally managed on this computer**. Ask for tunnel name and hostname,
   preview the DNS change and local target, then run create, route, and verify.
   Laplus owns only the config/credential files and connector process it
   created; deletion of the Cloudflare tunnel or DNS record is a separate,
   explicit action.
4. **Recommended stable setup:** make **Create in Cloudflare / paste connector
   token** the recommended remote-managed route. It has one dashboard handoff
   but leaves account configuration with Cloudflare and gives laplus only the
   tunnel-specific run secret. The CLI-login route is the convenience option,
   not the least-privilege default.
5. **Already running elsewhere or as a service:** accept only the HTTPS
   hostname, verify HTTP identity and WebSocket upgrade, and record external
   ownership.

Thus existing-tunnel discovery can feel native when the user is already logged
in locally, but the product must not claim that all installed-cloudflared users
can simply pick from a list. The honest fallback is connector-token import or
hostname-only registration, and the best default for a new production-stable
tunnel remains remotely managed rather than retaining an account-wide
`cert.pem` merely to eliminate one dashboard step.

## Tunnel verification follow-up (2026-08-02)

`cloudflared` should be started with an explicit loopback metrics address and
port, then polled at `/ready`. Cloudflare documents that this endpoint returns
HTTP 200 only while the connector has an active connection to the Cloudflare
network. That is useful local connector readiness, but it does **not** prove the
configured DNS route, ingress rule, laplus origin, authentication, or public
WebSocket path is working
([tunnel metrics](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/monitor-tunnels/metrics/)).
Cloudflare Tunnel officially supports proxied WebSockets, so verification
should exercise a real `wss://` upgrade rather than infer it from an HTTP check
([WebSocket support](https://developers.cloudflare.com/network/websockets/)).

Treat **Connected** as a layered result:

1. Confirm local `/ready` first.
2. Fetch the fixed, configured HTTPS hostname's unauthenticated public
   descriptor and require its persisted `environmentId` to equal the local
   laplus environment ID.
3. Run purpose-built, one-time diagnostic HTTP and WebSocket challenges to
   prove authentication and the upgraded data path. Never send a long-lived
   administrator credential through this probe.

The public probes must accept only the already configured hostname: HTTPS only,
redirects disabled, with no caller-supplied URL or hostname. Resolve and reject
private, loopback, link-local, or otherwise non-public destinations while
allowing the configured Cloudflare hostname's normal public resolution. This
keeps the health checker from becoming an SSRF primitive. A Cloudflare Access
redirect or HTML login response is a distinct **Access intercepted
verification** failure, not an identity success; Access redirects
unauthenticated requests that lack its authorization cookie
([Access cookies](https://developers.cloudflare.com/cloudflare-one/identity/authorization-cookie/)).

Poll local readiness frequently and public identity/challenge checks less
often, with bounded exponential backoff and jitter after failures. Preserve the
last successful public verification time, expose **Test now**, and report stale
verification separately from an actively failing connector.

## Locally managed credential lifecycle (2026-08-02)

Cloudflare's two local-tunnel credentials have deliberately different powers.
`cert.pem` is an account certificate issued by `cloudflared tunnel login`; it
contains the selected zone ID, account ID, and a Cloudflare API token (encoded
inside an `ARGO TUNNEL TOKEN` PEM block). It can create, list, route, and delete
all tunnels in that account. Cloudflare says the certificate is valid for at
least ten years and its service token remains valid until revoked. Revocation
means deleting the corresponding Cloudflare Tunnel/Argo Tunnel API token in the
dashboard; deleting the local file only removes this machine's copy
([Tunnel permissions](https://developers.cloudflare.com/tunnel/advanced/local-management/tunnel-permissions/),
[source representation](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/credentials/origin_cert.go)).

`<TUNNEL-UUID>.json`, created by `cloudflared tunnel create`, is the
tunnel-specific credential. It does not expire and authorizes only running that
tunnel. Consequently a locally managed tunnel continues running after
`cert.pem` is removed, provided its config still points at the UUID credential
JSON. The account certificate is required for account-management operations
such as create, list, route DNS, token retrieval, and tunnel deletion, but not
for `tunnel run`
([Tunnel permissions](https://developers.cloudflare.com/tunnel/advanced/local-management/tunnel-permissions/),
[local tunnel terms](https://developers.cloudflare.com/tunnel/advanced/local-management/local-tunnel-terms/)).

A later `cloudflared tunnel login` can issue a new account certificate, and
because tunnel ownership is account-bound rather than certificate-bound, that
new certificate can manage the account's existing tunnels. This is also
Cloudflare's documented recovery after revocation or a stale user API key.
However, the current CLI login has an important UX constraint: it always writes
`cert.pem` in the first default cloudflared directory, has no output-path flag,
and refuses to overwrite a non-empty existing certificate. The global
`--origincert` option selects a certificate for subsequent management commands;
it does not change login's output path
([login source](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/cmd/cloudflared/tunnel/login.go),
[run parameters](https://developers.cloudflare.com/tunnel/advanced/run-parameters/)).
Laplus therefore must never delete, move, or replace a pre-existing default
certificate. If one exists, use it in place after explicit consent. If none
exists and laplus initiates login, record that laplus created that exact file
before treating it as disposable; the CLI cannot directly write it into a
laplus-private path.

`cloudflared tunnel route dns` creates the CNAME and requires account
authorization. The CLI exposes no symmetric `route dns delete` command. Tunnel
deletion also requires the account certificate, while deleting the DNS record
is a separate Cloudflare DNS API/dashboard operation and requires appropriate
DNS authority
([useful commands](https://developers.cloudflare.com/tunnel/advanced/local-management/tunnel-useful-commands/),
[CLI subcommands source](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/cmd/cloudflared/tunnel/subcommands.go)).
Thus **Delete everywhere** cannot be implemented solely as `cloudflared tunnel
delete`; laplus must retain or reacquire account/DNS authorization and delete
the exact recorded DNS resource separately.

For resumable setup, retain access to the account certificate until all
account-level mutations and verification complete, and journal the exact
tunnel UUID, hostname, DNS record, credential path, and whether the certificate
pre-existed. Do not discard a laplus-created certificate while an incomplete
setup may still need create/route/cleanup. Once setup is verified (or explicit
cleanup succeeds), the safer default is to remove only a certificate proven to
have been created by laplus and keep the narrow tunnel JSON. This avoids
retaining a decade-long account-wide secret; the UX cost is another browser
login for later listing, DNS changes, or **Delete everywhere**. Offer an
explicit **Keep Cloudflare sign-in on this computer** choice for users who value
repeat management, clearly describing its account-wide authority. Never revoke
the API token automatically, because that could invalidate other copies of the
same account certificate on other machines.

## Account certificate lifecycle follow-up (2026-08-02)

The earlier idea of an ephemeral, laplus-owned account certificate is not a
cleanly supported `cloudflared` workflow. The account-wide `cert.pem` is valid
for at least ten years and permits management of every tunnel in the account,
whereas the non-expiring tunnel UUID JSON is narrow: it can only run its one
tunnel. A connector needs only that JSON, so it continues to run without
`cert.pem`
([Tunnel permissions](https://developers.cloudflare.com/tunnel/advanced/local-management/tunnel-permissions/)).

However, `cloudflared tunnel login` writes only to the platform's default
cloudflared directory (normally `~/.cloudflared/cert.pem`). Its
`checkForExistingCert` path refuses to overwrite an existing certificate, and
`--origincert` selects a certificate for later commands rather than redirecting
login output. Making the certificate ephemeral would therefore require laplus
to manipulate the user's environment or cloudflared-owned files
([login implementation](https://github.com/cloudflare/cloudflared/blob/3a2b45c2a511fcdd81b68c190938e4ffadbea5dc/cmd/cloudflared/tunnel/login.go),
[origin-certificate parameter](https://developers.cloudflare.com/tunnel/advanced/run-parameters/)).

Recommendation: treat `cert.pem` as a cloudflared-owned prerequisite. Use it
in place with an explicit warning about its account-wide scope; never copy,
move, replace, or delete it. Laplus should retain only the narrow tunnel JSON
for steady-state operation. If account management is needed later, ask the
user to authenticate with cloudflared again; because login refuses to
overwrite the default certificate, the user may first have to move or delete
an existing stale certificate, making re-login somewhat awkward. Laplus should
explain that step but not perform it automatically.
