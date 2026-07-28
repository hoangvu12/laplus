//! How this server refuses a method it has not implemented.
//!
//! An error on this wire is `_tag`-discriminated, and the client decodes each
//! one against the union that *that method* declares in
//! `packages/contracts/src/rpc.ts`. A tag outside the union is not a refusal the
//! client can read: `decodeExit` fails, and what reaches the screen is the schema
//! decoder's complaint about the shape of the refusal rather than any statement
//! about the feature. `/settings/diagnostics` showed exactly that — five lines of
//! `Expected { readonly "_tag": "EnvironmentAuthorizationError", … }, got …` —
//! for a page whose real news is that the method behind it does not exist here.
//!
//! [`crate::config`] already reasons about the same rule one layer up, for
//! `ConfigIssue`: _"its `kind` is one of two literals the contract names, not a
//! label: an invented kind fails the client's decode of the whole payload."_
//! `ServerMethodNotImplementedError` is an invented kind in exactly that sense.
//! It is not in the contract at all — no method declares it, so no method can
//! decode it — and answering all thirty unimplemented methods with it made
//! every refusal illegible.
//!
//! So the tag is chosen **per method**, from `REFUSALS`, which is the contract's
//! own list read out. The sentence does not change: what was refused and why is
//! the part of the old refusal that worked, and it is the only part the client
//! renders. `docs/adr/0017` is the decision and its cost.
//!
//! ## Why every entry says the same thing
//!
//! Every one of the sixty-one methods `rpc.ts` declares has
//! `EnvironmentAuthorizationError` in its error union — it is the union of one
//! for thirteen of them, including every page the surface walk found, and the
//! last member of the union for the rest. So the per-method answer happens to be
//! uniform today, and the table below is a column of one value: `Tag` has one
//! variant, because the contract offers one answer.
//!
//! It is still a table rather than a constant, because the thing that has to be
//! checkable is the *pairing*: this method, that tag, and a test that reads the
//! union out of the contract and fails if the two ever part company. The
//! original bug is one method being added without anyone checking its union, and
//! a constant cannot be checked against a union it does not name a method for.
//!
//! The tag is a compromise and worth naming as one. It says "authorization"
//! where the truth is "unimplemented", and the truth is carried in the message
//! instead. The alternative — inventing a tag that says what happened — is what
//! the page was already showing.
//!
//! ## The tag is not inert
//!
//! Borrowing a real tag means borrowing what the client does with it, and for
//! two methods that is not nothing: `session.ts` maps
//! `EnvironmentAuthorizationError` from `server.getConfig` or `server.probe` to
//! `ConnectionBlockedError`, which is a connection refused on *permission* and
//! not retried. Both rows below say so. The consequence is live rather than
//! theoretical and is the first thing `docs/adr/0017` lists.

use serde_json::{json, Value};

#[cfg(test)]
pub(crate) mod contract;

/// The tag a refusal carries, and with it the shape of the payload.
///
/// One variant, because every union in `packages/contracts/src/rpc.ts` contains
/// `EnvironmentAuthorizationError` and nothing else is available to every
/// method. It is an enum rather than a string so that a second tag cannot be
/// added to `REFUSALS` without [`refusal`] failing to compile — the fields a
/// tag's own class declares are not optional, and a required field left out
/// fails the client's decode exactly as a wrong tag does.
#[derive(Debug, Clone, Copy)]
enum Tag {
    EnvironmentAuthorization,
}

impl Tag {
    /// The literal that goes on the wire.
    const fn as_str(self) -> &'static str {
        match self {
            Tag::EnvironmentAuthorization => "EnvironmentAuthorizationError",
        }
    }
}

/// The scope a `Tag::EnvironmentAuthorization` refusal names.
///
/// The field is required by `EnvironmentAuthorizationError` and is one of the
/// eight literals in `packages/contracts/src/auth.ts` — a ninth would fail the
/// decode as surely as a wrong `_tag` would.
///
/// Nothing in the client reads the *scope*: `useEnvironmentQuery` squashes the
/// cause and renders `message`. The **tag** is read, on two methods — see the
/// module docs — so this being cosmetic is not a licence to treat the tag the
/// same way.
///
/// `orchestration:read` rather than something narrower because it is the scope
/// every connected client already holds, so the refusal does not read as a
/// permission the developer could go and grant.
const REFUSAL_SCOPE: &str = "orchestration:read";

