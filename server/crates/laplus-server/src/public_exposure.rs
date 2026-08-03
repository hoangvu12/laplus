//! Operator-owned public endpoint registration and layered verification.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const DESCRIPTOR_BODY_LIMIT: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub hostname: String,
}

/// A string that arrived where one of a closed set of words was expected.
///
/// Returned rather than defaulted, because every one of these vocabularies
/// decides what laplus is allowed to *delete*, and a parse that silently picked
/// a value would pick it for a row nothing wrote. See [`TunnelOwnership`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownWord {
    pub vocabulary: &'static str,
    pub found: String,
}

impl std::fmt::Display for UnknownWord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} is not a {}", self.found, self.vocabulary)
    }
}

/// A closed vocabulary that crosses a module boundary, as a Rust enum.
///
/// **The point of the macro is the `match` it generates.** Every one of these
/// used to be a `String` compared against a literal in another file — the
/// server asked `selection.classification == "external"`, the connector wrote
/// `ownership: "adopted"` and nothing ever read it back. Adding a state to a
/// set of literals is silent; adding a variant here is a compile error at every
/// site that has to answer for it, which is the whole reason tickets 05, 06 and
/// 07 can add `adopting`, `creating`, `cleanup-required` and `partially-deleted`
/// without auditing the tree by hand.
///
/// The wire spelling is pinned beside the variant rather than derived, because
/// the contract in `packages/contracts/src/remoteAccess.ts` already fixed these
/// words and a rename here must not be able to change them by accident.
macro_rules! closed_vocabulary {
    (
        $(#[$meta:meta])*
        $name:ident as $vocabulary:literal { $($(#[$variant_meta:meta])* $variant:ident => $wire:literal),+ $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name {
            $($(#[$variant_meta])* $variant),+
        }

        impl $name {
            /// The word the contract pins for this variant.
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }

            /// Every variant, so a caller that must cover the vocabulary can
            /// iterate it instead of repeating it.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
        }

        impl std::str::FromStr for $name {
            type Err = $crate::public_exposure::UnknownWord;

            fn from_str(word: &str) -> Result<Self, $crate::public_exposure::UnknownWord> {
                match word {
                    $($wire => Ok(Self::$variant),)+
                    other => Err($crate::public_exposure::UnknownWord {
                        vocabulary: $vocabulary,
                        found: other.to_string(),
                    }),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let word = <String as serde::Deserialize>::deserialize(deserializer)?;
                word.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

pub(crate) use closed_vocabulary;

closed_vocabulary! {
    /// Who owns the Cloudflare tunnel behind an endpoint.
    ///
    /// **Not the same question as who runs the connector.** A connector token
    /// tunnel is configured at Cloudflare and run by laplus; an adopted tunnel
    /// is allocated at Cloudflare and configured *and* run by laplus; only a
    /// laplus-created tunnel is laplus's to delete. `CONTEXT.md`'s "Remote
    /// access" section is the vocabulary and `docs/adr/0049` is the decision to
    /// persist it rather than emit it as a literal.
    ///
    /// This is the value ticket 07's whole acceptance matrix is indexed by, and
    /// the reason "Delete everywhere is never offered for an adopted tunnel" can
    /// be a server-side refusal rather than a hidden button.
    TunnelOwnership as "tunnel ownership" {
        /// Somebody else's tunnel. laplus verifies and advertises a hostname and
        /// touches nothing — including when laplus runs the connector from a
        /// tunnel-specific token, because Cloudflare still owns the tunnel's
        /// configuration and allocation.
        External => "external",
        /// An inactive existing tunnel explicitly dedicated to this environment.
        /// laplus may configure and supervise it; the Cloudflare allocation and
        /// DNS route remain someone else's. Ticket 05.
        Adopted => "adopted",
        /// laplus created the allocation and the DNS route, and is the only
        /// owner that may delete either. Ticket 06.
        LaplusCreated => "laplus-created",
    }
}

impl TunnelOwnership {
    /// Whether laplus may delete this tunnel's Cloudflare resources.
    ///
    /// One method rather than a comparison at each call site, because ticket 07
    /// forbids adopted and external tunnels from *ever* reaching a deletion
    /// command "including through repeated, stale, or forged client requests" —
    /// which means the check belongs somewhere a route cannot forget to make.
    pub const fn deletable_at_cloudflare(self) -> bool {
        matches!(self, Self::LaplusCreated)
    }
}

closed_vocabulary! {
    /// Which multi-step Cloudflare mutation a journal entry belongs to.
    MutationIntent as "mutation intent" {
        /// Dedicate an inactive existing tunnel to this environment. Ticket 05.
        Adopt => "adopt",
        /// Create a stable tunnel and its DNS route. Ticket 06.
        Create => "create",
        /// Delete the exact Cloudflare resources laplus created. Ticket 07.
        DeleteEverywhere => "delete-everywhere",
        /// Remove laplus-owned local configuration and secrets. Ticket 07.
        Forget => "forget",
    }
}

closed_vocabulary! {
    /// One journaled step of a Cloudflare mutation.
    ///
    /// **Journaled before and after, which is what makes a step resumable.** A
    /// step recorded `Pending` and never settled is exactly the "remaining
    /// work" tickets 06 and 07 require a partial failure to preserve; a step
    /// recorded `Completed` is the mutation a retry must not repeat.
    MutationStep as "mutation step" {
        /// Retrieve or create the narrow run credential for the tunnel.
        Credential => "credential",
        /// `cloudflared tunnel create`.
        TunnelCreate => "tunnel-create",
        /// `cloudflared tunnel route dns`.
        DnsRoute => "dns-route",
        /// Write laplus's own isolated ingress configuration.
        Configuration => "configuration",
        /// Delete the exact recorded DNS record. **Not a cloudflared command**:
        /// the CLI has no `route dns delete`, so this step is a Cloudflare DNS
        /// API call and needs DNS authority of its own. See `research.md`.
        DnsRecordDelete => "dns-record-delete",
        /// `cloudflared tunnel delete`.
        TunnelDelete => "tunnel-delete",
        /// Remove laplus's own ingress configuration.
        ConfigurationRemove => "configuration-remove",
        /// Remove the narrow run credential laplus stored.
        CredentialRemove => "credential-remove",
    }
}

closed_vocabulary! {
    /// How far a journaled step got.
    MutationState as "mutation state" {
        /// Started and not settled. After a restart this is the remaining work.
        Pending => "pending",
        /// Done at Cloudflare or on disk. A retry must not repeat it.
        Completed => "completed",
        /// Attempted and refused. Distinct from `Pending` because a retry may
        /// safely start it again, and because a wizard that reported it as
        /// merely unfinished would be claiming a rollback that did not occur.
        Failed => "failed",
    }
}

closed_vocabulary! {
    /// Why a public-exposure command was refused.
    ///
    /// **Named where the refusal is raised, never recovered from its prose.**
    /// The first version of this crossed the wire as a `&'static str` chosen by
    /// prefix-matching the message in the route — which is the same mistake
    /// [`TunnelOwnership`] exists to undo, and it had already produced one: a
    /// connector whose settings file could not be written was reported as
    /// `restarts-exhausted`, because the only thing the route could see was a
    /// string. A reason is a decision the code that failed already made.
    RefusalReason as "refusal reason" {
        /// Sign in to Cloudflare first.
        SignInRequired => "sign-in-required",
        /// The account certificate may not be used until its authority is
        /// accepted.
        ConsentRequired => "consent-required",
        /// The chosen tunnel is no longer in the listing.
        SelectionStale => "selection-stale",
        /// There is no connector to act on yet, or not enough to make one.
        ConnectorRequired => "connector-required",
        /// Nothing is running that could be cancelled.
        NothingRunning => "nothing-running",
        /// laplus already owns this exposure, or another owner already does.
        OwnershipConflict => "ownership-conflict",
        /// Automatic restarts are spent; an explicit retry is required.
        RestartsExhausted => "restarts-exhausted",
        /// The named cloudflared cannot be started, or is too old.
        ExecutableUnusable => "executable-unusable",
        /// The hostname is not a bare public HTTPS host.
        HostnameInvalid => "hostname-invalid",
        /// The approved release is no longer the one the feed offers.
        ReleaseMoved => "release-moved",
        /// cloudflared ran and said no.
        CommandFailed => "command-failed",
        /// laplus could not write its own private configuration or credential.
        /// Distinct from `CommandFailed` because nothing at Cloudflare went
        /// wrong and a retry is local.
        LocalSetupFailed => "local-setup-failed",
        /// The tunnel became active between listing and mutation, so it is
        /// externally managed after all. Ticket 05's activation race.
        TunnelBecameActive => "tunnel-became-active",
        /// Only a laplus-created tunnel may be deleted at Cloudflare. Ticket 07.
        NotLaplusCreated => "not-laplus-created",
        /// A previous mutation left state half-changed. Ticket 07.
        CleanupRequired => "cleanup-required",
    }
}

/// Which half of the refusal contract this is, and therefore its status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalKind {
    /// The developer has to do something first. `409`.
    Precondition,
    /// cloudflared, its output, or the request itself said no. `400`.
    Rejected,
}

/// A refused public-exposure command, in the shape a client can decode.
///
/// **One refusal type for the whole surface.** The account module, the connector
/// manager and the routes each used to refuse in their own currency — two
/// bespoke enums and a bare `String` — so the route was left inferring a
/// contract word from a sentence somebody else wrote. Every site that can refuse
/// now says which reason it is refusing for, because it is the only place that
/// knows.
///
/// `IntoResponse` lives in `server.rs`, which is where the status and the tag
/// belong.
#[derive(Debug)]
pub struct Refusal {
    pub kind: RefusalKind,
    pub reason: RefusalReason,
    pub message: String,
    pub completed: Vec<MutationStep>,
    pub remaining: Vec<MutationStep>,
}

impl Refusal {
    /// The developer has to do something first. `409`.
    pub fn precondition(reason: RefusalReason, message: impl Into<String>) -> Self {
        Self::new(RefusalKind::Precondition, reason, message)
    }

    /// cloudflared, its output, or the request itself said no. `400`.
    pub fn rejected(reason: RefusalReason, message: impl Into<String>) -> Self {
        Self::new(RefusalKind::Rejected, reason, message)
    }

    fn new(kind: RefusalKind, reason: RefusalReason, message: impl Into<String>) -> Self {
        Self { kind, reason, message: message.into(), completed: Vec::new(), remaining: Vec::new() }
    }

    /// Attach the exact work a partial mutation finished and left outstanding.
    ///
    /// Tickets 06 and 07 both require a failure to name both halves, so that a
    /// retry repeats nothing and the wizard never claims a rollback it could
    /// not perform. Nothing journals yet — the two lists are empty on every
    /// refusal this build raises — but the shape is the one those tickets fill,
    /// so they add a call rather than a contract.
    #[allow(dead_code)]
    pub fn after(mut self, completed: &[MutationStep], remaining: &[MutationStep]) -> Self {
        self.completed = completed.to_vec();
        self.remaining = remaining.to_vec();
        self
    }

    /// Redact a secret from the sentence, wherever it came from.
    ///
    /// Applied at the boundary rather than at each raise site: a connector token
    /// reaches several of them by way of `cloudflared`'s own output, and a
    /// redaction that has to be remembered is one that will be forgotten.
    pub fn redacting(mut self, secret: &str) -> Self {
        if !secret.trim().is_empty() {
            self.message = self.message.replace(secret, "[REDACTED]");
        }
        self
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub configured: bool,
    pub https_origin: Option<String>,
    pub wss_origin: Option<String>,
    pub ownership: TunnelOwnership,
    pub health: serde_json::Value,
    pub verification_state: String,
    pub failure_kind: Option<String>,
    pub failure_message: Option<String>,
    pub last_attempt_at: Option<String>,
    pub last_verified_at: Option<String>,
    pub advertised_endpoint: Option<serde_json::Value>,
}

pub fn normalize_hostname(input: &str) -> Result<String, &'static str> {
    let candidate = input.trim();
    let url = if candidate.contains("://") {
        reqwest::Url::parse(candidate).map_err(|_| "Enter a valid HTTPS hostname.")?
    } else {
        reqwest::Url::parse(&format!("https://{candidate}"))
            .map_err(|_| "Enter a valid HTTPS hostname.")?
    };
    if url.scheme() != "https"
        || url.host_str().is_none()
        || url.port().is_some()
        || url.username() != ""
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Enter a hostname only; HTTPS and the default port are required.");
    }
    let host = url
        .host_str()
        .ok_or("Enter a valid HTTPS hostname.")?
        .trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") || host.parse::<IpAddr>().is_ok() {
        return Err("A public DNS hostname is required.");
    }
    Ok(format!("https://{}", host.to_ascii_lowercase()))
}

pub fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip.is_unspecified()
                || first == 0
                || (first == 100 && (64..=127).contains(&second))
                || (first == 192 && second == 0)
                || (first == 198 && (second == 18 || second == 19))
                || first >= 224)
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_multicast()
                || (segments[0] == 0x2001 && segments[1] == 0x0db8)
                || (segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0])
                || (segments[..3] == [0x0064, 0xff9b, 1])
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|mapped| !public_address(IpAddr::V4(mapped))))
        }
    }
}

