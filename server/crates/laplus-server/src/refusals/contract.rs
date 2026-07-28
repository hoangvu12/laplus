//! The contract, read rather than remembered.
//!
//! A hand-written table cannot be trusted against a file in another language
//! that nothing here compiles, so [`super`]'s tests do not trust it: they parse
//! `packages/contracts/src/` and compare. This is a small parser rather than a
//! TypeScript one because the shapes it has to read are three, all of them
//! literal — an `as const` map of method names, an `Rpc.make` options object,
//! and an error class or a union of them.
//!
//! It lives in its own file because it changes for its own reason. The module
//! above changes when a refusal's shape changes; this changes when upstream
//! writes the contract differently — a test-harness concern with no bearing on
//! what reaches the wire.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn directory() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../packages/contracts/src")
}

fn read(file: &str) -> String {
    let path = directory().join(file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("the contract at {} is unreadable: {error}", path.display()))
}

/// Every `.ts` source in the contract, concatenated into one haystack.
///
/// Which file an error class is declared in is not information this needs —
/// a union member is named by a bare identifier and the identifiers are
/// unique across the package — and looking it up through the import list
/// would be a second parser for no answer. `rpc.ts` is in here *and* read
/// separately by [`declared_unions`], which needs it alone to split on its
/// `export const` boundaries.
fn all_sources() -> String {
    let mut sources = Vec::new();
    for entry in std::fs::read_dir(directory()).expect("the contract directory") {
        let path = entry.expect("a directory entry").path();
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if name.ends_with(".ts") && !name.ends_with(".test.ts") {
            sources.push(std::fs::read_to_string(&path).expect("a contract source"));
        }
    }
    sources.join("\n")
}

/// An `as const` object of string literals, as `WS_METHODS` and
/// `ORCHESTRATION_WS_METHODS` both are.
fn literal_map(source: &str, name: &str) -> BTreeMap<String, String> {
    let opening = format!("export const {name} = {{");
    let start = source
        .find(&opening)
        .unwrap_or_else(|| panic!("{name} is not in the contract"));
    let end = source[start..]
        .find("} as const;")
        .unwrap_or_else(|| panic!("{name} does not close"));

    let mut entries = BTreeMap::new();
    for line in source[start..start + end].lines() {
        let line = line.trim();
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = quoted(rest) else { continue };
        if key.chars().all(|character| character.is_alphanumeric() || character == '_') {
            entries.insert(key.to_string(), value);
        }
    }
    entries
}

/// The identifiers in a comma-separated list, which is how the contract writes
/// both a `Schema.Union([…])` body and an `Rpc.make`'s `error:` field.
fn identifiers(list: &str) -> Vec<&str> {
    list.split(',')
        .map(str::trim)
        .filter(|member| !member.is_empty())
        .collect()
}

/// The first double-quoted string in `text`.
fn quoted(text: &str) -> Option<String> {
    let opening = text.find('"')?;
    let rest = &text[opening + 1..];
    let closing = rest.find('"')?;
    Some(rest[..closing].to_string())
}

/// The `_tag` values an error identifier stands for.
///
/// Two shapes, because the contract writes errors two ways: a class, whose
/// tag is the string it is constructed with and is *not* always its own
/// name — `KeybindingsConfigError` is tagged `KeybindingsConfigParseError`
/// — and a union of other identifiers, which nests.
fn tags_of(source: &str, identifier: &str, seen: &mut BTreeSet<String>) -> BTreeSet<String> {
    if !seen.insert(identifier.to_string()) {
        return BTreeSet::new();
    }

    let class = format!("export class {identifier} extends");
    if let Some(start) = source.find(&class) {
        let arguments = source[start..]
            .find("()(")
            .unwrap_or_else(|| panic!("{identifier} is not a tagged error class"));
        let tag = quoted(&source[start + arguments..])
            .unwrap_or_else(|| panic!("{identifier} names no tag"));
        return BTreeSet::from([tag]);
    }

    let union = format!("export const {identifier} = Schema.Union([");
    if let Some(start) = source.find(&union) {
        let body = &source[start + union.len()..];
        let end = body.find(']').unwrap_or_else(|| panic!("{identifier} does not close"));
        return identifiers(&body[..end])
            .into_iter()
            .flat_map(|member| tags_of(source, member, seen))
            .collect();
    }

    panic!("{identifier} is neither an error class nor a union of them");
}

/// Every method `rpc.ts` declares, and the `_tag` values its error union
/// accepts.
pub fn declared_unions() -> BTreeMap<String, BTreeSet<String>> {
    let rpc = read("rpc.ts");
    let source = all_sources();
    let ws = literal_map(&rpc, "WS_METHODS");
    let orchestration = literal_map(&read("orchestration.ts"), "ORCHESTRATION_WS_METHODS");

    let mut declared = BTreeMap::new();
    // Each `export const Ws…Rpc = Rpc.make(…)` runs to the next `export
    // const`, and inside one the only `WS_METHODS.` is the method it names.
    for definition in rpc.split("\nexport const ").skip(1) {
        if !definition.contains("Rpc.make(") {
            continue;
        }
        let method = method_of(definition, &ws, &orchestration);
        let mut seen = BTreeSet::new();
        let tags = union_members(definition)
            .iter()
            .flat_map(|member| tags_of(&source, member, &mut seen))
            .collect();
        declared.insert(method, tags);
    }
    assert!(!declared.is_empty(), "rpc.ts declares no methods");
    declared
}

/// The one assertion every check of a refusal makes: `method` is refused with
/// `tag`, and `tag` is a member of the union `rpc.ts` declares for it.
///
/// Three tests ask this — of the table, of the payload
/// [`super::refusal`] builds, and of what dispatch actually answers — and the
/// sentence they fail with is the same sentence in each case, so it is written
/// once here.
pub fn assert_declares(method: &str, tag: &str, union: &BTreeSet<String>) {
    assert!(
        union.contains(tag),
        "{method} is refused with {tag}, which its union does not contain: {union:?}"
    );
}

/// The string behind the `WS_METHODS.x` or `ORCHESTRATION_WS_METHODS.x` an
/// `Rpc.make` is given as its first argument.
fn method_of(
    definition: &str,
    ws: &BTreeMap<String, String>,
    orchestration: &BTreeMap<String, String>,
) -> String {
    for (prefix, names) in [
        ("ORCHESTRATION_WS_METHODS.", orchestration),
        ("WS_METHODS.", ws),
    ] {
        let Some(start) = definition.find(prefix) else {
            continue;
        };
        let rest = &definition[start + prefix.len()..];
        let key: String = rest
            .chars()
            .take_while(|character| character.is_alphanumeric() || *character == '_')
            .collect();
        return names
            .get(&key)
            .unwrap_or_else(|| panic!("{prefix}{key} names no method"))
            .clone();
    }
    panic!("an Rpc.make that names no method: {definition}");
}

/// The identifiers in an `Rpc.make`'s `error:` field, which is either one
/// of them or a `Schema.Union([…])` of them, and is written on one line.
fn union_members(definition: &str) -> Vec<String> {
    let line = definition
        .lines()
        .find_map(|line| line.trim().strip_prefix("error:"))
        .unwrap_or_else(|| panic!("an Rpc.make that declares no error: {definition}"))
        .trim()
        .trim_end_matches(',');

    let inner = line
        .strip_prefix("Schema.Union([")
        .and_then(|rest| rest.strip_suffix("])"))
        .unwrap_or(line);
    identifiers(inner).into_iter().map(str::to_string).collect()
}
