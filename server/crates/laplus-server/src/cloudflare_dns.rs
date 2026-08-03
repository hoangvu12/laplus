//! Removing a DNS record, which `cloudflared` cannot do at all.
//!
//! **This module exists because of an asymmetry in the CLI.** `cloudflared
//! tunnel route dns` creates the CNAME that points a hostname at a tunnel, and
//! there is no `route dns delete` to undo it — the reverse is a Cloudflare DNS
//! API call needing DNS authority of its own (`.scratch/cloudflare-tunnel/research.md`).
//! So "Delete everywhere" is not one command with a `--cascade` flag; it is two
//! operations against two different Cloudflare surfaces, which is exactly why
//! ticket 07 journals them as two steps.
//!
//! **The authority is supplied per request and never kept.** The account
//! certificate does contain a token that could do this, and ADR-0045 forbids
//! laplus to read its contents — the certificate is used in place, by pointing
//! `cloudflared` at it, and opening it to extract a token would be exactly the
//! copying that ADR rules out. So the developer supplies a Cloudflare API token
//! for the one destructive request that needs it. It is not written down, not
//! logged, not put in a snapshot, and not passed as a process argument: it lives
//! in this module for the length of one call and goes out in an `Authorization`
//! header. ADR-0052.
//!
//! **The record is recorded by name and has to be resolved.** ADR-0051 explains
//! why: `route dns` reports no identifiers, so the endpoint row carries a name
//! and two `Option` ids. Deleting it means finding the zone whose name the
//! record sits under, then the record within it — and writing both back onto the
//! row, so that a retry after a partial cleanup addresses the record it already
//! found rather than resolving it again.

use std::time::Duration;

use serde::Deserialize;

use crate::public_exposure::{Refusal, RefusalReason};

const DEFAULT_API: &str = "https://api.cloudflare.com";
/// A hang detector rather than a budget — the same reasoning as everywhere else
/// in this crate. A destructive request that cannot be answered is reported, not
/// waited on forever.
const TIMEOUT: Duration = Duration::from_secs(15);

/// What happened to a record laplus asked Cloudflare to remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    /// Cloudflare removed it during this request.
    Removed,
    /// It was not there. **Not a failure**: a cleanup interrupted between
    /// deleting the record and journaling that it had is exactly the state this
    /// answers, and a retry that read "no such record" as a new error could
    /// never finish. Cloudflare's own code for it is `81044`.
    AlreadyGone,
}

/// Cloudflare's DNS API, as narrowly as laplus needs it.
#[derive(Debug, Clone)]
pub struct Dns {
    origin: String,
    token: String,
}

/// One record laplus resolved, and the zone it is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located {
    pub zone_id: String,
    pub record_id: String,
}

/// The envelope every Cloudflare API response is wrapped in.
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    code: i64,
}

/// A Cloudflare object as narrowly as a lookup needs it: what it is called, and
/// what to address it by.
///
/// One shape for zones and for DNS records, because laplus asks the same
/// question of both — find the thing with this name, and tell me its id — and
/// the two answers differ only in the fields this deliberately ignores.
#[derive(Debug, Deserialize)]
struct NamedResource {
    id: String,
    name: String,
}

/// Cloudflare's code for "that DNS record is not here", which an idempotent
/// retry has to read as already-done rather than as a new failure.
const RECORD_DOES_NOT_EXIST: i64 = 81044;

impl Dns {
    /// A client for one destructive request, or the refusal saying there is no
    /// authority to make one.
    ///
    /// **A blank token is a refusal rather than an attempt.** Sending an empty
    /// `Authorization` header would spend a round trip to learn what is already
    /// known, and would report it as Cloudflare saying no — when what actually
    /// happened is that laplus was never given DNS authority. Ticket 07 requires
    /// missing authority to leave a recoverable state, and the most recoverable
    /// state is the one where nothing was attempted.
    pub fn with_token(token: &str) -> Result<Self, Refusal> {
        if token.trim().is_empty() {
            return Err(missing_authority());
        }
        Ok(Self { origin: api_origin(), token: token.trim().to_string() })
    }

