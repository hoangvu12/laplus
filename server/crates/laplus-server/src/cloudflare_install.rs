//! An app-managed `cloudflared`, downloaded once and verified before it runs.
//!
//! ADR-0046 is what this implements: setup must not end at a terminal
//! prerequisite, so laplus may install `cloudflared` itself — but only an
//! identified official release, only after the developer approved that exact
//! release, and only into laplus's own data directory. Nothing here touches
//! `PATH`, asks for elevation, or removes an executable laplus did not put
//! there.
//!
//! **Cloudflare publishes the digests in the release notes, not as a file.**
//! Every asset appears once as `<name>: <sha256>` in the body of the GitHub
//! release, and there is no `SHA256SUMS` asset to fetch instead — so the feed
//! read here is the release itself, and [`published_checksum`] is the parser
//! that turns its notes into the one digest this platform's artifact must have.
//!
//! **macOS is deliberately not offered.** Cloudflare ships it as a `.tgz`, and
//! unpacking an archive is a second supply chain — a decompressor and a tar
//! reader — for a platform this application does not release on. A developer
//! there installs `cloudflared` however they like and selects it, which is the
//! path a system executable already takes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Where the release notes and the artifact URLs come from in production.
const DEFAULT_RELEASE_API: &str = "https://api.github.com";
const RELEASE_PATH: &str = "/repos/cloudflare/cloudflared/releases/latest";
/// Sub-directory of the private Cloudflare directory. Separate from the
/// connector's credentials so that "the executable laplus owns" is a directory
/// listing rather than a naming convention.
const TOOLS: &str = "tools";
const RECORD: &str = "installed.json";
/// Generous next to a ~40 MB artifact, and small enough that a feed pointing at
/// something else cannot fill the disk before the digest disagrees.
const ARTIFACT_LIMIT: usize = 256 * 1024 * 1024;

/// The published artifact for this platform, by Cloudflare's own asset names.
pub fn asset_name() -> Option<&'static str> {
    asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Split from [`asset_name`] so the mapping can be checked for platforms the
/// suite does not run on.
pub fn asset_name_for(os: &str, architecture: &str) -> Option<&'static str> {
    match (os, architecture) {
        ("linux", "x86_64") => Some("cloudflared-linux-amd64"),
        ("linux", "aarch64") => Some("cloudflared-linux-arm64"),
        ("linux", "arm") => Some("cloudflared-linux-armhf"),
        ("linux", "x86") => Some("cloudflared-linux-386"),
        ("windows", "x86_64") => Some("cloudflared-windows-amd64.exe"),
        ("windows", "x86") => Some("cloudflared-windows-386.exe"),
        _ => None,
    }
}

/// Why this platform gets no offer, in terms the wizard can show.
pub fn unsupported_message(os: &str) -> String {
    if os == "macos" {
        return "Cloudflare publishes cloudflared for macOS only as an archive. Install it \
                yourself — `brew install cloudflared` — and select the executable above."
            .into();
    }
    "Cloudflare does not publish a cloudflared executable for this platform and architecture. \
     Install it yourself and select the executable above."
        .into()
}

/// One identified release: the version, the artifact, and the digest it must
/// hash to. Assembled from the feed and shown to the developer *before* any
/// bytes are fetched, because approving an installation means approving this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub asset: String,
    pub download_url: String,
    pub checksum: String,
}

/// What was installed, and enough to tell whether it still is.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub version: String,
    pub path: PathBuf,
    pub checksum: String,
    pub asset: String,
    pub installed_at: String,
}

#[derive(Debug, Default)]
struct State {
    record: Option<Record>,
    failure: Option<String>,
    installing: bool,
}

#[derive(Debug)]
pub struct Installer {
    directory: PathBuf,
    state: Mutex<State>,
}