/// What every refusal says, before the method name.
///
/// The sentence rather than the tag is now the only stable way to *recognise* a
/// refusal from outside: the tag is whatever the called method declares, and for
/// almost every method that is `EnvironmentAuthorizationError`, which a real
/// authorization failure would also carry. So `tools/ui-driver/surface-walk.mjs`
/// and `surface-actions.mjs` both match on this wording — it is how the surface
/// walk answers "of everything the UI offers, what does nothing?" — and changing
/// it silently blinds both. The tests below and in
/// `tests/socket_handshake.rs` spell it out as a literal on purpose, so that a
/// change here fails rather than agreeing with itself.
const REFUSAL_SENTENCE: &str = "Method not implemented by this server";

/// What a tag the contract does not name is refused with.
///
/// Kept, and correct where it is used: a tag outside `rpc.ts` has no declared
/// union, so there is nothing for this to fail to decode against. It is what
/// `no.such.method` gets, and `fixtures/socket-wire/03-typed-error.ndjson`'s
/// divergence — an `Exit` where the reference server sends a bare `Defect` — is
/// still the whole of the reasoning. See [`crate::rpc::DispatchError::to_error`].
const NOT_IMPLEMENTED: &str = "ServerMethodNotImplementedError";

/// Every method `packages/contracts/src/rpc.ts` declares, and the error tag a
/// refusal of it carries.
///
/// All sixty-one, not just the ones this server has yet to implement: an
/// implemented method never reaches this table, and listing only the gap would
/// mean deleting a row every time one closes — a second place to forget. The
/// order is the contract's own, so the two can be read side by side.
///
/// `refusals::tests::the_table_is_the_contract` reads `rpc.ts` and fails if a
/// method is missing, invented, or paired with a tag its union does not contain.
const REFUSALS: &[(&str, Tag)] = &[
    ("server.upsertKeybinding", Tag::EnvironmentAuthorization),
    ("server.removeKeybinding", Tag::EnvironmentAuthorization),
    // Refused, and the one refusal the client acts on rather than draws:
    // `session.ts` turns this tag into `ConnectionBlockedError`, a connection
    // refused on permission and not retried. Dormant only because this server
    // does not advertise `capabilities.connectionProbe`, so the client probes
    // with `server.getConfig` instead. **Advertising that capability before
    // implementing this method turns every probe into a blocked connection.**
    ("server.probe", Tag::EnvironmentAuthorization),
    // Implemented today, so it does not reach this table — but it is on the
    // same `session.ts` path as `server.probe` above, and a regression that
    // made it refuse would not draw an empty state: it would refuse the
    // connection.
    ("server.getConfig", Tag::EnvironmentAuthorization),
    ("server.refreshProviders", Tag::EnvironmentAuthorization),
    ("server.updateProvider", Tag::EnvironmentAuthorization),
    ("server.updateServer", Tag::EnvironmentAuthorization),
    ("server.getSettings", Tag::EnvironmentAuthorization),
    ("server.updateSettings", Tag::EnvironmentAuthorization),
    ("server.getTraceDiagnostics", Tag::EnvironmentAuthorization),
    ("server.getProcessDiagnostics", Tag::EnvironmentAuthorization),
    ("server.getProcessResourceHistory", Tag::EnvironmentAuthorization),
    ("server.signalProcess", Tag::EnvironmentAuthorization),
    ("projects.searchEntries", Tag::EnvironmentAuthorization),
    ("projects.listEntries", Tag::EnvironmentAuthorization),
    ("projects.readFile", Tag::EnvironmentAuthorization),
    ("projects.writeFile", Tag::EnvironmentAuthorization),
    ("shell.openInEditor", Tag::EnvironmentAuthorization),
    ("filesystem.browse", Tag::EnvironmentAuthorization),
    ("assets.createUrl", Tag::EnvironmentAuthorization),
    ("subscribeVcsStatus", Tag::EnvironmentAuthorization),
    ("vcs.pull", Tag::EnvironmentAuthorization),
    ("vcs.refreshStatus", Tag::EnvironmentAuthorization),
    ("vcs.listRefs", Tag::EnvironmentAuthorization),
    ("vcs.createWorktree", Tag::EnvironmentAuthorization),
    ("vcs.removeWorktree", Tag::EnvironmentAuthorization),
    ("vcs.createRef", Tag::EnvironmentAuthorization),
    ("vcs.switchRef", Tag::EnvironmentAuthorization),
    ("vcs.init", Tag::EnvironmentAuthorization),
    ("review.getDiffPreview", Tag::EnvironmentAuthorization),
    ("terminal.open", Tag::EnvironmentAuthorization),
    ("terminal.attach", Tag::EnvironmentAuthorization),
    ("terminal.write", Tag::EnvironmentAuthorization),
    ("terminal.resize", Tag::EnvironmentAuthorization),
    ("terminal.clear", Tag::EnvironmentAuthorization),
    ("terminal.restart", Tag::EnvironmentAuthorization),
    ("terminal.close", Tag::EnvironmentAuthorization),
    ("preview.open", Tag::EnvironmentAuthorization),
    ("preview.navigate", Tag::EnvironmentAuthorization),
    ("preview.resize", Tag::EnvironmentAuthorization),
    ("preview.refresh", Tag::EnvironmentAuthorization),
    ("preview.close", Tag::EnvironmentAuthorization),
    ("preview.list", Tag::EnvironmentAuthorization),
    ("preview.reportStatus", Tag::EnvironmentAuthorization),
    ("previewAutomation.connect", Tag::EnvironmentAuthorization),
    ("previewAutomation.respond", Tag::EnvironmentAuthorization),
    ("previewAutomation.focusHost", Tag::EnvironmentAuthorization),
    ("subscribePreviewEvents", Tag::EnvironmentAuthorization),
    ("subscribeDiscoveredLocalServers", Tag::EnvironmentAuthorization),
    ("orchestration.dispatchCommand", Tag::EnvironmentAuthorization),
    ("orchestration.getTurnDiff", Tag::EnvironmentAuthorization),
    ("orchestration.getFullThreadDiff", Tag::EnvironmentAuthorization),
    ("orchestration.replayEvents", Tag::EnvironmentAuthorization),
    ("orchestration.getArchivedShellSnapshot", Tag::EnvironmentAuthorization),
    ("orchestration.subscribeShell", Tag::EnvironmentAuthorization),
    ("orchestration.subscribeThread", Tag::EnvironmentAuthorization),
    ("subscribeTerminalEvents", Tag::EnvironmentAuthorization),
    ("subscribeTerminalMetadata", Tag::EnvironmentAuthorization),
    ("subscribeServerConfig", Tag::EnvironmentAuthorization),
    ("subscribeServerLifecycle", Tag::EnvironmentAuthorization),
    ("subscribeAuthAccess", Tag::EnvironmentAuthorization),
];

