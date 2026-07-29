//! URLs for files the browser fetches for itself.
//!
//! Everything else this server answers goes over the socket, and an image
//! cannot: `<img src=…>` is the browser's own request, made without the
//! socket's credential and without any way to carry one. `assets.createUrl` is
//! how the client gets a URL it may then hand to the browser, and this module
//! is both halves of that — the method that issues one and the route that
//! honours it.
//!
//! ## The URL is the credential
//!
//! There is no session behind `/api/assets/…`, and there cannot usefully be
//! one. Instead the URL carries its own authority: the claims that say which
//! file it is for are signed with a key only this server holds, so a URL that
//! verifies is a URL this server issued, and one that does not is a 404. An
//! hour after it was issued it stops working ([`TTL_MS`]).
//!
//! This is the reference server's design, kept deliberately —
//! `reference/t3code-server/src/assets/AssetAccess.ts` signs the same claims
//! with the same construction. What differs is the encoding: upstream writes
//! base64url and this writes hex, because the token is opaque to every reader
//! except this file and hex needs no dependency. Nothing in the client parses
//! it.
//!
//! It also differs from how ticket 73 issued *auth* credentials, which are rows
//! in SQLite. That was the right answer there and is the wrong one here: a
//! pairing credential must be revocable, and revocation is what a row buys. An
//! asset URL is issued per project per page load and is never revoked — it
//! expires — so a row per issuance would be a write, and an `fsync`, for a
//! favicon.
//!
//! ## Confinement, twice
//!
//! The claims name a workspace root and a path inside it. Both are checked when
//! the URL is issued and **again** when it is served, because the answer can
//! change in between: a path that was a file inside the project when the token
//! was minted can be a symlink out of it a minute later, and only the check at
//! the moment of reading is about the file that is actually about to be sent.
//! The lexical half is [`crate::files::within`], which is the same code
//! `projects.readFile` is confined by; the other half is `canonicalize` on both
//! ends and a prefix test, here.
//!
//! ## One resource of three
//!
//! `AssetResource` in `packages/contracts/src/assets.ts` has three shapes and
//! this answers for one, `project-favicon`. `workspace-file` and `attachment`
//! are refused by name through [`crate::refusals::partial_refusal`] rather than
//! silently mishandled — attachments in particular are PARITY-LEDGER M7, where
//! the cost of a silent answer has already been paid once.
//!
//! ## The missing icon has a URL too
//!
//! A project with no icon is the common case, and answering the method with an
//! error would make it an error the client has to render. Upstream instead
//! issues a perfectly good URL whose *filename* is [`FALLBACK_MARKER`], and
//! `packages/shared/src/projectFavicon.ts` recognises it and draws a folder —
//! without ever making the request. So the marker is a wire constant shared
//! with the client, and [`serve`] answers 404 for a token that carries it,
//! because a client that asks anyway is asking for a file that is not there.

use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::files::within;
use crate::project_favicon;
use crate::projects::WorkspaceRoot;

/// The method, as `packages/contracts/src/rpc.ts` names it.
pub const CREATE_URL: &str = "assets.createUrl";

/// Where an issued URL points. Upstream's prefix, because the client resolves
/// the relative URL this returns against the http base and nothing else.
pub const ROUTE_PREFIX: &str = "/api/assets";

/// The filename that means "this project has no icon".
///
/// `PROJECT_FAVICON_FALLBACK_MARKER` in `packages/shared/src/projectFavicon.ts`,
/// and a wire constant in both directions — `assets::tests` and that package's
/// own test are the two ends of it.
pub const FALLBACK_MARKER: &str = "project-favicon-missing";

/// How long an issued URL lasts. Upstream's hour.
///
/// Long enough that a sidebar rendered once keeps its icons for a working
/// session, short enough that a URL which leaks into a log is not a permanent
/// handle on a file.
pub const TTL_MS: i64 = 60 * 60 * 1000;

/// The name the signing key is kept under.
pub const SIGNING_SECRET_NAME: &str = "asset-access-signing-key";

