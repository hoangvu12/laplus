//! Which file in a project stands for it, in a sidebar row.
//!
//! The UI has drawn a folder next to every project since the fork, and not
//! because it wanted to: `ProjectFavicon.tsx` asks for the project's own icon
//! and falls back to `FolderIcon` when the answer does not come. Nothing here
//! is new UI — this module is the answer that was missing.
//!
//! ## The order is upstream's, and the order is the feature
//!
//! `pingdotgg/t3code:apps/server/src/project/ProjectFaviconResolver.ts` looks in
//! three places, in this order, and stops at the first hit:
//!
//! 1. **`t3.json`'s `iconPath`.** A project that says what its icon is has
//!    settled the question, which is why this is first and why a declared path
//!    that does not exist falls through to the guesses rather than failing —
//!    the file may simply not have been built yet.
//! 2. **[`CANDIDATES`]**, twenty-one well-known paths. The order inside that
//!    list matters too: `favicon.svg` before `favicon.ico` because a project
//!    with both has the sharper one first, and `public/` before `app/` because
//!    a Vite project has the former and a Next project the latter, so the
//!    common case is not paying for the rare one.
//! 3. **[`ICON_SOURCES`]**, seven files that *declare* an icon rather than
//!    being one — an `index.html` with `<link rel="icon">`, or a TanStack or
//!    Remix root route with the same thing as an object. The href is then
//!    looked for under `public/` **and** at the root, because a declaration of
//!    `/favicon.png` means the first in a Vite project and the second in
//!    something serving the tree directly.
//!
//! Returning `None` is an ordinary answer and not a failure: most projects have
//! no icon, and the caller turns that into the marker filename that tells the
//! client to draw its folder without asking for anything.
//!
//! ## No regexes
//!
//! Upstream matches the declaration forms with two regular expressions, using
//! lookahead so `rel` and `href` may appear in either order. There is no regex
//! crate in this workspace and this does not justify adding one, so
//! [`icon_href`] scans instead: it walks the `<…>` and `{…}` spans of the file
//! and asks each one for a `rel` and an `href`, which is what those lookaheads
//! amount to. The scan is deliberately dumber than a parser — it will read a
//! `<link rel="icon">` inside a comment — and that is acceptable for a guess at
//! a decoration, where the cost of being wrong is the wrong small picture.
//!
//! Confinement is [`crate::files::within`], not a second copy of the rule: a
//! declared `iconPath` of `../../.ssh/id_rsa` is a path from a file in the
//! project, and is refused for the same reason and by the same code that
//! refuses it from a `projects.readFile` call.

use std::path::{Path, PathBuf};

use crate::files::within;

/// The project file a workspace may declare its icon in.
pub const PROJECT_FILE: &str = "t3.json";

/// The most of a declaring source that is read looking for a `<link>`.
///
/// An `index.html` is a few kilobytes and a root route a few more. The cap is
/// here because nothing stops a project from having a 400 MB `index.html`, and
/// the scan below would otherwise hold all of it to look at the first tag.
const MAX_SOURCE_BYTES: u64 = 512 * 1024;

/// Well-known icon paths, in the order they are tried.
pub const CANDIDATES: &[&str] = &[
    "favicon.svg",
    "favicon.ico",
    "favicon.png",
    "public/favicon.svg",
    "public/favicon.ico",
    "public/favicon.png",
    "app/favicon.ico",
    "app/favicon.png",
    "app/icon.svg",
    "app/icon.png",
    "app/icon.ico",
    "src/favicon.ico",
    "src/favicon.svg",
    "src/app/favicon.ico",
    "src/app/icon.svg",
    "src/app/icon.png",
    "assets/icon.svg",
    "assets/icon.png",
    "assets/logo.svg",
    "assets/logo.png",
    ".idea/icon.svg",
];

/// Files that may name an icon rather than be one.
pub const ICON_SOURCES: &[&str] = &[
    "index.html",
    "public/index.html",
    "app/routes/__root.tsx",
    "src/routes/__root.tsx",
    "app/root.tsx",
    "src/root.tsx",
    "src/index.html",
];

/// The project's icon, absolute, or `None` if it has none.
///
/// `root` is expected to be a directory that exists — the caller has already
/// put it through [`crate::projects::WorkspaceRoot`] — and every path this
/// returns is a regular file inside it.
pub fn resolve(root: &Path) -> Option<PathBuf> {
    if let Some(declared) = declared_icon_path(root) {
        if let Some(found) = existing_file(root, &declared) {
            return Some(found);
        }
    }

    for candidate in CANDIDATES {
        if let Some(found) = existing_file(root, candidate) {
            return Some(found);
        }
    }

    for source in ICON_SOURCES {
        let Some(text) = read_source(root, source) else {
            continue;
        };
        let Some(href) = icon_href(&text) else {
            continue;
        };
        // A leading slash is a URL's root and not the disk's, so it is stripped
        // rather than honoured — honouring it is how `/etc/passwd` would become
        // a candidate.
        let clean = href.trim_start_matches('/');
        if clean.is_empty() {
            continue;
        }
        for candidate in [format!("public/{clean}"), clean.to_string()] {
            if let Some(found) = existing_file(root, &candidate) {
                return Some(found);
            }
        }
    }

    None
}

