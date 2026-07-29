//! The UI, answered by the same server the UI talks to.
//!
//! Ticket 23 makes laplus a desktop application, and the first question a
//! shell has to answer is where the page comes from. Tauri's own answer is to
//! embed the assets and serve them from a custom scheme — which on Windows is
//! `http://tauri.localhost`, a **different origin** from the server on
//! `127.0.0.1`. Everything the UI does would then be cross-origin, and three
//! separate things break at once:
//!
//! - the socket upgrade is refused, because [`crate::auth`] takes only loopback
//!   origins — and widening it would widen it for a real browser too;
//! - the UI's boot fetches are relative (`/.well-known/t3/environment`,
//!   `/api/auth/session`) and would go to the scheme handler rather than the
//!   server, so the only fix is to rebuild the web bundle with `VITE_HTTP_URL`
//!   baked in — a Node build of vendored code this repository does not own;
//! - `localStorage` is per-origin, and the UI keeps the developer's layout,
//!   drafts and selections there.
//!
//! So the assets are served **by this server, from the same origin as
//! everything else**, and the shell's webview is simply pointed at
//! `http://127.0.0.1:<port>/`. Nothing here is cross-origin, no capture-only
//! contract is widened, and the vendored bundle is shipped exactly as upstream
//! built it.
//!
//! The split with the shell is deliberate: **this module is the policy, the
//! shell is the payload.** What a path resolves to, what content type it gets
//! and what may be cached is decided and tested here against a handful of
//! bytes; the real 17 MB of `apps/web/dist` is a static table generated
//! in `laplus-shell`'s build script and handed in at startup. A server crate
//! that embedded the bundle itself would put it into every test binary, and the
//! suite would pay for it on every build.
//!
//! There is no filesystem here. [`Assets`] is a fixed table, so a path that is
//! not in it cannot be read — `..` is not defended against because it cannot
//! reach anything.

use std::borrow::Cow;
use std::collections::BTreeMap;

/// Where the web bundle's content-hashed output lives, as Vite emits it.
///
/// The one thing that distinguishes a file safe to cache forever from one that
/// is not: everything under here has a hash in its name and a new build gives
/// it a new name, while `index.html` and the icons beside it keep theirs.
const HASHED_DIRECTORY: &str = "assets/";

/// The file a client-side route falls back to.
const ENTRY_POINT: &str = "index.html";

/// Path prefixes that belong to the server's own surface, and must keep
/// answering 404 rather than being handed the UI.
///
/// A `GET /api/something-we-do-not-implement` that returned an HTML page would
/// be decoded by the client as a malformed response to a real call, instead of
/// the plain 404 `tests/http_boot.rs` pins. `/ws` needs no entry: it is a real
/// route, so it never reaches the fallback.
///
/// The other half of this list is the router in [`crate::server`], and they have
/// to be read together: a route added there under a *new* prefix needs that
/// prefix here too, or its unimplemented siblings quietly start answering with
/// the UI. Nothing fails if it is forgotten, which is why it is said here.
const SERVER_SURFACE: [&str; 2] = ["api/", ".well-known/"];

/// The UI's files, by path relative to the bundle root.
///
/// Empty for every server the suite starts and for the plain binary: the UI is
/// something the *shell* brings, and a server without one answers 404 exactly
/// as it did before ticket 23.
#[derive(Debug, Clone, Default)]
pub struct Assets {
    files: BTreeMap<String, Cow<'static, [u8]>>,
    /// What the bundle calls itself — `@t3tools/web`'s `package.json` version,
    /// which is also the `APP_VERSION` compiled into these files.
    ///
    /// It travels with the bytes rather than beside them because the server
    /// reports it as its own `serverVersion`, and a bundle whose version had to
    /// be remembered separately is one that would eventually be shipped with
    /// somebody else's number. [`crate::config::ServerConfig::serving_ui_version`]
    /// is where that matters and why.
    version: Option<&'static str>,
}

/// One file, ready to be written as a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset<'a> {
    /// Which file answered. Not always the path that was asked for — see
    /// [`Assets::resolve`].
    pub path: &'a str,
    pub bytes: &'a [u8],
    pub content_type: &'static str,
    pub caching: Caching,
}

/// How long the webview may keep a file without asking again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caching {
    /// Content-hashed: the name changes when the bytes do, so it never needs
    /// revalidating. This is what makes the second launch fast — 17 MB of
    /// JavaScript is not re-read from the socket every time the window opens.
    Immutable,
    /// Everything else. Cached, but checked on each load, because the name
    /// stays the same across builds and a stale `index.html` would point at
    /// assets that are gone.
    Revalidate,
}