/// The tag a refusal of `method` must carry, or `None` for a tag the contract
/// does not name.
fn declared_error_tag(method: &str) -> Option<Tag> {
    REFUSALS
        .iter()
        .find(|(declared, _)| *declared == method)
        .map(|(_, tag)| *tag)
}

/// The typed error answering a method this server does not implement.
pub fn refusal(method: &str) -> Value {
    let message = format!("{REFUSAL_SENTENCE}: {method}");
    match declared_error_tag(method) {
        // The method is the contract's, so the tag has to be one it declares —
        // and the tag brings its class's required fields with it. A second
        // variant of [`Tag`] does not compile until it is given its own, which
        // is the check this module exists for, made by the compiler rather than
        // by a test.
        Some(tag @ Tag::EnvironmentAuthorization) => json!({
            "_tag": tag.as_str(),
            "message": message,
            "requiredScope": REFUSAL_SCOPE,
        }),
        // Not the contract's method, so there is no union to answer inside.
        // `method` is the only thing that says which of the sixty-one a
        // developer has mistyped, so it survives into the error.
        None => json!({
            "_tag": NOT_IMPLEMENTED,
            "method": method,
            "message": message,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The pairing, checked against the file it was copied from. A method the
    /// contract has grown, one it has dropped, and a tag whose union does not
    /// contain it all fail here — which is the whole of what stops this
    /// recurring, because the original bug is a method arriving without anyone
    /// reading its union.
    ///
    /// The method sets are compared for **equality**, and that is deliberate
    /// even though it makes a purely additive `upstream` merge red. A contract
    /// method this server has not implemented is refused whether or not it is
    /// listed here, and an unlisted one is refused with
    /// `ServerMethodNotImplementedError` — no union contains it, so the bug the
    /// ticket fixed is back on the screen. The merge that adds a method is
    /// exactly when somebody should read its union, so the red suite is the
    /// point rather than the cost. A subset would let the row be forgotten and
    /// leave the failure to
    /// [`every_refusal_carries_a_tag_the_methods_union_declares`], which catches
    /// the same thing one test later and says less about why.
    #[test]
    fn the_table_is_the_contract() {
        let declared = contract::declared_unions();

        let ours: BTreeSet<&str> = REFUSALS.iter().map(|(method, _)| *method).collect();
        assert_eq!(ours.len(), REFUSALS.len(), "a method is listed twice");

        let theirs: BTreeSet<&str> = declared.keys().map(String::as_str).collect();
        assert_eq!(
            ours, theirs,
            "the table and packages/contracts/src/rpc.ts name different methods: \
             add a row for one the contract has grown — unlisted, it is refused \
             with a tag no union declares — and drop one it no longer names"
        );

        for (method, tag) in REFUSALS {
            contract::assert_declares(method, tag.as_str(), &declared[*method]);
        }
    }

    /// Every refusal this server can send, checked the same way — the table is
    /// what [`refusal`] reads, but the shape it builds is separate code and a
    /// tag lost between the two would be invisible above.
    #[test]
    fn every_refusal_carries_a_tag_the_methods_union_declares() {
        let declared = contract::declared_unions();

        for (method, union) in &declared {
            let error = refusal(method);
            contract::assert_declares(method, error["_tag"].as_str().expect("a tag"), union);
            assert!(
                error["message"].as_str().is_some_and(|text| text.contains(method)),
                "{method}: a refusal that does not say what was refused: {error}"
            );
        }
    }

    /// The pages the surface walk found, written out rather than derived: this
    /// is the bug the ticket is about, and a test that computes both sides
    /// would pass against a table that had lost every one.
    ///
    /// There were four. `/settings/source-control` was the fourth, and ticket
    /// 71 deleted the page and `server.discoverSourceControl` with it — a
    /// method the contract no longer declares is refused with
    /// `ServerMethodNotImplementedError`, which is right, because there is no
    /// union left for a borrowed tag to be legible inside.
    #[test]
    fn the_pages_that_showed_a_decoder_error_are_answered() {
        for method in [
            "server.getProcessDiagnostics",
            "server.getProcessResourceHistory",
            "server.getTraceDiagnostics",
        ] {
            let error = refusal(method);
            assert_eq!(error["_tag"], "EnvironmentAuthorizationError", "{method}");
            assert_eq!(error["requiredScope"], "orchestration:read", "{method}");
            assert_eq!(
                error["message"],
                format!("Method not implemented by this server: {method}"),
                "{method}"
            );
        }
    }

    /// A tag outside the contract keeps the old answer, and keeps naming the
    /// method: nothing declares an error union for it, so there is nothing it
    /// could fail to decode against.
    #[test]
    fn a_tag_the_contract_does_not_name_is_still_not_implemented() {
        let error = refusal("no.such.method");
        assert_eq!(error["_tag"], "ServerMethodNotImplementedError");
        assert_eq!(error["method"], "no.such.method");
        assert_eq!(
            error["message"],
            "Method not implemented by this server: no.such.method"
        );
    }
}

