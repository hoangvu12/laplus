//! Operator-owned public endpoint registration and layered verification.

use std::net::IpAddr;
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub hostname: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub configured: bool,
    pub https_origin: Option<String>,
    pub wss_origin: Option<String>,
    pub ownership: &'static str,
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
    if url.scheme() != "https" || url.host_str().is_none() || url.port().is_some()
        || url.username() != "" || url.password().is_some() || url.path() != "/"
        || url.query().is_some() || url.fragment().is_some()
    {
        return Err("Enter a hostname only; HTTPS and the default port are required.");
    }
    let host = url.host_str().ok_or("Enter a valid HTTPS hostname.")?.trim_end_matches('.');
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
                || ip.to_ipv4_mapped().is_some_and(|mapped| {
                    !public_address(IpAddr::V4(mapped))
                }))
        }
    }
}

#[derive(Debug)]
pub struct VerificationFailure {
    pub kind: &'static str,
    pub message: &'static str,
}

pub async fn verify(origin: &str, environment_id: &str, http_token: &str, ws_token: &str)
    -> Result<(), VerificationFailure>
{
    let url = reqwest::Url::parse(origin).map_err(|_| VerificationFailure { kind: "dns", message: "The configured hostname is invalid." })?;
    let host = url.host_str().ok_or(VerificationFailure { kind: "dns", message: "The configured hostname has no DNS name." })?;
    let resolved = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::net::lookup_host((host, 443)),
    )
        .await
        .map_err(|_| VerificationFailure { kind: "dns", message: "DNS lookup timed out." })?
        .map_err(|_| VerificationFailure { kind: "dns", message: "DNS lookup failed." })?;
    let resolved: Vec<_> = resolved.collect();
    if resolved.is_empty() || !resolved.iter().all(|address| public_address(address.ip())) {
        return Err(VerificationFailure { kind: "destination", message: "The hostname resolves to a disallowed address." });
    }
    // Pin every outbound protocol to the address that passed policy. Resolving
    // again in either client would leave a DNS-rebinding gap between the check
    // above and the authenticated request.
    let destination = resolved[0];
    let client = reqwest::Client::builder().redirect(Policy::none()).timeout(Duration::from_secs(10))
        .resolve(host, destination)
        .build().map_err(|_| VerificationFailure { kind: "tls", message: "Could not prepare HTTPS verification." })?;
    let descriptor = client.get(format!("{origin}/.well-known/t3/environment")).send().await
        .map_err(|error| VerificationFailure { kind: if error.is_connect() { "tls" } else { "http" }, message: "The public HTTPS endpoint could not be reached." })?;
    if descriptor.status().is_redirection() {
        return Err(VerificationFailure { kind: "cloudflare-access", message: "Cloudflare Access intercepted the environment descriptor." });
    }
    let content_type = descriptor.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    if content_type.contains("text/html") {
        return Err(VerificationFailure { kind: "cloudflare-access", message: "An HTML access page intercepted the environment descriptor." });
    }
    let body: serde_json::Value = descriptor.json().await.map_err(|_| VerificationFailure { kind: "identity", message: "The endpoint did not return a laplus environment descriptor." })?;
    if body.get("environmentId").and_then(|v| v.as_str()) != Some(environment_id) {
        return Err(VerificationFailure { kind: "wrong-environment", message: "The hostname reaches a different laplus environment." });
    }
    let challenge = client.get(format!("{origin}/api/access/cloudflare/challenge"))
        .bearer_auth(http_token).send().await.map_err(|_| VerificationFailure { kind: "authentication", message: "The authenticated HTTP challenge failed." })?;
    if challenge.status().is_redirection()
        || challenge.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(|value| value.contains("text/html"))
    {
        return Err(VerificationFailure { kind: "cloudflare-access", message: "An access page intercepted the authenticated HTTP challenge." });
    }
    if !challenge.status().is_success() {
        return Err(VerificationFailure { kind: "authentication", message: "The authenticated HTTP challenge was refused." });
    }
    let ws_url = format!("wss://{host}/api/access/cloudflare/challenge/ws");
    let mut request = ws_url.into_client_request()
        .map_err(|_| VerificationFailure { kind: "websocket", message: "The WebSocket challenge could not be prepared." })?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {ws_token}").parse()
            .map_err(|_| VerificationFailure { kind: "websocket", message: "The WebSocket challenge could not be prepared." })?,
    );
    let stream = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect(destination)).await
        .map_err(|_| VerificationFailure { kind: "websocket", message: "The authenticated WebSocket upgrade timed out." })?
        .map_err(|_| VerificationFailure { kind: "websocket", message: "The authenticated WebSocket connection failed." })?;
    let (mut socket, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::client_async_tls_with_config(request, stream, None, None),
    ).await
        .map_err(|_| VerificationFailure { kind: "websocket", message: "The authenticated WebSocket upgrade timed out." })?
        .map_err(|error| {
            if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
                let intercepted = response.status().is_redirection()
                    || response.headers().get("content-type").and_then(|value| value.to_str().ok()).is_some_and(|value| value.contains("text/html"));
                if intercepted {
                    return VerificationFailure { kind: "cloudflare-access-websocket", message: "An access page intercepted the WebSocket upgrade." };
                }
            }
            VerificationFailure { kind: "websocket", message: "The authenticated WebSocket upgrade failed." }
        })?;
    let answer = tokio::time::timeout(Duration::from_secs(10), socket.next()).await
        .map_err(|_| VerificationFailure { kind: "websocket", message: "The authenticated WebSocket challenge timed out." })?
        .ok_or(VerificationFailure { kind: "websocket", message: "The authenticated WebSocket closed before answering." })?
        .map_err(|_| VerificationFailure { kind: "websocket", message: "The authenticated WebSocket challenge failed." })?;
    if answer.into_text().ok().as_deref() != Some("ok") {
        return Err(VerificationFailure { kind: "websocket", message: "The authenticated WebSocket challenge returned an unexpected answer." });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_normalizes_only_https_hostnames() {
        assert_eq!(normalize_hostname(" Example.COM. "), Ok("https://example.com".into()));
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
}