impl Caching {
    /// The `Cache-Control` value. A method rather than a field so that the two
    /// spellings cannot drift apart in the handler that writes them.
    pub fn header(self) -> &'static str {
        match self {
            Caching::Immutable => "public, max-age=31536000, immutable",
            Caching::Revalidate => "no-cache",
        }
    }
}

impl Assets {
    /// No UI. What the plain binary and the whole suite run with.
    pub fn none() -> Assets {
        Assets::default()
    }

    /// The shell's generated table: names and bytes that live in the
    /// executable, and the version the bundle was built as.
    ///
    /// The version is an argument rather than a later `with_` call because a
    /// bundle always has one — the shell's build script reads both out of the
    /// same directory — and the one thing this must not be is optional to
    /// supply.
    pub fn from_static(files: &[(&'static str, &'static [u8])], version: &'static str) -> Assets {
        Assets {
            files: files
                .iter()
                .map(|(path, bytes)| ((*path).to_string(), Cow::Borrowed(*bytes)))
                .collect(),
            version: Some(version),
        }
    }

    /// Whether this server has a UI at all. The distinction the plain binary
    /// and the whole suite live on the other side of.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The version of the UI in here, or `None` for a server that brought no
    /// UI and therefore speaks only for itself.
    pub fn version(&self) -> Option<&'static str> {
        self.version
    }

    /// Answer one `GET`, or decline it.
    ///
    /// Three rules, in order, and the second and third are where the judgement
    /// is:
    ///
    /// 1. The path names a file we have. Serve it.
    /// 2. It does not, and it looks like a **file** — its last segment has an
    ///    extension, or it is under the server's own surface. That is a 404. A
    ///    missing `assets/index-abc.js` must not come back as HTML: the webview
    ///    would run the page as a script and report a syntax error instead of a
    ///    missing file.
    /// 3. It does not, and it looks like a **route** (`/settings`,
    ///    `/thread/1a2b`). The UI is a single-page application whose router
    ///    lives in the browser, so every one of its routes is a path this
    ///    server has never heard of. Serve the entry point and let the client
    ///    route it.
    pub fn resolve(&self, path: &str) -> Option<Asset<'_>> {
        let requested = path.trim_start_matches('/');
        let requested = if requested.is_empty() {
            ENTRY_POINT
        } else {
            requested
        };

        self.serving(requested).or_else(|| {
            is_client_route(requested)
                .then(|| self.serving(ENTRY_POINT))
                .flatten()
        })
    }

    /// One file by its exact name, described.
    fn serving(&self, name: &str) -> Option<Asset<'_>> {
        let (path, bytes) = self.files.get_key_value(name)?;
        Some(Asset {
            path,
            bytes,
            content_type: content_type(path),
            caching: caching(path),
        })
    }
}

/// Could this path be one of the UI's own routes, rather than a file it failed
/// to find?
fn is_client_route(path: &str) -> bool {
    if SERVER_SURFACE.iter().any(|prefix| path.starts_with(prefix)) {
        return false;
    }
    let last_segment = path.rsplit('/').next().unwrap_or_default();
    !last_segment.contains('.')
}

fn caching(path: &str) -> Caching {
    if path.starts_with(HASHED_DIRECTORY) {
        Caching::Immutable
    } else {
        Caching::Revalidate
    }
}