#[derive(Debug)]
pub struct VerificationFailure {
    pub kind: &'static str,
    pub message: &'static str,
}

fn verify_descriptor(
    status: reqwest::StatusCode,
    content_type: &str,
    body: &[u8],
    environment_id: &str,
) -> Result<(), VerificationFailure> {
    if status.is_redirection() || content_type.contains("text/html") {
        return Err(VerificationFailure {
            kind: "cloudflare-access",
            message: "Cloudflare Access intercepted the environment descriptor.",
        });
    }
    let body: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| VerificationFailure {
            kind: "identity",
            message: "The endpoint did not return a laplus environment descriptor.",
        })?;
    if body.get("environmentId").and_then(|value| value.as_str()) != Some(environment_id) {
        return Err(VerificationFailure {
            kind: "wrong-environment",
            message: "The hostname reaches a different laplus environment.",
        });
    }
    Ok(())
}

async fn read_descriptor_body(response: reqwest::Response) -> Result<Vec<u8>, VerificationFailure> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| VerificationFailure {
            kind: "identity",
            message: "The endpoint descriptor could not be read.",
        })?;
        if body.len().saturating_add(chunk.len()) > DESCRIPTOR_BODY_LIMIT {
            return Err(VerificationFailure {
                kind: "identity",
                message: "The endpoint descriptor was too large.",
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub type VerificationFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), VerificationFailure>> + Send + 'a>>;

pub trait EndpointVerifier: std::fmt::Debug + Send + Sync {
    fn verify<'a>(
        &'a self,
        origin: &'a str,
        environment_id: &'a str,
        http_token: &'a str,
        ws_token: &'a str,
    ) -> VerificationFuture<'a>;
}

#[derive(Debug, Default)]
pub struct NetworkEndpointVerifier {
    resolved_addresses: Option<Vec<std::net::SocketAddr>>,
    trusted_root_der: Option<Vec<u8>>,
    permit_non_public_test_addresses: bool,
}

impl NetworkEndpointVerifier {
    /// A hermetic network boundary for integration tests. Production always
    /// uses [`Default`], system DNS and platform roots; this keeps the real
    /// HTTPS/WSS implementation under test without making a local peer public.
    #[doc(hidden)]
    pub fn with_hermetic_network(
        resolved_addresses: Vec<std::net::SocketAddr>,
        trusted_root_der: Vec<u8>,
    ) -> Self {
        Self {
            resolved_addresses: Some(resolved_addresses),
            trusted_root_der: Some(trusted_root_der),
            permit_non_public_test_addresses: true,
        }
    }
}

pub fn next_background_delay(current: Duration, succeeded: bool) -> Duration {
    if succeeded {
        Duration::from_secs(300)
    } else {
        std::cmp::min(current.saturating_mul(2), Duration::from_secs(1800))
    }
}

impl EndpointVerifier for NetworkEndpointVerifier {
    fn verify<'a>(
        &'a self,
        origin: &'a str,
        environment_id: &'a str,
        http_token: &'a str,
        ws_token: &'a str,
    ) -> VerificationFuture<'a> {
        Box::pin(verify_with_network(
            origin,
            environment_id,
            http_token,
            ws_token,
            self,
        ))
    }
}

