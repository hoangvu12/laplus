//! Comparing a live payload against a captured one, structurally.
//!
//! Values cannot be compared directly — the capture holds another machine's
//! hostname, another checkout's `cwd`, another day's timestamps. What *can* be
//! compared is shape: which keys exist and what JSON type each holds.
//!
//! The interesting part is what happens to the differences. laplus is a
//! hard fork and is expected to diverge, so a difference is not automatically
//! a failure — but an *undeclared* difference is. Every divergence has to be
//! named in the test, with a reason, and a declaration that stops being true
//! fails too. That turns "we diverge here" from a thing someone remembers into
//! a thing the suite enforces.

use std::collections::BTreeSet;

use serde_json::Value;

/// What a structural comparison found.
#[derive(Debug, Default)]
pub struct Differences {
    /// Paths the capture has and the live payload does not.
    pub missing: BTreeSet<String>,
    /// Paths the live payload has and the capture does not.
    pub added: BTreeSet<String>,
    /// Paths present in both but holding different JSON types.
    pub retyped: BTreeSet<String>,
    /// Arrays that could not be compared because one side was empty. Recorded
    /// rather than passed over: an empty array hides its element shape, and
    /// silently skipping it would read as "checked" when it was not.
    pub uncompared: BTreeSet<String>,
}

/// Walk `captured` against `live` and collect every structural difference.
pub fn compare(captured: &Value, live: &Value) -> Differences {
    let mut differences = Differences::default();
    walk(captured, live, "", &mut differences);
    differences
}

fn walk(captured: &Value, live: &Value, path: &str, found: &mut Differences) {
    match (captured, live) {
        (Value::Object(captured_fields), Value::Object(live_fields)) => {
            for (key, captured_value) in captured_fields {
                let child = format!("{path}/{key}");
                match live_fields.get(key) {
                    Some(live_value) => walk(captured_value, live_value, &child, found),
                    None => {
                        found.missing.insert(child);
                    }
                }
            }
            for key in live_fields.keys() {
                if !captured_fields.contains_key(key) {
                    found.added.insert(format!("{path}/{key}"));
                }
            }
        }
        (Value::Array(captured_items), Value::Array(live_items)) => {
            match (captured_items.first(), live_items.first()) {
                (Some(captured_first), Some(live_first)) => {
                    walk(captured_first, live_first, &format!("{path}/0"), found);
                }
                _ => {
                    found.uncompared.insert(format!("{path}[]"));
                }
            }
        }
        _ if json_type(captured) != json_type(live) => {
            found.retyped.insert(format!(
                "{path} ({} in the capture, {} here)",
                json_type(captured),
                json_type(live)
            ));
        }
        _ => {}
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// One declared divergence: a path, and why laplus differs there.
pub struct Declared {
    pub path: &'static str,
    pub because: &'static str,
}

/// Assert a set of found differences is exactly the set that was declared.
///
/// Fails two ways, and both matter: an undeclared difference is drift nobody
/// decided on, and a declaration nothing matches is a note that has outlived
/// the thing it described — usually because a later ticket filled the field in
/// and left the excuse behind.
pub fn assert_declared(kind: &str, found: &BTreeSet<String>, declared: &[Declared]) {
    let declared_paths: BTreeSet<&str> = declared.iter().map(|entry| entry.path).collect();

    // A path is matched by an exact hit, or — for retyped paths, which carry a
    // parenthesised explanation — by prefix.
    let undeclared: Vec<&String> = found
        .iter()
        .filter(|path| {
            !declared_paths
                .iter()
                .any(|declared| path.as_str() == *declared || path.starts_with(declared))
        })
        .collect();

    let unmatched: Vec<String> = declared
        .iter()
        .filter(|entry| {
            !found
                .iter()
                .any(|path| path.as_str() == entry.path || path.starts_with(entry.path))
        })
        .map(|entry| format!("{} — declared because {}", entry.path, entry.because))
        .collect();

    assert!(
        undeclared.is_empty(),
        "undeclared {kind} against the ticket 01 capture: {undeclared:#?}\n\
         Either fix the payload or add the path to the declared list with a reason."
    );
    assert!(
        unmatched.is_empty(),
        "declared {kind} that no longer happens: {unmatched:#?}\n\
         Remove the declaration — the payload has caught up with the capture."
    );
}