/// `iconPath` out of the project's `t3.json`, if it has one.
///
/// Read with `serde_json` and no schema: this wants one string out of a file
/// whose other keys are none of its business, and a strict decode would mean a
/// project that has grown a key this build does not know about loses its icon.
fn declared_icon_path(root: &Path) -> Option<String> {
    let text = read_source(root, PROJECT_FILE)?;
    let document: serde_json::Value = serde_json::from_str(&text).ok()?;
    let declared = document.get("iconPath")?.as_str()?.trim();
    if declared.is_empty() {
        None
    } else {
        Some(declared.to_string())
    }
}

/// The absolute path of `relative` under `root`, if it is a file that is really
/// there.
///
/// Only the lexical half of confinement is checked here. The other half — where
/// the path goes once symlinks are followed — is checked by [`crate::assets`]
/// at the moment the file is actually served, which is both the last chance to
/// check it and the only moment at which the answer is still true.
fn existing_file(root: &Path, relative: &str) -> Option<PathBuf> {
    let absolute = within(root, Path::new(relative.trim()))?;
    std::fs::metadata(&absolute)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|_| absolute)
}

/// A declaring file's text, or `None` if it is absent, too large, or not UTF-8.
fn read_source(root: &Path, relative: &str) -> Option<String> {
    use std::io::Read;

    let absolute = within(root, Path::new(relative))?;
    let file = std::fs::File::open(&absolute).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }

    let mut text = String::new();
    file.take(MAX_SOURCE_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// The `href` of the first icon declaration in `source`.
///
/// Accepts both spellings upstream accepts — `<link rel="icon" href="…">` and
/// `{ rel: "icon", href: "…" }` — in either field order, and both quote
/// characters. A query string is dropped, because `?v=2` is a cache-buster on a
/// URL and not part of a filename.
pub fn icon_href(source: &str) -> Option<String> {
    for span in spans(source) {
        let Some(rel) = attribute(span, "rel") else {
            continue;
        };
        if !rel.eq_ignore_ascii_case("icon") && !rel.eq_ignore_ascii_case("shortcut icon") {
            continue;
        }
        let Some(href) = attribute(span, "href") else {
            continue;
        };
        let href = href.split('?').next().unwrap_or_default().trim();
        if !href.is_empty() {
            return Some(href.to_string());
        }
    }
    None
}

/// Every `<…>` and `{…}` region of a source, which is as much structure as
/// [`icon_href`] needs: both declaration forms put their `rel` and their `href`
/// inside one of the two, and nothing else in either file can.
fn spans(source: &str) -> impl Iterator<Item = &str> {
    let mut spans = Vec::new();
    for (open, close) in [('<', '>'), ('{', '}')] {
        let mut rest = source;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + open.len_utf8()..];
            match after.find(close) {
                Some(end) => {
                    spans.push(&after[..end]);
                    rest = &after[end + close.len_utf8()..];
                }
                None => break,
            }
        }
    }
    spans.into_iter()
}