    /// One request to Cloudflare, answered as the status and the envelope every
    /// reply comes wrapped in.
    ///
    /// **Written once because each rung of it is a decision.** Carrying the
    /// token, reading `401` and `403` as *laplus was never given authority*
    /// rather than as Cloudflare saying no, and reading a body that will not
    /// parse as unreadable rather than as an empty result are three answers this
    /// module has to give the same way every time — and they were given twice,
    /// in a `get` and a `delete` that had already drifted to reading the status
    /// at different points. The status comes back out because deleting needs it:
    /// `404` plus Cloudflare's own `81044` is a record already gone.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
    ) -> Result<(reqwest::StatusCode, Envelope<T>), Refusal> {
        let response = self
            .client()?
            .request(method, format!("{}{path}", self.origin))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| unreachable_api())?;
        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::UNAUTHORIZED
        {
            return Err(missing_authority());
        }
        let envelope = response.json().await.map_err(|_| unreadable_api())?;
        Ok((status, envelope))
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<Vec<T>, Refusal> {
        let (_, envelope): (_, Envelope<Vec<T>>) =
            self.call(reqwest::Method::GET, path).await?;
        if !envelope.success {
            // **Not every refusal is a missing token.** Cloudflare answers a
            // rate limit, a suspended zone and a malformed query with the same
            // `success: false`, and telling a developer to go and make an API
            // token they already have is the wrong next action. Only the two
            // authorization statuses above mean what `dns-authority-required`
            // says.
            return Err(unreadable_api());
        }
        Ok(envelope.result.unwrap_or_default())
    }

    /// Which zone and record the recorded name is, if this token can see them.
    ///
    /// **The zone is asked for by name rather than searched for in a list.** A
    /// record name is `laplus.example.com` and its zone might be `example.com`
    /// or `laplus.example.com`, and nothing laplus recorded says which — the
    /// account certificate knows, and its contents are never read (ADR-0045). So
    /// each suffix of the name is looked up directly, longest first, and the
    /// first that exists is the zone.
    ///
    /// Listing instead would have to page: Cloudflare caps `/zones` at fifty per
    /// page, so an account with more zones than that would answer "no zone
    /// contains this name" for a hostname it does own — and a legitimate
    /// deletion would be refused for want of authority it actually had. A
    /// handful of exact lookups is bounded by the number of labels in the name
    /// and cannot go wrong that way.
    ///
    /// A name whose every suffix is a zone this token cannot see is a name it
    /// has no authority over, which is the same answer as no token at all.
    pub async fn locate(&self, record_name: &str) -> Result<Option<Located>, Refusal> {
        let mut zone = None;
        for candidate in zone_candidates(record_name) {
            let found: Vec<NamedResource> = self
                .get(&format!("/client/v4/zones?name={candidate}"))
                .await?;
            if let Some(named) = found.into_iter().find(|held| held.name == candidate) {
                zone = Some(named);
                break;
            }
        }
        let zone = zone.ok_or_else(missing_authority)?;
        let records: Vec<NamedResource> = self
            .get(&format!(
                "/client/v4/zones/{}/dns_records?name={record_name}",
                zone.id
            ))
            .await?;
        // Matched by name as well as asked for by name: the query is a filter
        // and a Cloudflare that answered with more than was asked for must not
        // have laplus delete the extra. Cleanup targets one record.
        Ok(records
            .into_iter()
            .find(|record| record.name == record_name)
            .map(|record| Located { zone_id: zone.id, record_id: record.id }))
    }

    /// Remove exactly one record, by zone and id.
    ///
    /// Never by name: the name was resolved to an address once, that address is
    /// written back onto the endpoint row, and this deletes what was addressed.
    /// A deletion that re-derived its target from a name each time is one that
    /// can be pointed somewhere else by a zone that changed underneath it.
    pub async fn delete(&self, located: &Located) -> Result<Removal, Refusal> {
        let (status, envelope): (_, Envelope<serde_json::Value>) = self
            .call(
                reqwest::Method::DELETE,
                &format!(
                    "/client/v4/zones/{}/dns_records/{}",
                    located.zone_id, located.record_id
                ),
            )
            .await?;
        if envelope.success {
            return Ok(Removal::Removed);
        }
        if already_gone(status, &envelope.errors) {
            return Ok(Removal::AlreadyGone);
        }
        Err(Refusal::rejected(
            RefusalReason::CommandFailed,
            "Cloudflare refused to delete the DNS record laplus created.",
        ))
    }

    fn client(&self) -> Result<reqwest::Client, Refusal> {
        reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|_| unreachable_api())
    }
}

/// Whether Cloudflare said the record is not there.
///
/// Read from the error code rather than from the status alone, because a `404`
/// is also what a wrong path answers — and a cleanup that treated any `404` as
/// done would report a record removed that it never found.
fn already_gone(status: reqwest::StatusCode, errors: &[ApiError]) -> bool {
    status == reqwest::StatusCode::NOT_FOUND
        && errors.iter().any(|error| error.code == RECORD_DOES_NOT_EXIST)
}

/// Every zone a record name could sit in, longest first.
///
/// `a.b.example.com` is in `a.b.example.com`, `b.example.com` or `example.com`,
/// and the longest one that exists is the right one — a record in a zone
/// delegated at `b.example.com` must not be looked for in `example.com`. The
/// bare public suffix is included because nothing here knows what a public
/// suffix is, and a lookup for a zone the token cannot see costs one refused
/// read rather than a wrong answer.
fn zone_candidates(record_name: &str) -> Vec<String> {
    let labels: Vec<&str> = record_name.split('.').collect();
    (0..labels.len().saturating_sub(1))
        .map(|from| labels[from..].join("."))
        .collect()
}

