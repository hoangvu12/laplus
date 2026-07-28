//! How much Rust the server is, split so the answer means something.

use std::fs;
use std::path::Path;

/// The spec's "if the Rust server grows past roughly 20K LOC, that is the
/// signal to stop and re-evaluate" — a signal about scope creep back toward
/// parity, which is why [`Breakdown::production`] is the figure set against it
/// and not [`Breakdown::total`].
pub const SIGNAL: usize = 20_000;

/// What a Rust source file's lines are.
#[derive(Debug, PartialEq, Eq)]
pub struct Breakdown {
    pub total: usize,
    pub comment: usize,
    pub test: usize,
    pub blank: usize,
    /// The scan ended where a file is allowed to end: outside every comment,
    /// literal and `#[cfg(test)]` region. False means the numbers beside it are
    /// not to be believed.
    pub balanced: bool,
}

impl Default for Breakdown {
    fn default() -> Self {
        Self {
            total: 0,
            comment: 0,
            test: 0,
            blank: 0,
            balanced: true,
        }
    }
}

impl Breakdown {
    /// Everything that is not a comment, not a test and not empty.
    pub fn production(&self) -> usize {
        self.total - self.comment - self.test - self.blank
    }
}

impl std::ops::AddAssign for Breakdown {
    fn add_assign(&mut self, other: Self) {
        self.total += other.total;
        self.comment += other.comment;
        self.test += other.test;
        self.blank += other.blank;
        self.balanced &= other.balanced;
    }
}

/// Classify every `.rs` file under a directory, together.
pub fn breakdown_tree(root: &Path) -> std::io::Result<Breakdown> {
    let mut measured = Breakdown::default();

    crate::tree::walk(root, &mut |path| {
        if path.extension().is_some_and(|extension| extension == "rs") {
            measured += breakdown(&fs::read_to_string(path)?);
        }
        Ok(())
    })?;

    Ok(measured)
}

/// Classify one file's lines.
pub fn breakdown(source: &str) -> Breakdown {
    let mut measured = Breakdown::default();
    let mut scan = Scan::default();
    let mut region: Option<Region> = None;

    for line in source.lines() {
        measured.total += 1;
        let read = scan.line(line);

        if let Some(open) = &mut region {
            // The attribute's line is already counted; this one is the item it
            // applies to, or more of it.
            measured.test += 1;
            if open.closed_by(&read) {
                region = None;
            }
            continue;
        }

        if read.opens_test_region {
            measured.test += 1;
            // The attribute's own line is fed to the region it opens, because
            // the item can be on it: `#[cfg(test)] mod tests {` opens a block
            // here, and `#[cfg(test)] use std::sync::Mutex;` opens and closes
            // one. A bare `#[cfg(test)]` line has no braces and no semicolon,
            // so this does nothing to it.
            let mut opened = Region::default();
            if !opened.closed_by(&read) {
                region = Some(opened);
            }
            continue;
        }

        match read.kind {
            Kind::Blank => measured.blank += 1,
            Kind::Comment => measured.comment += 1,
            Kind::Code => {}
        }
    }

    measured.balanced = scan.neutral() && region.is_none();
    measured
}

/// A `#[cfg(test)]` region, from the attribute to the end of the item under it.
///
/// Two shapes, and the difference is whether the item has a body. `mod tests {
/// … }` is over when its braces balance; `use std::sync::Mutex;` has no braces
/// at all and is over at the semicolon. Tracking both is what stops a plain
/// `#[cfg(test)] use` from swallowing the rest of the file.
///
/// A whole module on one line — `#[cfg(test)] mod tests { fn it() {} }` —
/// satisfies neither: its braces net to zero without ever going positive, and
/// it ends on `}` rather than `;`, so the region never closes and the file ends
/// unbalanced. That reports *no* line count rather than a wrong one, which is
/// the direction to fail in, and no such line exists in this repository.
#[derive(Default)]
struct Region {
    depth: i32,
    entered: bool,
}

impl Region {
    fn closed_by(&mut self, read: &Read) -> bool {
        self.depth += read.braces;
        if self.depth > 0 {
            self.entered = true;
            return false;
        }
        if self.entered {
            return true;
        }
        // No block has opened yet, so this is the bodiless form and it ends
        // where the statement does.
        read.ends_statement
    }
}

/// What one line turned out to be.
#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Blank,
    Comment,
    Code,
}