pub async fn verify(
    origin: &str,
    environment_id: &str,
    http_token: &str,
    ws_token: &str,
) -> Result<(), VerificationFailure> {
    verify_with_network(
        origin,
        environment_id,
        http_token,
        ws_token,
        &NetworkEndpointVerifier::default(),
    )
    .await
}

async fn verify_with_network(
    origin: &str,
    environment_id: &str,
    http_token: &str,
    ws_token: &str,
    network: &NetworkEndpointVerifier,
) -> Result<(), VerificationFailure> {
    let url = reqwest::Url::parse(origin).map_err(|_| VerificationFailure {
        kind: "dns",
        message: "The configured hostname is invalid.",
    })?;
    let host = url.host_str().ok_or(VerificationFailure {
        kind: "dns",
        message: "The configured hostname has no DNS name.",
    })?;
    let port = url.port_or_known_default().unwrap_or(443);
    let resolved = match &network.resolved_addresses {
        Some(addresses) => addresses.clone(),
        None => tokio::time::timeout(
            Duration::from_secs(10),
            tokio::net::lookup_host((host, port)),
        )
        .await
        .map_err(|_| VerificationFailure {
            kind: "dns",
            message: "DNS lookup timed out.",
        })?
        .map_err(|_| VerificationFailure {
            kind: "dns",
            message: "DNS lookup failed.",
        })?
        .collect(),
    };
    if resolved.is_empty()
        || (!network.permit_non_public_test_addresses
            && !resolved.iter().all(|address| public_address(address.ip())))
    {
        return Err(VerificationFailure {
            kind: "destination",
            message: "The hostname resolves to a disallowed address.",
        });
    }
    // Pin every outbound protocol to the address that passed policy. Resolving
    // again in either client would leave a DNS-rebinding gap between the check
    // above and the authenticated request.
    let destination = resolved[0];
    let mut client_builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(10))
        .resolve(host, destination);
    let ws_connector = if let Some(root_der) = &network.trusted_root_der {
        let certificate =
            reqwest::Certificate::from_der(root_der).map_err(|_| VerificationFailure {
                kind: "tls",
                message: "Could not read the trusted verification root.",
            })?;
        client_builder = client_builder.add_root_certificate(certificate);
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(root_der.clone()))
            .map_err(|_| VerificationFailure {
                kind: "tls",
                message: "Could not read the trusted verification root.",
            })?;
        let config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Some(tokio_tungstenite::Connector::Rustls(std::sync::Arc::new(
            config,
        )))
    } else {
        None
    };
    let client = client_builder.build().map_err(|_| VerificationFailure {
        kind: "tls",
        message: "Could not prepare HTTPS verification.",
    })?;
    let descriptor = client
        .get(format!("{origin}/.well-known/t3/environment"))
        .send()
        .await
        .map_err(|error| VerificationFailure {
            kind: if error.is_connect() { "tls" } else { "http" },
            message: "The public HTTPS endpoint could not be reached.",
        })?;
    let status = descriptor.status();
    let content_type = descriptor
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let body = read_descriptor_body(descriptor).await?;
    verify_descriptor(status, &content_type, &body, environment_id)?;
    let challenge = client
        .get(format!("{origin}/api/access/cloudflare/challenge"))
        .bearer_auth(http_token)
        .send()
        .await
        .map_err(|_| VerificationFailure {
            kind: "authentication",
            message: "The authenticated HTTP challenge failed.",
        })?;
    if challenge.status().is_redirection()
        || challenge
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("text/html"))
    {
        return Err(VerificationFailure {
            kind: "cloudflare-access",
            message: "An access page intercepted the authenticated HTTP challenge.",
        });
    }
    if !challenge.status().is_success() {
        return Err(VerificationFailure {
            kind: "authentication",
            message: "The authenticated HTTP challenge was refused.",
        });
    }
    let mut ws_url = url.clone();
    ws_url.set_scheme("wss").map_err(|_| VerificationFailure {
        kind: "websocket",
        message: "The WebSocket challenge could not be prepared.",
    })?;
    ws_url.set_path("/api/access/cloudflare/challenge/ws");
    ws_url.set_query(None);
    ws_url.set_fragment(None);
    let mut request = ws_url
        .as_str()
        .into_client_request()
        .map_err(|_| VerificationFailure {
            kind: "websocket",
            message: "The WebSocket challenge could not be prepared.",
        })?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {ws_token}")
            .parse()
            .map_err(|_| VerificationFailure {
                kind: "websocket",
                message: "The WebSocket challenge could not be prepared.",
            })?,
    );
    let stream = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::TcpStream::connect(destination),
    )
    .await
    .map_err(|_| VerificationFailure {
        kind: "websocket",
        message: "The authenticated WebSocket upgrade timed out.",
    })?
    .map_err(|_| VerificationFailure {
        kind: "websocket",
        message: "The authenticated WebSocket connection failed.",
    })?;
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::client_async_tls_with_config(request, stream, None, ws_connector),
    )
    .await
    .map_err(|_| VerificationFailure {
        kind: "websocket",
        message: "The authenticated WebSocket upgrade timed out.",
    })?
    .map_err(|error| {
        if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
            let intercepted = response.status().is_redirection()
                || response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.contains("text/html"));
            if intercepted {
                return VerificationFailure {
                    kind: "cloudflare-access-websocket",
                    message: "An access page intercepted the WebSocket upgrade.",
                };
            }
        }
        VerificationFailure {
            kind: "websocket",
            message: "The authenticated WebSocket upgrade failed.",
        }
    })?;
    let answer = tokio::time::timeout(Duration::from_secs(10), socket.next())
        .await
        .map_err(|_| VerificationFailure {
            kind: "websocket",
            message: "The authenticated WebSocket challenge timed out.",
        })?
        .ok_or(VerificationFailure {
            kind: "websocket",
            message: "The authenticated WebSocket closed before answering.",
        })?
        .map_err(|_| VerificationFailure {
            kind: "websocket",
            message: "The authenticated WebSocket challenge failed.",
        })?;
    if answer.into_text().ok().as_deref() != Some("ok") {
        return Err(VerificationFailure {
            kind: "websocket",
            message: "The authenticated WebSocket challenge returned an unexpected answer.",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vocabulary crosses two process boundaries — a SQLite column and the
    /// JSON contract — so the words have to survive both round trips exactly.
    #[test]
    fn tunnel_ownership_survives_the_wire_and_the_column_unchanged() {
        for ownership in TunnelOwnership::ALL {
            assert_eq!(
                ownership.as_str().parse::<TunnelOwnership>(),
                Ok(*ownership),
                "{ownership} does not read back"
            );
            assert_eq!(
                serde_json::to_string(ownership).unwrap(),
                format!("\"{}\"", ownership.as_str())
            );
        }
        assert_eq!(
            TunnelOwnership::ALL
                .iter()
                .map(|ownership| ownership.as_str())
                .collect::<Vec<_>>(),
            ["external", "adopted", "laplus-created"]
        );
    }

    /// ADR-0045 gives every lifecycle action one owner, and this is the half of
    /// it that decides whether a *destructive* Cloudflare command may run at
    /// all. Ticket 07 forbids adopted and external tunnels from reaching one
    /// through any request, so the answer lives here rather than in a route.
    #[test]
    fn only_a_laplus_created_tunnel_may_be_deleted_at_cloudflare() {
        assert!(TunnelOwnership::LaplusCreated.deletable_at_cloudflare());
        assert!(!TunnelOwnership::Adopted.deletable_at_cloudflare());
        assert!(!TunnelOwnership::External.deletable_at_cloudflare());
    }

    /// A word nothing in this build wrote is refused rather than defaulted,
    /// because every default here would be a guess about deletion authority.
    #[test]
    fn a_word_outside_the_vocabulary_is_refused_and_says_what_it_was() {
        let failure = "laplus".parse::<TunnelOwnership>().expect_err("refused");
        assert_eq!(failure.vocabulary, "tunnel ownership");
        assert_eq!(failure.found, "laplus");
        assert!("".parse::<MutationStep>().is_err());
        assert!("done".parse::<MutationState>().is_err());
    }

    /// **The cross-check the wire needs.** `RefusalReason` and
    /// `PublicExposureRefusalReason` in
    /// `packages/contracts/src/environmentHttp.ts` are the same closed set on
    /// two sides of a socket, and nothing links them but agreement. Both are
    /// pinned to this list in this order, so adding a word to one without the
    /// other fails a test rather than reaching a client that cannot decode it.
    #[test]
    fn every_refusal_reason_matches_the_contract_vocabulary() {
        assert_eq!(
            RefusalReason::ALL.iter().map(|reason| reason.as_str()).collect::<Vec<_>>(),
            [
                "sign-in-required",
                "consent-required",
                "selection-stale",
                "connector-required",
                "nothing-running",
                "ownership-conflict",
                "restarts-exhausted",
                "executable-unusable",
                "hostname-invalid",
                "release-moved",
                "command-failed",
                "local-setup-failed",
                "tunnel-became-active",
                "not-laplus-created",
                "cleanup-required",
            ]
        );
        for reason in RefusalReason::ALL {
            assert_eq!(reason.as_str().parse::<RefusalReason>(), Ok(*reason));
        }
    }

    /// A refusal that changed nothing says so, rather than leaving the client
    /// to guess whether the two lists were merely not filled in.
    #[test]
    fn a_refusal_carries_no_completed_work_unless_it_is_given_some() {
        let refused = Refusal::precondition(RefusalReason::ConsentRequired, "Confirm first.");
        assert_eq!(refused.kind, RefusalKind::Precondition);
        assert!(refused.completed.is_empty() && refused.remaining.is_empty());

        let partial = Refusal::rejected(RefusalReason::CleanupRequired, "The DNS record remains.")
            .after(&[MutationStep::TunnelDelete], &[MutationStep::DnsRecordDelete]);
        assert_eq!(partial.kind, RefusalKind::Rejected);
        assert_eq!(partial.completed, [MutationStep::TunnelDelete]);
        assert_eq!(partial.remaining, [MutationStep::DnsRecordDelete]);
    }

    /// The connector token reaches a refusal by way of cloudflared's own
    /// output, so redaction is done once at the boundary rather than at each
    /// site that could quote it.
    #[test]
    fn a_refusal_can_have_a_secret_taken_out_of_it_wherever_it_came_from() {
        let refused = Refusal::rejected(
            RefusalReason::CommandFailed,
            "cloudflared rejected the token sekret-value.",
        )
        .redacting("sekret-value");
        assert!(!refused.message.contains("sekret-value"));
        assert!(refused.message.contains("[REDACTED]"));
        // An empty secret redacts nothing rather than replacing every gap in
        // the sentence, which is what `str::replace` on "" would do.
        let untouched =
            Refusal::rejected(RefusalReason::CommandFailed, "no secret here").redacting("");
        assert_eq!(untouched.message, "no secret here");
    }

    #[test]
    fn every_journal_word_reads_back_as_itself() {
        for step in MutationStep::ALL {
            assert_eq!(step.as_str().parse::<MutationStep>(), Ok(*step));
        }
        for state in MutationState::ALL {
            assert_eq!(state.as_str().parse::<MutationState>(), Ok(*state));
        }
        for intent in MutationIntent::ALL {
            assert_eq!(intent.as_str().parse::<MutationIntent>(), Ok(*intent));
        }
        // The one step that is not a cloudflared command, and the reason
        // ticket 07 needs Cloudflare DNS authority separately from the CLI.
        assert_eq!(MutationStep::DnsRecordDelete.as_str(), "dns-record-delete");
    }

    #[test]
    fn registration_normalizes_only_https_hostnames() {
        assert_eq!(
            normalize_hostname(" Example.COM. "),
            Ok("https://example.com".into())
        );
        assert!(normalize_hostname("http://example.com").is_err());
        assert!(normalize_hostname("https://example.com/path").is_err());
        assert!(normalize_hostname("127.0.0.1").is_err());
    }

    #[test]
    fn non_public_destinations_are_disallowed() {
        assert!(!public_address("10.0.0.1".parse().unwrap()));
        assert!(!public_address("100.64.0.1".parse().unwrap()));
        assert!(!public_address("198.18.0.1".parse().unwrap()));
        assert!(!public_address("224.0.0.1".parse().unwrap()));
        assert!(!public_address("240.0.0.1".parse().unwrap()));
        assert!(!public_address("::1".parse().unwrap()));
        assert!(!public_address("2001:db8::1".parse().unwrap()));
        assert!(!public_address("ff02::1".parse().unwrap()));
        assert!(!public_address("::ffff:10.0.0.1".parse().unwrap()));
        assert!(!public_address("64:ff9b::a00:1".parse().unwrap()));
        assert!(!public_address("64:ff9b:1::a00:1".parse().unwrap()));
        assert!(public_address("1.1.1.1".parse().unwrap()));
        assert!(public_address("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn background_verification_backoff_is_bounded_and_success_resets_it() {
        let mut delay = Duration::from_secs(30);
        for expected in [60, 120, 240, 480, 960, 1800, 1800] {
            delay = next_background_delay(delay, false);
            assert_eq!(delay, Duration::from_secs(expected));
        }
        assert_eq!(next_background_delay(delay, true), Duration::from_secs(300));
    }

    #[test]
    fn redirects_html_and_wrong_environment_descriptors_stay_distinct() {
        let redirected = verify_descriptor(
            reqwest::StatusCode::FOUND,
            "text/plain",
            b"",
            "environment-a",
        )
        .unwrap_err();
        assert_eq!(redirected.kind, "cloudflare-access");

        let access_page = verify_descriptor(
            reqwest::StatusCode::OK,
            "text/html",
            b"<html>sign in</html>",
            "environment-a",
        )
        .unwrap_err();
        assert_eq!(access_page.kind, "cloudflare-access");

        let wrong = verify_descriptor(
            reqwest::StatusCode::OK,
            "application/json",
            br#"{"environmentId":"environment-b"}"#,
            "environment-a",
        )
        .unwrap_err();
        assert_eq!(wrong.kind, "wrong-environment");
    }

    #[tokio::test]
    async fn an_oversized_descriptor_is_refused_without_buffering_the_whole_response() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            let body = vec![b'x'; DESCRIPTOR_BODY_LIMIT + 1];
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            stream.write_all(&body).await.unwrap();
        });
        let response = reqwest::get(format!("http://{address}/descriptor"))
            .await
            .unwrap();

        let failure = read_descriptor_body(response).await.unwrap_err();
        assert_eq!(failure.kind, "identity");
        assert_eq!(failure.message, "The endpoint descriptor was too large.");
        peer.await.unwrap();
    }
}