/// How much key. Upstream's 32 bytes, which is HMAC-SHA256's block-filling
/// size and past the point where more would add anything.
pub const SIGNING_SECRET_BYTES: usize = 32;

/// The most of a project's icon that will be read.
///
/// An icon is kilobytes. The cap is not about icons — it is that `iconPath` is
/// a string in a file in the project, so a project can point this at its own
/// 2 GB video and the read would be a 2 GB allocation.
const MAX_ASSET_BYTES: u64 = 4 * 1024 * 1024;

type Signer = Hmac<Sha256>;

/// What a signed URL says, once it is opened.
///
/// `kind` is a string rather than an enum with one variant because the field
/// exists to make a *second* kind possible without every token issued before it
/// becoming unreadable — and a token that names a kind this build does not know
/// is refused rather than guessed at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claims {
    version: u8,
    kind: String,
    /// The project, canonical. Stored resolved so that the check at serve time
    /// compares two paths the filesystem agrees about.
    workspace_root: String,
    /// The icon inside it, or `None` for a project that has none.
    relative_path: Option<String>,
    expires_at: i64,
}

const CLAIMS_VERSION: u8 = 1;
const KIND_PROJECT_FAVICON: &str = "project-favicon";

// ---------------------------------------------------------------------------
// assets.createUrl
// ---------------------------------------------------------------------------

/// A validated `assets.createUrl` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateUrl {
    /// The resource exactly as the client sent it. Every error in this method's
    /// union carries it back, so it is kept whole rather than rebuilt — a
    /// rebuilt one could differ from what was asked about, which is the one
    /// thing the field is for.
    resource: Value,
    cwd: String,
}

impl CreateUrl {
    pub fn read(payload: &Value) -> Result<CreateUrl, Value> {
        let resource = payload.get("resource").cloned().unwrap_or(Value::Null);
        let tag = resource.get("_tag").and_then(Value::as_str).unwrap_or_default();

        match tag {
            KIND_PROJECT_FAVICON => {}
            "workspace-file" | "attachment" => {
                return Err(crate::refusals::partial_refusal(CREATE_URL, tag))
            }
            other => {
                return Err(crate::refusals::partial_refusal(
                    CREATE_URL,
                    if other.is_empty() { "a resource with no _tag" } else { other },
                ))
            }
        }

        let cwd = resource
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if cwd.is_empty() {
            return Err(error(
                "AssetWorkspaceRootNormalizationError",
                &resource,
                "This call needs a workspace root; none was given.",
            ));
        }

        Ok(CreateUrl { resource, cwd })
    }

    /// The refusal for a key that could not be loaded.
    ///
    /// On [`CreateUrl`] rather than free, because the tag's class requires the
    /// resource and this is the only thing holding it — the key is loaded
    /// before the work is deferred, where the caller has the call and nothing
    /// else.
    pub fn signing_key_error(&self, failure: impl std::fmt::Display) -> Value {
        error(
            "AssetSigningKeyLoadError",
            &self.resource,
            format_args!("The asset signing key could not be loaded: {failure}"),
        )
    }

    /// Do the work. Blocking, and called from a blocking task.
    pub fn run(self, secret: &[u8], now_ms: i64) -> Result<Value, Value> {
        let root = WorkspaceRoot::check(&self.cwd).map_err(|rejection| {
            error(
                "AssetWorkspaceRootNormalizationError",
                &self.resource,
                rejection.message(),
            )
        })?;

        // Canonical from here on. The path that goes into the claims is the one
        // the serve-time check will compare against, so resolving it once, now,
        // is what stops the two from ever disagreeing about the same directory
        // spelled two ways.
        let root = std::fs::canonicalize(root.path()).map_err(|failure| {
            error(
                "AssetProjectFaviconInspectionError",
                &self.resource,
                format_args!("The project folder could not be resolved: {failure}"),
            )
        })?;

        let relative_path = match project_favicon::resolve(&root) {
            Some(icon) => match icon.strip_prefix(&root) {
                Ok(relative) => Some(relative.to_string_lossy().replace('\\', "/")),
                // Not reachable through `within`, which builds every candidate
                // by descending from the root. Answered rather than asserted
                // because a panic here would take the connection with it.
                Err(_) => None,
            },
            None => None,
        };

        let file_name = relative_path
            .as_deref()
            .and_then(|relative| relative.rsplit('/').next())
            .unwrap_or(FALLBACK_MARKER)
            .to_string();

        let expires_at = now_ms.saturating_add(TTL_MS);
        let claims = Claims {
            version: CLAIMS_VERSION,
            kind: KIND_PROJECT_FAVICON.to_string(),
            workspace_root: root.to_string_lossy().into_owned(),
            relative_path,
            expires_at,
        };

        Ok(json!({
            "relativeUrl": format!("{ROUTE_PREFIX}/{}/{}", seal(&claims, secret), encode_segment(&file_name)),
            "expiresAt": expires_at,
        }))
    }
}