/// Refusals carry the shape the route needs.
#[derive(Debug)]
pub enum Refusal {
    /// Nothing is wrong with the request, but the moment has moved: the feed
    /// publishes a different release than the one approved, or an installation
    /// is already running. Either way the developer looks again and re-approves.
    Conflict(String),
    /// The supply chain said no — an unverifiable release, a digest that did not
    /// match, a download that did not finish.
    Rejected(String),
}

impl Installer {
    /// Reads what a previous run installed. A record whose executable is no
    /// longer on disk is *not* an installation: the wizard has to say what is
    /// true after a half-finished run rather than what was true before it.
    pub fn open(cloudflare_directory: &Path) -> Installer {
        let directory = cloudflare_directory.join(TOOLS);
        let record: Option<Record> = std::fs::read(directory.join(RECORD))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .filter(|record: &Record| record.path.is_file());
        // A `.part` from a download that was interrupted by a shutdown. Nothing
        // resumes it, and leaving it behind would grow the directory once per
        // failed attempt.
        if let Ok(entries) = std::fs::read_dir(&directory) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".part") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        Installer {
            directory,
            state: Mutex::new(State {
                record,
                failure: None,
                installing: false,
            }),
        }
    }

    /// The executable laplus owns, if it owns one.
    pub fn installed_path(&self) -> Option<PathBuf> {
        self.state
            .lock()
            .unwrap()
            .record
            .as_ref()
            .map(|record| record.path.clone())
    }

    /// What is installed *now*, and what it says its version is now.
    ///
    /// Both halves are read fresh rather than trusted from the record, because
    /// both can stop being true without laplus doing anything. The file can go
    /// — a failed reinstall of the same version removes it — and ADR-0046
    /// deliberately keeps cloudflared's own replacement behaviour, so the
    /// executable at that path can become a different version than the one
    /// laplus put there. The record is what was installed; this is what is.
    async fn installed_now(&self) -> Option<(Record, Option<String>)> {
        let record = self.state.lock().unwrap().record.clone()?;
        if !record.path.is_file() {
            self.state.lock().unwrap().record = None;
            return None;
        }
        let live = crate::cloudflare_connector::detected_version(&record.path).await;
        Some((record, live))
    }

    /// The wizard's whole view of installation: what this platform can be
    /// offered, what the feed publishes now, and what is on disk.
    ///
    /// **Reading this reaches Cloudflare's release feed**, so the client asks
    /// for it only once it knows an installation could be offered — which it
    /// can tell from executable discovery alone. Deciding that here instead
    /// would mean this answer depended on whatever `cloudflared` the machine
    /// running the suite happens to have on its `PATH`.
    pub async fn snapshot(&self) -> Value {
        let asset = asset_name();
        let release = match asset {
            Some(asset) => fetch_release(asset).await,
            None => Err(unsupported_message(std::env::consts::OS)),
        };
        let installed = self.installed_now().await;
        let state = self.state.lock().unwrap();
        let (release, release_failure) = match release {
            Ok(release) => (
                json!({
                    "version": release.version,
                    "assetName": release.asset,
                    "downloadUrl": release.download_url,
                    "checksum": release.checksum,
                }),
                Value::Null,
            ),
            Err(message) => (Value::Null, json!(message)),
        };
        json!({
            "supported": asset.is_some(),
            "platform": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "assetName": asset,
            "ownership": "app-managed",
            "unsupportedMessage": asset
                .is_none()
                .then(|| unsupported_message(std::env::consts::OS)),
            "state": if state.installing {
                "installing"
            } else if installed.is_some() {
                "installed"
            } else if state.failure.is_some() {
                "failed"
            } else {
                "not-installed"
            },
            "installedPath": installed.as_ref().map(|(record, _)| record.path.clone()),
            // Two different facts, kept apart on purpose. `installedVersion` is
            // the release laplus fetched and verified; `detectedVersion` is what
            // the executable says it is now, which ADR-0046's tolerance of
            // cloudflared's own self-replacement lets drift away from it.
            "installedVersion": installed.as_ref().map(|(record, _)| record.version.clone()),
            "detectedVersion": installed.as_ref().and_then(|(_, live)| live.clone()),
            "installedAt": installed.as_ref().map(|(record, _)| record.installed_at.clone()),
            "failureMessage": state.failure,
            "release": release,
            "releaseFailureMessage": release_failure,
        })
    }

    /// Download, verify and promote the release the developer approved.
    ///
    /// The approval is re-checked against a fresh read of the feed rather than
    /// against anything the client sent, so a release that moved between the
    /// preview and the button is a conflict and not a silent substitution.
    pub async fn install(&self, version: &str, checksum: &str) -> Result<(), Refusal> {
        let asset = asset_name()
            .ok_or_else(|| Refusal::Rejected(unsupported_message(std::env::consts::OS)))?;
        let release = fetch_release(asset).await.map_err(Refusal::Rejected)?;
        if release.version != version || !release.checksum.eq_ignore_ascii_case(checksum) {
            return Err(Refusal::Conflict(format!(
                "Cloudflare now publishes {} rather than the release you approved. Review the \
                 new release and approve it again.",
                release.version
            )));
        }
        {
            let mut state = self.state.lock().unwrap();
            if state.installing {
                return Err(Refusal::Conflict(
                    "An installation is already running.".into(),
                ));
            }
            state.installing = true;
            state.failure = None;
        }
        let outcome = self.acquire(&release).await;
        let mut state = self.state.lock().unwrap();
        state.installing = false;
        match outcome {
            Ok(record) => {
                state.record = Some(record);
                state.failure = None;
                Ok(())
            }
            Err(message) => {
                // A retry of the version already installed promotes onto the
                // same name, so a failure after that rename has removed the
                // copy the record names. Saying "installed" then would point
                // the wizard at a file that is gone.
                state.record = state.record.take().filter(|record| record.path.is_file());
                state.failure = Some(message.clone());
                Err(Refusal::Rejected(message))
            }
        }
    }

    async fn acquire(&self, release: &Release) -> Result<Record, String> {
        crate::cloudflare_connector::private_directory(&self.directory)?;
        // A partial download is never executable and never has the installed
        // name: promotion is a rename of something already whole and already
        // verified, so an interrupted run cannot leave a runnable file behind.
        let partial = self
            .directory
            .join(format!(".{}-{}.part", release.asset, std::process::id()));
        let _ = std::fs::remove_file(&partial);
        let digest = match download(&release.download_url, &partial).await {
            Ok(digest) => digest,
            Err(message) => {
                let _ = std::fs::remove_file(&partial);
                return Err(message);
            }
        };
        if !digest.eq_ignore_ascii_case(&release.checksum) {
            let _ = std::fs::remove_file(&partial);
            return Err(
                "The downloaded cloudflared did not match Cloudflare's published checksum, so it \
                 was discarded."
                    .into(),
            );
        }
        let installed = self.directory.join(installed_name(&release.version));
        let promotion =
            make_executable(&partial).and_then(|()| std::fs::rename(&partial, &installed));
        if let Err(_error) = promotion {
            let _ = std::fs::remove_file(&partial);
            return Err("The verified cloudflared could not be installed.".into());
        }
        if let Err(message) = crate::cloudflare_connector::compatible_version(&installed).await {
            let _ = std::fs::remove_file(&installed);
            return Err(message);
        }
        // Only ever the copy laplus put here: a system or user-selected
        // executable is never named by a record, so it is never removed.
        if let Some(previous) = self.installed_path() {
            if previous != installed {
                let _ = std::fs::remove_file(previous);
            }
        }
        let record = Record {
            version: release.version.clone(),
            path: installed,
            checksum: release.checksum.clone(),
            asset: release.asset.clone(),
            installed_at: crate::clock::now_iso(),
        };
        let bytes = serde_json::to_vec_pretty(&record)
            .map_err(|_| "The installation record could not be encoded.".to_string())?;
        crate::cloudflare_connector::private_write(&self.directory.join(RECORD), &bytes)?;
        Ok(record)
    }
}