/// One line, as the scanner read it.
struct Read {
    kind: Kind,
    /// `{` minus `}`, counting only the ones that are syntax.
    braces: i32,
    /// The line's last code byte is a `;`.
    ends_statement: bool,
    /// The line begins with a `#[cfg(…test…)]` attribute. The item it applies
    /// to may be on this line or the next, which is why the line's own braces
    /// and semicolon are read either way.
    opens_test_region: bool,
}

/// Enough of a Rust lexer to tell prose from code, and no more.
///
/// The naive version of this — `trim().starts_with("//")` — is wrong in both
/// directions on this codebase, and both directions move the headline number.
/// It calls `let s = "// not a comment";` prose, and it calls every line of a
/// block comment code. What it takes to be right is knowing where string
/// literals and comments begin and end, which is what this is: a byte scan that
/// carries its state across lines, because block comments and raw strings both
/// do.
///
/// It is deliberately not a parser. Rust's grammar is not needed to count
/// lines; its *literals* are, because they are where a `//` or a `{` can appear
/// and mean nothing.
#[derive(Default)]
struct Scan {
    /// Nesting depth of `/* */`, which nests in Rust.
    block: usize,
    /// Inside an ordinary `"…"`, which can span lines by escaping the newline.
    string: bool,
    /// Inside `r#"…"#`, with the number of hashes needed to close it.
    raw: Option<usize>,
}

impl Scan {
    /// Nothing is open — where a well-formed file ends.
    fn neutral(&self) -> bool {
        self.block == 0 && !self.string && self.raw.is_none()
    }

    fn line(&mut self, line: &str) -> Read {
        let bytes = line.as_bytes();
        let mut at = 0;
        let mut code = false;
        let mut comment = false;
        let mut braces = 0;
        let mut last_code_byte = None;

        while at < bytes.len() {
            if let Some(hashes) = self.raw {
                if bytes[at] == b'"' && closes_raw(&bytes[at + 1..], hashes) {
                    self.raw = None;
                    at += 1 + hashes;
                } else {
                    at += 1;
                }
                code = true;
                continue;
            }

            if self.string {
                code = true;
                match bytes[at] {
                    b'\\' => at += 2,
                    b'"' => {
                        self.string = false;
                        at += 1;
                    }
                    _ => at += 1,
                }
                continue;
            }

            if self.block > 0 {
                comment = true;
                if bytes[at..].starts_with(b"/*") {
                    self.block += 1;
                    at += 2;
                } else if bytes[at..].starts_with(b"*/") {
                    self.block -= 1;
                    at += 2;
                } else {
                    at += 1;
                }
                continue;
            }

            // Ordinary code.
            if bytes[at..].starts_with(b"//") {
                comment = true;
                break;
            }
            if bytes[at..].starts_with(b"/*") {
                comment = true;
                self.block = 1;
                at += 2;
                continue;
            }
            if let Some(hashes) = opens_raw(bytes, at) {
                code = true;
                self.raw = Some(hashes);
                at += 2 + hashes;
                continue;
            }
            if bytes[at] == b'"' {
                code = true;
                self.string = true;
                at += 1;
                continue;
            }
            if bytes[at] == b'\'' {
                if let Some(end) = char_literal_end(bytes, at) {
                    code = true;
                    at = end;
                    continue;
                }
            }
            match bytes[at] {
                b'{' => braces += 1,
                b'}' => braces -= 1,
                _ => {}
            }
            if !bytes[at].is_ascii_whitespace() {
                code = true;
                last_code_byte = Some(bytes[at]);
            }
            at += 1;
        }

        let kind = if code {
            Kind::Code
        } else if comment {
            Kind::Comment
        } else {
            Kind::Blank
        };

        Read {
            opens_test_region: kind == Kind::Code && opens_test_region(line.trim()),
            kind,
            braces,
            ends_statement: last_code_byte == Some(b';'),
        }
    }
}

/// Whether a line begins a `#[cfg(test)]` item.
///
/// Wider than the literal string, because every form this misses fails the same
/// dangerous way: the test module is counted as **production code**, which is
/// the one number this whole crate exists to report. Nothing detects that — the
/// braces still balance, so [`Breakdown::balanced`] stays true and the figure is
/// simply wrong. All 33 occurrences in the server today are the bare form; that
/// is a fact about today, not a guarantee.
///
/// `not(test)` is excluded deliberately, and it is the reason this is not just a
/// substring search: `#[cfg(not(test))]` marks code that ships.
fn opens_test_region(trimmed: &str) -> bool {
    let Some(predicate) = trimmed.strip_prefix("#[cfg(") else {
        return false;
    };
    let Some(end) = predicate.find(")]") else {
        return false;
    };

    let predicate = &predicate[..end];
    predicate.contains("test") && !predicate.contains("not(")
}