/// One of this method's declared errors, with the resource it is about.
///
/// Every member of `AssetAccessError` carries `resource`, and a required field
/// left out fails the client's decode exactly as a wrong `_tag` would — the
/// same rule [`crate::refusals`] is built around. `cause` is `Schema.Defect()`,
/// which takes anything, so the sentence goes there as well as in `message`:
/// the client renders one or the other depending on where it caught it.
fn error(tag: &str, resource: &Value, message: impl std::fmt::Display) -> Value {
    let message = message.to_string();
    json!({
        "_tag": tag,
        "resource": resource,
        "cause": message,
        "message": message,
    })
}

// ---------------------------------------------------------------------------
// GET /api/assets/{token}/{name}
// ---------------------------------------------------------------------------

/// A file, ready to be a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

/// The file a token stands for, or `None` for every way that can fail.
///
/// One `None` rather than a reason, and deliberately: the reasons are "the
/// signature is wrong", "it expired", "the file moved" and "the path escaped",
/// and telling them apart would tell whoever is guessing at tokens which part
/// of the guess was close. The caller answers all of them with 404.
///
/// The filename in the URL is **not** consulted. It is there so a saved image
/// has a sensible name and so the URL looks like what it returns; the claims
/// are what say which file this is, and upstream's own tests pass a deliberately
/// wrong name and still expect the right file.
pub fn serve(token: &str, secret: &[u8], now_ms: i64) -> Option<Served> {
    let claims = open(token, secret)?;
    if claims.version != CLAIMS_VERSION || claims.kind != KIND_PROJECT_FAVICON {
        return None;
    }
    if claims.expires_at <= now_ms {
        return None;
    }

    // A token minted for a project with no icon. Its URL was only ever meant to
    // be recognised and not fetched, so a client that fetched it anyway is
    // asking for a file that does not exist.
    let relative_path = claims.relative_path?;
    let path = confirm(Path::new(&claims.workspace_root), &relative_path)?;

    Some(Served {
        content_type: crate::ui::content_type(&relative_path),
        bytes: read_at_most(&path, MAX_ASSET_BYTES)?,
    })
}

/// Where `relative` really goes under `root`, if that is still inside it and
/// still a file.
fn confirm(root: &Path, relative: &str) -> Option<PathBuf> {
    let absolute = within(root, Path::new(relative))?;

    let real_root = std::fs::canonicalize(root).ok()?;
    let real = std::fs::canonicalize(&absolute).ok()?;
    if !real.starts_with(&real_root) {
        return None;
    }

    std::fs::metadata(&real)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|_| real)
}