/// The content type for a file name.
///
/// A short table rather than a dependency, covering what a Vite bundle emits.
/// The default is deliberate: `application/octet-stream` makes an unknown type
/// a download the page ignores, where a guess of `text/html` would make it
/// something the webview tries to run.
pub(crate) fn content_type(path: &str) -> &'static str {
    let extension = path.rsplit('.').next().unwrap_or_default().to_ascii_lowercase();
    match extension.as_str() {
        "html" => "text/html; charset=utf-8",
        // Not `application/javascript`: a module script served as anything but
        // a JavaScript MIME type is refused outright by the browser, and this
        // is the spelling the HTML standard settled on.
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of a Vite bundle, at four files.
    fn bundle() -> Assets {
        Assets::from_static(
            &[
                ("index.html", b"<!doctype html><div id=root></div>"),
                ("assets/index-a1b2c3.js", b"export default 1"),
                ("assets/index-d4e5f6.css", b":root{}"),
                ("favicon.ico", b"\x00\x00\x01\x00"),
            ],
            "0.0.28",
        )
    }

    #[test]
    fn a_server_with_no_ui_answers_nothing() {
        let none = Assets::none();
        assert!(none.is_empty());
        assert_eq!(none.resolve("/"), None);
        assert_eq!(none.resolve("/index.html"), None);
        assert_eq!(none.resolve("/settings"), None);
    }

    /// Ticket 26: the number the server reports as its own comes from here, so
    /// a bundle knows what it is and a server with no bundle says nothing.
    #[test]
    fn a_bundle_carries_its_version_and_the_absence_of_one_carries_none() {
        assert_eq!(bundle().version(), Some("0.0.28"));
        assert_eq!(Assets::none().version(), None);
    }

    #[test]
    fn the_root_is_the_entry_point() {
        let assets = bundle();
        let root = assets.resolve("/").expect("the root is served");
        assert_eq!(root.path, "index.html");
        assert_eq!(root.content_type, "text/html; charset=utf-8");
        assert_eq!(root.bytes, b"<!doctype html><div id=root></div>");
    }

    #[test]
    fn a_file_in_the_bundle_is_served_with_its_own_type() {
        let assets = bundle();
        for (path, content_type) in [
            ("/index.html", "text/html; charset=utf-8"),
            ("/assets/index-a1b2c3.js", "text/javascript; charset=utf-8"),
            ("/assets/index-d4e5f6.css", "text/css; charset=utf-8"),
            ("/favicon.ico", "image/x-icon"),
        ] {
            let asset = assets.resolve(path).unwrap_or_else(|| panic!("{path}"));
            assert_eq!(asset.path, path.trim_start_matches('/'));
            assert_eq!(asset.content_type, content_type, "{path}");
        }
    }

    /// The UI's router lives in the browser, so its routes are paths this
    /// server has never heard of — and a reload on one of them has to render
    /// the app rather than a 404.
    #[test]
    fn a_client_side_route_is_answered_with_the_entry_point() {
        let assets = bundle();
        for route in ["/settings", "/project/1a2b/thread/3c4d", "/settings/"] {
            let asset = assets.resolve(route).unwrap_or_else(|| panic!("{route}"));
            assert_eq!(asset.path, "index.html", "{route}");
        }
    }

    /// The rule that keeps the fallback from doing harm. A missing script that
    /// came back as HTML would be *run* as HTML, and the developer would read a
    /// syntax error instead of "that file is not there".
    #[test]
    fn a_missing_file_is_missing_rather_than_the_entry_point() {
        let assets = bundle();
        for path in [
            "/assets/index-gone.js",
            "/assets/index-gone.css",
            "/favicon.png",
        ] {
            assert_eq!(assets.resolve(path), None, "{path}");
        }
    }

    /// `tests/http_boot.rs` pins these as plain 404s, and they are answers the
    /// client decodes. Attaching a UI must not change what an unimplemented
    /// method says.
    #[test]
    fn the_servers_own_surface_is_never_answered_with_the_ui() {
        let assets = bundle();
        for path in [
            "/api/orchestration/shell",
            "/api/auth/browser-session",
            "/.well-known/openid-configuration",
        ] {
            assert_eq!(assets.resolve(path), None, "{path}");
        }
    }

    /// Nothing here opens a file, so the classic escape has nothing to escape
    /// from — a table lookup either names an asset or it does not. Pinned
    /// because "we serve files over HTTP" is a sentence that usually means
    /// otherwise.
    #[test]
    fn a_traversal_is_a_lookup_that_misses_rather_than_a_file_that_is_read() {
        let assets = bundle();
        for path in [
            "/../Cargo.toml",
            "/assets/../../secrets.txt",
            "/..%2fCargo.toml",
        ] {
            assert_eq!(assets.resolve(path), None, "{path}");
        }
    }

    /// Hashed output may be kept forever; anything whose name survives a
    /// rebuild may not. Getting this backwards is a second launch that shows
    /// the previous build's page.
    #[test]
    fn only_content_hashed_output_is_cached_forever() {
        let assets = bundle();
        assert_eq!(
            assets.resolve("/assets/index-a1b2c3.js").expect("js").caching,
            Caching::Immutable
        );
        for path in ["/", "/index.html", "/favicon.ico", "/settings"] {
            assert_eq!(
                assets.resolve(path).unwrap_or_else(|| panic!("{path}")).caching,
                Caching::Revalidate,
                "{path}"
            );
        }
        assert_eq!(Caching::Revalidate.header(), "no-cache");
        assert!(Caching::Immutable.header().contains("immutable"));
    }

    /// An unknown extension is a download, not something the webview will try
    /// to render.
    #[test]
    fn an_unrecognised_extension_is_not_guessed_at() {
        assert_eq!(content_type("LICENCE"), "application/octet-stream");
        assert_eq!(content_type("some.thing"), "application/octet-stream");
        assert_eq!(content_type("INDEX.HTML"), "text/html; charset=utf-8");
    }
}