/// The quoted value of `name` inside one span, for either `name="value"` or
/// `name: "value"`.
///
/// Matches the name as a whole word: without that, `href` would be found inside
/// `data-href` and an icon declaration could be read out of a tag that has
/// none.
fn attribute(span: &str, name: &str) -> Option<String> {
    let bytes = span.as_bytes();
    let mut from = 0;

    while let Some(found) = span[from..].find(name) {
        let start = from + found;
        let end = start + name.len();
        from = end;

        let preceded_by_word = start
            .checked_sub(1)
            .is_some_and(|before| bytes[before].is_ascii_alphanumeric() || bytes[before] == b'-' || bytes[before] == b'_');
        if preceded_by_word {
            continue;
        }

        let mut cursor = span[end..].trim_start();
        let Some(rest) = cursor.strip_prefix('=').or_else(|| cursor.strip_prefix(':')) else {
            continue;
        };
        cursor = rest.trim_start();

        let quote = match cursor.chars().next() {
            Some(quote @ ('"' | '\'')) => quote,
            _ => continue,
        };
        let value = &cursor[quote.len_utf8()..];
        if let Some(closing) = value.find(quote) {
            return Some(value[..closing].to_string());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a parent directory");
        }
        std::fs::write(path, contents).expect("a file");
    }

    #[test]
    fn a_project_with_nothing_has_no_icon() {
        let workspace = workspace();
        write(workspace.path(), "README.md", "# hello");

        assert_eq!(resolve(workspace.path()), None);
    }

    #[test]
    fn the_first_well_known_path_wins() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", "i");
        write(workspace.path(), "public/favicon.png", "p");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("favicon.ico"))
        );
    }

    /// The order inside [`CANDIDATES`] is a decision and not an accident, so it
    /// is asserted rather than left to the one pair above.
    #[test]
    fn svg_is_preferred_to_ico_at_the_same_place() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", "i");
        write(workspace.path(), "favicon.svg", "s");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("favicon.svg"))
        );
    }

    #[test]
    fn a_declared_icon_path_beats_every_well_known_one() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", "i");
        write(workspace.path(), "art/brand.png", "b");
        write(workspace.path(), PROJECT_FILE, r#"{"iconPath":"art/brand.png"}"#);

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("art/brand.png"))
        );
    }

    /// A declared path that is not there yet is a project mid-build, not a
    /// project without an icon.
    #[test]
    fn a_declared_icon_path_that_is_missing_falls_through() {
        let workspace = workspace();
        write(workspace.path(), "favicon.ico", "i");
        write(workspace.path(), PROJECT_FILE, r#"{"iconPath":"dist/brand.png"}"#);

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("favicon.ico"))
        );
    }

    #[test]
    fn a_declared_icon_path_may_not_leave_the_project() {
        let workspace = workspace();
        write(workspace.path(), PROJECT_FILE, r#"{"iconPath":"../outside.png"}"#);
        std::fs::write(workspace.path().parent().unwrap().join("outside.png"), "x").ok();

        assert_eq!(resolve(workspace.path()), None);
    }

    #[test]
    fn a_project_file_that_is_not_json_is_survived() {
        let workspace = workspace();
        write(workspace.path(), PROJECT_FILE, "{not json");
        write(workspace.path(), "favicon.png", "p");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("favicon.png"))
        );
    }

    #[test]
    fn an_html_link_names_a_file_under_public() {
        let workspace = workspace();
        write(
            workspace.path(),
            "index.html",
            r#"<!doctype html><link rel="icon" href="/brand.png" /><div id=root></div>"#,
        );
        write(workspace.path(), "public/brand.png", "b");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("public/brand.png"))
        );
    }

    #[test]
    fn an_html_link_also_names_a_file_at_the_root() {
        let workspace = workspace();
        write(
            workspace.path(),
            "index.html",
            r#"<link href="brand.png" rel="shortcut icon">"#,
        );
        write(workspace.path(), "brand.png", "b");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("brand.png"))
        );
    }

    #[test]
    fn a_root_route_declares_its_icon_as_an_object() {
        let workspace = workspace();
        write(
            workspace.path(),
            "src/routes/__root.tsx",
            "export const Route = { head: () => ({ links: [{ rel: 'icon', href: '/icon.svg' }] }) }",
        );
        write(workspace.path(), "public/icon.svg", "s");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("public/icon.svg"))
        );
    }

    #[test]
    fn a_declaration_that_climbs_out_is_not_followed() {
        let workspace = workspace();
        write(
            workspace.path(),
            "index.html",
            r#"<link rel="icon" href="../../secret.png">"#,
        );

        assert_eq!(resolve(workspace.path()), None);
    }

    #[test]
    fn a_query_string_is_not_part_of_the_filename() {
        assert_eq!(
            icon_href(r#"<link rel="icon" href="/favicon.ico?v=4">"#),
            Some("/favicon.ico".to_string())
        );
    }

    #[test]
    fn a_link_that_is_not_an_icon_is_ignored() {
        assert_eq!(
            icon_href(r#"<link rel="stylesheet" href="/app.css">"#),
            None
        );
    }

    /// `data-href` is not `href`, and a stylesheet that carries one must not
    /// become an icon declaration.
    #[test]
    fn a_prefixed_attribute_is_not_the_attribute() {
        assert_eq!(
            icon_href(r#"<link rel="icon" data-href="/wrong.png">"#),
            None
        );
    }

    #[test]
    fn the_first_declaration_of_several_wins() {
        assert_eq!(
            icon_href(
                r#"<link rel="stylesheet" href="/a.css"><link rel="icon" href="/b.png"><link rel="icon" href="/c.png">"#
            ),
            Some("/b.png".to_string())
        );
    }

    /// A directory named `favicon.ico` is not an icon, and statting without
    /// asking what kind of thing was found would serve one.
    #[test]
    fn a_directory_is_not_a_candidate() {
        let workspace = workspace();
        std::fs::create_dir_all(workspace.path().join("favicon.ico")).expect("a directory");
        write(workspace.path(), "favicon.png", "p");

        assert_eq!(
            resolve(workspace.path()),
            Some(workspace.path().join("favicon.png"))
        );
    }
}