fn installed_name(version: &str) -> String {
    let version: String = version
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '.')
        .collect();
    if cfg!(windows) {
        format!("cloudflared-{version}.exe")
    } else {
        format!("cloudflared-{version}")
    }
}

/// Owner-only, and runnable by nobody else.
///
/// On Windows there is no mode bit to set — what a file may do is its
/// extension, and who may read it is the ACL
/// [`crate::cloudflare_connector::private_directory`] already applied to the
/// directory it sits in.
#[cfg(unix)]
fn make_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Where the release feed is read from.
///
/// The environment variable exists so the suite can serve a stand-in feed and
/// artifact from loopback; it is ignored unless it points at loopback, so it
/// cannot redirect a real installation at an arbitrary host.
fn release_api_origin() -> String {
    if let Ok(origin) = std::env::var("LAPLUS_CLOUDFLARED_RELEASE_API") {
        if loopback_origin(&origin) {
            return origin.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_RELEASE_API.into()
}

fn loopback_origin(origin: &str) -> bool {
    reqwest::Url::parse(origin).is_ok_and(|url| {
        url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
    })
}

/// The one digest this artifact must have, out of the release notes.
///
/// Cloudflare writes one `<asset>: <sha256>` line per published file. A body
/// without a line for this platform's asset is a release laplus cannot verify,
/// which is a refusal rather than a reason to trust the download.
pub fn published_checksum(body: &str, asset: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let (name, digest) = line.trim().split_once(':')?;
        if name.trim() != asset {
            return None;
        }
        let digest = digest.trim().to_ascii_lowercase();
        (digest.len() == 64 && digest.chars().all(|character| character.is_ascii_hexdigit()))
            .then_some(digest)
    })
}