fn missing_authority() -> Refusal {
    Refusal::precondition(
        RefusalReason::DnsAuthorityRequired,
        "laplus needs a Cloudflare API token with DNS edit permission for this hostname's zone \
         before it can remove the DNS record it created. cloudflared cannot delete a DNS record, \
         and the account certificate is used in place and never read.",
    )
}

fn unreachable_api() -> Refusal {
    Refusal::rejected(
        RefusalReason::CommandFailed,
        "Cloudflare's DNS API could not be reached.",
    )
}

fn unreadable_api() -> Refusal {
    Refusal::rejected(
        RefusalReason::CommandFailed,
        "Cloudflare's DNS API answered in a shape laplus cannot read.",
    )
}

/// Where Cloudflare's API is, and the one direction that may be overridden.
///
/// **Towards loopback only**, exactly as [`crate::cloudflare_install`]'s release
/// feed is: the override exists so a test can stand a fake API on `127.0.0.1`,
/// and a build that let it point anywhere would let an environment variable
/// redirect a request carrying a Cloudflare API token to somebody else's host.
fn api_origin() -> String {
    if let Ok(origin) = std::env::var("LAPLUS_CLOUDFLARE_API") {
        if loopback_origin(&origin) {
            return origin.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_API.to_string()
}

/// Whether an origin names this machine.
///
/// The brackets come off first: `host_str` keeps an IPv6 literal's `[…]`, and
/// `[::1]` does not parse as an address — so a check that skipped that would
/// quietly decline to override towards loopback on an IPv6-only machine and send
/// a request carrying a Cloudflare API token to the real API instead.
fn loopback_origin(origin: &str) -> bool {
    reqwest::Url::parse(origin).is_ok_and(|url| {
        url.host_str().is_some_and(|host| {
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            bare == "localhost"
                || bare
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No token is not "Cloudflare said no": nothing was attempted, and the
    /// developer's next action is to supply one rather than to read output.
    #[test]
    fn a_missing_token_is_a_refusal_before_any_request() {
        for blank in ["", "   ", "\n"] {
            let refused = Dns::with_token(blank).expect_err("no authority");
            assert_eq!(refused.reason, RefusalReason::DnsAuthorityRequired);
            assert_eq!(refused.kind, crate::public_exposure::RefusalKind::Precondition);
        }
        assert!(Dns::with_token("cf-token").is_ok());
    }

    /// A retry after a partial cleanup has to read Cloudflare's own "not here"
    /// as already-done, and must not read every `404` that way.
    #[test]
    fn only_cloudflares_own_missing_record_code_counts_as_already_gone() {
        assert!(already_gone(
            reqwest::StatusCode::NOT_FOUND,
            &[ApiError { code: RECORD_DOES_NOT_EXIST }]
        ));
        assert!(!already_gone(
            reqwest::StatusCode::NOT_FOUND,
            &[ApiError { code: 7003 }]
        ));
        assert!(!already_gone(reqwest::StatusCode::NOT_FOUND, &[]));
        assert!(!already_gone(
            reqwest::StatusCode::OK,
            &[ApiError { code: RECORD_DOES_NOT_EXIST }]
        ));
    }

    /// A record's zone is one of its own suffixes, and the longest that exists
    /// wins — a record under a zone delegated at `b.example.com` must not be
    /// looked for in `example.com`.
    ///
    /// Asked for by name rather than found in a listing, because Cloudflare caps
    /// `/zones` at fifty per page: an account with more zones than that would
    /// answer "no zone contains this name" for a hostname it owns, and refuse a
    /// legitimate deletion for want of authority it actually had.
    #[test]
    fn a_records_zone_is_looked_for_among_its_own_suffixes_longest_first() {
        assert_eq!(
            zone_candidates("laplus.stable.example.com"),
            ["laplus.stable.example.com", "stable.example.com", "example.com"]
        );
        assert_eq!(zone_candidates("stable.example.com"), ["stable.example.com", "example.com"]);
        // A bare two-label name is its own zone and nothing else.
        assert_eq!(zone_candidates("example.com"), ["example.com"]);
        // Nothing a hostname validator would ever pass, and no panic either.
        assert!(zone_candidates("example").is_empty());
        assert!(zone_candidates("").is_empty());
    }

    /// The override carries a Cloudflare API token in a header, so it may only
    /// ever move the destination to this machine.
    #[test]
    fn the_api_origin_is_only_overridden_towards_loopback() {
        assert!(loopback_origin("http://127.0.0.1:8080"));
        assert!(loopback_origin("http://localhost:9000"));
        assert!(loopback_origin("http://[::1]:9000"));
        assert!(!loopback_origin("https://api.cloudflare.example"));
        assert!(!loopback_origin("https://203.0.113.9"));
        assert!(!loopback_origin("not a url"));
    }
}