fn read_at_most(path: &Path, wanted: u64) -> Option<Vec<u8>> {
    use std::io::Read;

    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(wanted).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

// ---------------------------------------------------------------------------
// The token
// ---------------------------------------------------------------------------

/// `<hex of the claims>.<hex of their signature>`.
fn seal(claims: &Claims, secret: &[u8]) -> String {
    let payload = hex(
        serde_json::to_string(claims)
            .expect("claims are four owned fields and always serialize")
            .as_bytes(),
    );
    let signature = hex(&sign(&payload, secret));
    format!("{payload}.{signature}")
}

/// The claims inside a token, if it was signed with this key.
fn open(token: &str, secret: &[u8]) -> Option<Claims> {
    let (payload, signature) = token.split_once('.')?;

    let mut signer = Signer::new_from_slice(secret).ok()?;
    signer.update(payload.as_bytes());
    // `verify_slice` compares in constant time. A `==` here would leak how much
    // of a guessed signature was right, one byte at a time.
    signer.verify_slice(&unhex(signature)?).ok()?;

    serde_json::from_slice(&unhex(payload)?).ok()
}

fn sign(payload: &str, secret: &[u8]) -> Vec<u8> {
    let mut signer = Signer::new_from_slice(secret).expect("HMAC takes a key of any length");
    signer.update(payload.as_bytes());
    signer.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

/// Percent-encode the one path segment this builds, which is a filename that
/// came off a disk and can hold anything a filename can.
///
/// Hand-written because it is the only place in this server that needs it and
/// the alphabet is short: unreserved characters through, everything else as
/// `%XX`. `/` in particular must not survive, or a filename would add a segment
/// to the URL and the route would stop matching.
fn encode_segment(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            other => {
                use std::fmt::Write;
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";
    const NOW: i64 = 1_700_000_000_000;

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    fn write(root: &Path, relative: &str, contents: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a parent directory");
        }
        std::fs::write(path, contents).expect("a file");
    }

    fn call(cwd: &str) -> CreateUrl {
        CreateUrl::read(&json!({"resource": {"_tag": "project-favicon", "cwd": cwd}}))
            .expect("a well-formed call")
    }

    /// The token out of an issued URL, which is the part after the prefix and
    /// before the filename.
    fn token_of(answer: &Value) -> String {
        let url = answer["relativeUrl"].as_str().expect("a relative url");
        let suffix = url
            .strip_prefix(&format!("{ROUTE_PREFIX}/"))
            .expect("the route prefix");
        suffix
            .split_once('/')
            .expect("a token and a filename")
            .0
            .to_string()
    }

    fn file_name_of(answer: &Value) -> String {
        let url = answer["relativeUrl"].as_str().expect("a relative url");
        url.rsplit('/').next().expect("a filename").to_string()
    }

    #[test]
    fn a_projects_icon_is_issued_and_then_served() {
        let workspace = workspace();
        write(workspace.path(), "public/favicon.png", b"\x89PNGthe-icon");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");

        assert_eq!(file_name_of(&answer), "favicon.png");
        assert_eq!(answer["expiresAt"], json!(NOW + TTL_MS));

        let served = serve(&token_of(&answer), SECRET, NOW).expect("the icon");
        assert_eq!(served.bytes, b"\x89PNGthe-icon");
        assert_eq!(served.content_type, "image/png");
    }

    /// The whole of the no-icon path: a URL is still issued, its filename is
    /// the marker the client watches for, and fetching it anyway is a 404.
    #[test]
    fn a_project_without_an_icon_is_issued_the_marker() {
        let workspace = workspace();
        write(workspace.path(), "README.md", b"# nothing to see");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");

        assert_eq!(file_name_of(&answer), FALLBACK_MARKER);
        assert_eq!(serve(&token_of(&answer), SECRET, NOW), None);
    }

    #[test]
    fn a_token_signed_with_another_key_is_not_served() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", b"icon");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");

        assert_eq!(
            serve(&token_of(&answer), b"ffffffffffffffffffffffffffffffff", NOW),
            None
        );
    }

    #[test]
    fn a_tampered_token_is_not_served() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", b"icon");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");
        let token = token_of(&answer);

        assert_eq!(serve(&format!("{token}00"), SECRET, NOW), None);
        assert_eq!(serve(&token.replace('.', ""), SECRET, NOW), None);
        assert_eq!(serve("", SECRET, NOW), None);
    }

    /// Rewriting the claims means re-signing them, and the key is what makes
    /// that impossible — so the check is that a *validly shaped* forgery of
    /// another file is refused rather than that a corrupt string is.
    #[test]
    fn claims_pointing_somewhere_else_cannot_be_forged() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", b"icon");

        let elsewhere = Claims {
            version: CLAIMS_VERSION,
            kind: KIND_PROJECT_FAVICON.to_string(),
            workspace_root: workspace.path().to_string_lossy().into_owned(),
            relative_path: Some("../secret.txt".to_string()),
            expires_at: NOW + TTL_MS,
        };

        // Signed with the real key, it is still refused — by confinement.
        assert_eq!(serve(&seal(&elsewhere, SECRET), SECRET, NOW), None);
        // Signed with anything else, it never gets that far.
        assert_eq!(serve(&seal(&elsewhere, b"another key entirely"), SECRET, NOW), None);
    }

    #[test]
    fn an_expired_token_is_not_served() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", b"icon");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");

        assert!(serve(&token_of(&answer), SECRET, NOW + TTL_MS - 1).is_some());
        assert_eq!(serve(&token_of(&answer), SECRET, NOW + TTL_MS), None);
    }

    /// An icon deleted after its URL was issued. The token still verifies, and
    /// the file is what is gone — which is the case the second confinement
    /// check exists for and the reason it is not skipped when the signature is
    /// good.
    #[test]
    fn an_icon_that_has_since_gone_is_not_served() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", b"icon");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");
        std::fs::remove_file(workspace.path().join("favicon.ico")).expect("the icon to go");

        assert_eq!(serve(&token_of(&answer), SECRET, NOW), None);
    }

    #[test]
    fn the_filename_in_the_url_is_not_what_is_served() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", b"icon");

        let answer = call(&workspace.path().to_string_lossy())
            .run(SECRET, NOW)
            .expect("a url");

        // Same token, any name: the claims are what say which file this is.
        let served = serve(&token_of(&answer), SECRET, NOW).expect("the icon");
        assert_eq!(served.bytes, b"icon");
    }

    #[test]
    fn the_other_two_resources_are_refused_by_name() {
        for resource in [
            json!({"_tag": "attachment", "attachmentId": "a1"}),
            json!({"_tag": "workspace-file", "threadId": "t1", "path": "a.png"}),
        ] {
            let tag = resource["_tag"].as_str().expect("a tag").to_string();
            let refusal = CreateUrl::read(&json!({"resource": resource}))
                .expect_err("a refusal");

            assert_eq!(refusal["_tag"], json!("EnvironmentAuthorizationError"));
            let message = refusal["message"].as_str().expect("a message");
            assert!(message.contains(CREATE_URL), "{message}");
            assert!(message.contains(&tag), "{message}");
        }
    }

    #[test]
    fn a_blank_workspace_root_is_refused_with_the_resource_it_was_about() {
        let resource = json!({"_tag": "project-favicon", "cwd": "   "});
        let refusal = CreateUrl::read(&json!({"resource": resource.clone()}))
            .expect_err("a refusal");

        assert_eq!(refusal["_tag"], json!("AssetWorkspaceRootNormalizationError"));
        assert_eq!(refusal["resource"], resource);
    }

    #[test]
    fn a_workspace_that_is_not_there_is_refused() {
        let workspace = workspace();
        let missing = workspace.path().join("no-such-project");

        let refusal = call(&missing.to_string_lossy())
            .run(SECRET, NOW)
            .expect_err("a refusal");

        assert_eq!(refusal["_tag"], json!("AssetWorkspaceRootNormalizationError"));
        assert!(refusal["message"].as_str().is_some_and(|text| !text.is_empty()));
    }

    #[test]
    fn a_filename_that_is_not_url_safe_survives_the_round_trip() {
        assert_eq!(encode_segment("brand icon.png"), "brand%20icon.png");
        assert_eq!(encode_segment("a/b.png"), "a%2Fb.png");
        assert_eq!(encode_segment("logo~1.svg"), "logo~1.svg");
    }

    #[test]
    fn hex_round_trips_and_refuses_what_is_not_hex() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(unhex("000fff"), Some(vec![0x00, 0x0f, 0xff]));
        assert_eq!(unhex("00f"), None);
        assert_eq!(unhex("zz"), None);
    }
}