/// An artifact URL is only followed if the feed that named it could have.
///
/// Cloudflare's releases are served from GitHub, so a download that leaves
/// those hosts is not the official artifact however well-formed the feed was.
fn official_artifact_url(url: &str, api_origin: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    if loopback_origin(api_origin) {
        return loopback_origin(url);
    }
    parsed.scheme() == "https"
        && parsed.host_str().is_some_and(|host| {
            host == "github.com"
                || host.ends_with(".github.com")
                || host.ends_with(".githubusercontent.com")
        })
}

async fn fetch_release(asset: &str) -> Result<Release, String> {
    let origin = release_api_origin();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("laplus")
        .build()
        .map_err(|_| "The Cloudflare release feed could not be read.".to_string())?;
    let response = client
        .get(format!("{origin}{RELEASE_PATH}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| "The Cloudflare release feed could not be reached.".to_string())?;
    if !response.status().is_success() {
        return Err("The Cloudflare release feed could not be read.".into());
    }
    let feed: Value = response
        .json()
        .await
        .map_err(|_| "The Cloudflare release feed could not be understood.".to_string())?;
    let version = feed
        .get("tag_name")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or("The Cloudflare release feed named no version.")?
        .to_string();
    let download_url = feed
        .get("assets")
        .and_then(|value| value.as_array())
        .and_then(|assets| {
            assets
                .iter()
                .find(|entry| entry.get("name").and_then(|name| name.as_str()) == Some(asset))
        })
        .and_then(|entry| entry.get("browser_download_url"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            format!("Cloudflare release {version} publishes no {asset} for this platform.")
        })?
        .to_string();
    if !official_artifact_url(&download_url, &origin) {
        return Err("The Cloudflare release feed named an artifact somewhere unofficial.".into());
    }
    let checksum = feed
        .get("body")
        .and_then(|value| value.as_str())
        .and_then(|body| published_checksum(body, asset))
        .ok_or_else(|| {
            format!("Cloudflare release {version} publishes no checksum for {asset}, so it cannot be verified.")
        })?;
    Ok(Release {
        version,
        asset: asset.to_string(),
        download_url,
        checksum,
    })
}

/// Streams the artifact to `partial`, hashing as it goes.
///
/// The file is opened private and non-executable, and the hash is of what was
/// written rather than of a buffer held in memory — a 40 MB artifact does not
/// need to be resident, and a truncated body has to be an error rather than a
/// shorter file that hashes to something.
async fn download(url: &str, partial: &Path) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .user_agent("laplus")
        .build()
        .map_err(|_| "The cloudflared download could not be started.".to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| "The cloudflared download could not be started.".to_string())?;
    if !response.status().is_success() {
        return Err("Cloudflare did not serve the approved cloudflared release.".into());
    }
    let expected = response.content_length();
    let mut file = create_private(partial)?;
    let mut hasher = Sha256::new();
    let mut written: usize = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|_| "The cloudflared download was interrupted before it finished.".to_string())?;
        written = written.saturating_add(chunk.len());
        if written > ARTIFACT_LIMIT {
            return Err("The cloudflared download was larger than any published release.".into());
        }
        hasher.update(&chunk);
        std::io::Write::write_all(&mut file, &chunk)
            .map_err(|_| "The cloudflared download could not be written.".to_string())?;
    }
    std::io::Write::flush(&mut file)
        .and_then(|()| file.sync_all())
        .map_err(|_| "The cloudflared download could not be written.".to_string())?;
    if expected.is_some_and(|length| length != written as u64) {
        return Err("The cloudflared download was interrupted before it finished.".into());
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn create_private(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|_| "The cloudflared download could not be written.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_platform_maps_to_a_published_asset() {
        assert_eq!(
            asset_name_for("linux", "x86_64"),
            Some("cloudflared-linux-amd64")
        );
        assert_eq!(
            asset_name_for("linux", "aarch64"),
            Some("cloudflared-linux-arm64")
        );
        assert_eq!(
            asset_name_for("windows", "x86_64"),
            Some("cloudflared-windows-amd64.exe")
        );
        assert_eq!(asset_name_for("macos", "aarch64"), None);
        assert!(unsupported_message("macos").contains("brew install cloudflared"));
    }

    #[test]
    fn a_checksum_is_read_only_for_this_platforms_asset() {
        let body = "### SHA256 Checksums:\n```\ncloudflared-linux-amd64.deb: aa\n\
                    cloudflared-linux-amd64: 9d71c677db00134c1bd4144b7783486b654ad281b1ea62b4972098d19f770f17\n\
                    cloudflared-linux-arm64: 65259e652a7bea08bf5df603233ab22b8bf3116af8df9f9206209af6a1b955c0\n```";
        assert_eq!(
            published_checksum(body, "cloudflared-linux-amd64").as_deref(),
            Some("9d71c677db00134c1bd4144b7783486b654ad281b1ea62b4972098d19f770f17")
        );
        assert_eq!(published_checksum(body, "cloudflared-linux-386"), None);
        assert_eq!(published_checksum(body, "cloudflared-linux-amd64.deb"), None);
    }

    #[test]
    fn an_artifact_is_followed_only_where_the_feed_could_have_published_it() {
        assert!(official_artifact_url(
            "https://github.com/cloudflare/cloudflared/releases/download/2026.7.3/cloudflared-linux-amd64",
            DEFAULT_RELEASE_API,
        ));
        assert!(!official_artifact_url(
            "https://example.com/cloudflared-linux-amd64",
            DEFAULT_RELEASE_API,
        ));
        assert!(!official_artifact_url(
            "http://127.0.0.1:9/cloudflared-linux-amd64",
            DEFAULT_RELEASE_API,
        ));
        assert!(official_artifact_url(
            "http://127.0.0.1:9/download/cloudflared-linux-amd64",
            "http://127.0.0.1:9",
        ));
    }

    #[test]
    fn a_release_feed_origin_is_only_overridden_towards_loopback() {
        assert!(loopback_origin("http://127.0.0.1:8080"));
        assert!(loopback_origin("http://localhost:8080"));
        assert!(!loopback_origin("https://example.com"));
        assert!(!loopback_origin("not a url"));
    }
}
