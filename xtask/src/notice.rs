//! Whether the artifact still carries upstream's licence.
//!
//! Ticket 24's seventh criterion, and the one that is a legal obligation rather
//! than a measurement. Upstream's UI is MIT, and MIT asks that its copyright
//! and permission notice "be included in all copies or substantial portions of
//! the Software". Four fifths of this artifact by size *is* that software, so
//! the obligation is not a formality.
//!
//! `bundle.copyright` — a one-line string in the executable's version
//! resource — is not that notice. It names the holder and omits the permission
//! text, which is the half MIT actually requires. So the notice ships as a
//! file, and this is the check that it still does.

/// The file that carries it.
pub const NOTICE: &str = "THIRD_PARTY_NOTICES.md";

/// What is missing, if anything is.
#[derive(Debug, PartialEq, Eq)]
pub enum Missing {
    /// The notice file no longer carries upstream's copyright line.
    Copyright,
    /// It no longer carries the permission text, which is the part MIT is
    /// actually about.
    Permission,
    /// The installer would no longer show it.
    InstallerPage,
    /// It would no longer land beside the installed application.
    InstalledFile,
}

/// Read the notice and the bundle configuration together, because either one
/// alone can be right while the artifact carries nothing.
pub fn retained(config: &str, notice: &str) -> Result<(), Missing> {
    if !notice.contains("Copyright (c) 2026 T3 Tools Inc.") {
        return Err(Missing::Copyright);
    }
    if !notice.contains("The above copyright notice and this permission notice shall be included")
    {
        return Err(Missing::Permission);
    }

    // Read as text rather than as JSON, because what is being asked is
    // narrow — does this file name the notice in these two places — and a JSON
    // parser is a dependency this crate has no other use for.
    if !names_notice(config, "licenseFile") {
        return Err(Missing::InstallerPage);
    }
    if !names_notice(config, "resources") {
        return Err(Missing::InstalledFile);
    }

    Ok(())
}

/// Whether the value of `key` in the configuration mentions the notice file.
fn names_notice(config: &str, key: &str) -> bool {
    let Some(after) = config.split_once(&format!("\"{key}\"")).map(|(_, rest)| rest) else {
        return false;
    };
    // A JSON value ends at the next key or the end of its object; either way
    // the notice's name has to appear before the value after this one starts.
    let value = after.split(",\n").next().unwrap_or(after);
    value.contains(NOTICE)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"{
      "bundle": {
        "copyright": "laplus. UI derived from t3code, Copyright (c) T3 Tools, Inc., MIT licence.",
        "licenseFile": "../../THIRD_PARTY_NOTICES.md",
        "resources": { "../../THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md" }
      }
    }"#;

    const NOTICE_TEXT: &str = "Copyright (c) 2026 T3 Tools Inc.\n\n\
        Permission is hereby granted, free of charge, to any person obtaining a copy\n\
        of this software ...\n\n\
        The above copyright notice and this permission notice shall be included in all\n\
        copies or substantial portions of the Software.\n";

    #[test]
    fn a_notice_that_ships_both_ways_is_retained() {
        assert_eq!(retained(CONFIG, NOTICE_TEXT), Ok(()));
    }

    /// The failure that looks fine: `bundle.copyright` is still there, the file
    /// is still in the repository, and the artifact carries neither.
    #[test]
    fn a_notice_the_bundle_does_not_ship_is_not_retained() {
        let unshipped = CONFIG.replace("\"licenseFile\": \"../../THIRD_PARTY_NOTICES.md\",", "");
        assert_eq!(
            retained(&unshipped, NOTICE_TEXT),
            Err(Missing::InstallerPage)
        );

        let uninstalled = CONFIG.replace(
            "\"resources\": { \"../../THIRD_PARTY_NOTICES.md\": \"THIRD_PARTY_NOTICES.md\" }",
            "\"resources\": {}",
        );
        assert_eq!(
            retained(&uninstalled, NOTICE_TEXT),
            Err(Missing::InstalledFile)
        );
    }

    /// The three cases above are about the rule. This one is about *this*
    /// repository, and it is the one that will actually catch something: the
    /// build refuses to bundle without the notice, but a build is three minutes
    /// and a release, and `cargo test` is where someone finds out that an edit
    /// to `tauri.conf.json` dropped it.
    #[test]
    fn the_configuration_this_repository_ships_retains_it() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask is in the workspace");

        let config = std::fs::read_to_string(root.join("crates/laplus-shell/tauri.conf.json"))
            .expect("the shell's bundle configuration");
        let text = std::fs::read_to_string(root.join(NOTICE)).expect("the notice");

        assert_eq!(retained(&config, &text), Ok(()));
    }

    /// A notice trimmed to its copyright line is the mistake `bundle.copyright`
    /// already makes, made again in a file.
    #[test]
    fn a_copyright_line_without_the_permission_text_is_not_the_notice() {
        assert_eq!(
            retained(CONFIG, "Copyright (c) 2026 T3 Tools Inc.\n"),
            Err(Missing::Permission)
        );
        assert_eq!(
            retained(CONFIG, "Permission is hereby granted, free of charge"),
            Err(Missing::Copyright)
        );
    }
}