/// The hash count of a raw string starting here, if one does — where the string
/// itself then begins `2 + hashes` bytes along, at `r`, its hashes and its
/// quote.
///
/// `r"…"`, `r#"…"#` and the `b`-prefixed byte-string forms. The check that the
/// `r` is not just the last letter of an identifier is what keeps `for"` — were
/// such a thing written — from opening a string.
fn opens_raw(bytes: &[u8], at: usize) -> Option<usize> {
    let mut start = at;
    if bytes[start] == b'b' {
        start += 1;
    }
    if bytes.get(start) != Some(&b'r') {
        return None;
    }
    let preceding = at.checked_sub(1).map(|before| bytes[before]);
    if preceding.is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return None;
    }

    let hashes = bytes[start + 1..]
        .iter()
        .take_while(|byte| **byte == b'#')
        .count();
    (bytes.get(start + 1 + hashes) == Some(&b'"')).then_some(hashes)
}

fn closes_raw(after_quote: &[u8], hashes: usize) -> bool {
    after_quote.len() >= hashes && after_quote[..hashes].iter().all(|byte| *byte == b'#')
}

/// Where a character literal beginning at `at` ends, or `None` if this `'` is a
/// lifetime.
///
/// Worth the distinction because `'{'` and `'}'` are how a brace hides from a
/// brace count, and `&'a` is everywhere in this codebase.
fn char_literal_end(bytes: &[u8], at: usize) -> Option<usize> {
    if bytes.get(at + 1) == Some(&b'\\') {
        // `'\n'`, `'\u{1b}'` — run to the closing quote.
        let end = bytes[at + 2..].iter().position(|byte| *byte == b'\'')?;
        return Some(at + 3 + end);
    }
    (bytes.get(at + 2) == Some(&b'\'')).then_some(at + 3)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cases here are about the rule; this one is about the 32,000 lines
    /// the rule is actually run against.
    ///
    /// Without it the self-check only ever fires inside `cargo xtask release`,
    /// which is a three-minute build someone runs before shipping — so a
    /// construct that defeated the scanner would be found by the person least
    /// able to stop and deal with it. Here it is found by `cargo test`.
    ///
    /// The bounds are deliberately loose: this asserts the scan kept its place
    /// and the split is sane, not that the server is any particular size.
    /// Pinning the real figure would mean editing this test every time someone
    /// wrote a line of Rust.
    #[test]
    fn the_real_server_classifies_cleanly() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask is in the workspace")
            .join("crates/laplus-server/src");

        let measured = breakdown_tree(&source).expect("the server's sources are readable");

        assert!(measured.balanced, "the scan lost its place: {measured:?}");
        assert!(measured.total > 10_000, "that is not the whole server: {measured:?}");
        assert!(
            measured.test > measured.total / 5,
            "a fifth of this server is not its own unit tests? {measured:?}"
        );
        assert!(
            measured.production() > 1_000 && measured.production() < measured.total,
            "production code is neither nothing nor everything: {measured:?}"
        );
    }

    /// The whole risk of measuring this way. A scanner that loses its place
    /// does not fail — it silently reports a different number, and this number
    /// is the one the project is judged by. A file cannot end inside a comment,
    /// a string or a test module, so if the scan thinks it did, the scan is
    /// wrong and has to say so.
    #[test]
    fn a_scan_that_lost_its_place_says_so_rather_than_reporting_a_number() {
        assert!(breakdown("fn main() {}\n").balanced);
        assert!(!breakdown("/* never closed\nfn main() {}\n").balanced);
        assert!(!breakdown("let s = \"never closed;\n").balanced);
        assert!(
            !breakdown("#[cfg(test)]\nmod tests {\n    fn it() {}\n").balanced,
            "a test module left open would eat the rest of the crate"
        );
    }

    /// The measurement ticket 24 exists to get right: this server is a third
    /// unit tests by line, and counting them as scope is what turns ~12K into a
    /// false alarm against the spec's 20K signal.
    ///
    /// A test module's own prose and blank lines are test lines too — the
    /// question the number answers is "how much of this file is not the
    /// product", and a comment inside `mod tests` is not the product either.
    #[test]
    fn a_cfg_test_module_is_test_code_all_the_way_down() {
        let measured = breakdown(
            "fn shipped() {}\n\
             \n\
             #[cfg(test)]\n\
             mod tests {\n\
                 // a note about the test\n\
                 \n\
                 #[test]\n\
                 fn it_works() {}\n\
             }\n\
             \n\
             fn also_shipped() {}\n",
        );

        assert_eq!(measured.total, 11);
        assert_eq!(measured.test, 7);
        assert_eq!(measured.comment, 0, "the note is inside the test module");
        assert_eq!(measured.blank, 2, "the third blank is inside it too");
        assert_eq!(measured.production(), 2);
    }

    /// A brace in a string literal is how a test module could appear to end
    /// halfway through, silently handing the rest of the file back to
    /// production. This codebase's tests are full of JSON.
    #[test]
    fn a_brace_inside_a_literal_does_not_end_the_test_module() {
        let measured = breakdown(
            "#[cfg(test)]\n\
             mod tests {\n\
                 fn fixture() -> &'static str {\n\
                     r#\"{\"method\": \"ping\"}\"#\n\
                 }\n\
                 fn brace() -> char {\n\
                     '}'\n\
                 }\n\
             }\n\
             fn shipped() {}\n",
        );

        assert_eq!(measured.test, 9);
        assert_eq!(measured.production(), 1);
    }

    /// Every `#[cfg(test)]` in this server today is bare and on its own line,
    /// and none of the other spellings would be *noticed* if they appeared —
    /// braces still balance, so nothing is unbalanced and the test module is
    /// simply counted as production code. The failure is silent and lands
    /// directly on the headline figure, so the shapes are pinned here.
    #[test]
    fn the_other_spellings_of_cfg_test_are_test_code_too() {
        let same_line = breakdown("#[cfg(test)] mod tests {\n    fn it() {}\n}\nfn shipped() {}\n");
        assert_eq!(same_line.test, 3, "the module opens on the attribute's line");
        assert_eq!(same_line.production(), 1);

        let conditional = breakdown(
            "#[cfg(all(test, feature = \"slow\"))]\nmod tests {\n}\nfn shipped() {}\n",
        );
        assert_eq!(conditional.test, 3);
        assert_eq!(conditional.production(), 1);

        let one_liner = breakdown("#[cfg(test)] use std::sync::Mutex;\nfn shipped() {}\n");
        assert_eq!(one_liner.test, 1, "the item ends on the attribute's line");
        assert_eq!(one_liner.production(), 1);
    }

    /// The inverse, and the reason the check above is not a substring search:
    /// `#[cfg(not(test))]` marks code that ships.
    #[test]
    fn cfg_not_test_is_production_code() {
        let measured = breakdown("#[cfg(not(test))]\nfn shipped() {}\nfn also() {}\n");

        assert_eq!(measured.test, 0);
        assert_eq!(measured.production(), 3);
    }

    /// `#[cfg(test)]` is also written on plain items, where there is no block
    /// to close and the statement's own end is the end of the region.
    #[test]
    fn a_cfg_test_item_with_no_block_ends_at_its_statement() {
        let measured = breakdown(
            "#[cfg(test)]\n\
             use std::sync::Mutex;\n\
             fn shipped() {}\n",
        );

        assert_eq!(measured.test, 2);
        assert_eq!(measured.production(), 1);
    }

    /// The distinction that decides the headline number, since this repository
    /// writes a great deal of prose into its source and none of it is scope.
    #[test]
    fn a_line_is_a_comment_only_when_that_is_all_it_is() {
        let measured = breakdown(
            "//! A module.\n\
             /// An item.\n\
             // A note.\n\
             fn main() {} // explaining itself\n",
        );

        assert_eq!(measured.total, 4);
        assert_eq!(measured.comment, 3);
        assert_eq!(measured.production(), 1);
    }

    /// Rust's block comments nest, and this repository's doc comments quote
    /// code, so a `//` inside prose must not open anything.
    #[test]
    fn a_block_comment_runs_until_it_closes() {
        let measured = breakdown(
            "let a = 1;\n\
             /* opening\n\
                /* nested */\n\
                still inside */\n\
             let b = 2;\n",
        );

        assert_eq!(measured.comment, 3);
        assert_eq!(measured.production(), 2);
    }

    #[test]
    fn an_empty_line_is_not_production_code() {
        let measured = breakdown("fn main() {}\n\n");

        assert_eq!(measured.total, 2);
        assert_eq!(measured.blank, 1);
        assert_eq!(measured.production(), 1);
    }
}
