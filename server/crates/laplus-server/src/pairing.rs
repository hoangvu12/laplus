//! Pair codes, sessions and socket tickets: the credentials themselves.
//!
//! This module owns what a credential *is* — how it is generated, how long it
//! lives, and what it carries. It owns no storage: [`crate::store`] is still
//! the only file that speaks SQL, and it takes these types the same way it
//! takes a [`crate::projects::Project`]. And it owns no policy about the socket
//! upgrade, which is [`crate::auth`]'s.
//!
//! ## The chain
//!
//! A phone reaching this machine through a tunnel walks four steps, and each
//! one narrows what it holds:
//!
//! 1. The user mints a **pair code** in Settings on the PC. Twelve characters,
//!    five minutes, single use, read off one screen and typed into another.
//! 2. `POST /oauth/token` trades that code for a **session token** — a bearer
//!    good for thirty days, which is the thing the phone actually keeps.
//! 3. `POST /api/auth/websocket-ticket` trades the bearer for a **ticket**,
//!    good for five minutes and one upgrade. This step exists because the
//!    browser's WebSocket API cannot set a request header, so the credential
//!    has to ride in the query string — and a thirty-day credential in a query
//!    string ends up in a log.
//! 4. `GET /ws?wsTicket=…` opens the socket.
//!
//! ## Randomness
//!
//! Every credential here comes from [`getrandom`], which is the operating
//! system's own generator. The two sources already in this tree are deliberately
//! not used: `RandomState` (see [`crate::auth::trace_id`]) is a hash seed, and
//! SQLite's `randomblob()` is seeded on Windows from the system time, the
//! process id and the tick count. Both are fine for a correlation id and
//! neither is fine for something that keeps a stranger out of the user's shell.
//!
//! ## Scopes are recorded and not enforced
//!
//! **This is a decision, not an oversight.** The contract has eight scopes and
//! the UI offers them when minting a code, so they are carried end to end and
//! reported back. Nothing gates on them, because this server has no
//! per-method authorization to gate *with* — every RPC it answers, it answers
//! to any connection. Ticket 73 puts scope enforcement out of scope explicitly.
//! A reader who adds it should start at [`Grant::scopes`] and
//! [`crate::rpc`], and should expect the work to be in the latter.

use std::fmt;

/// The pair-code alphabet, from the reference server's `PairingGrantStore`.
///
/// Thirty-two characters with `0`/`O` and `1`/`I`/`L` left out, because this is
/// the one credential in the chain a human reads off one screen and types into
/// another.
const PAIRING_CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Twelve characters — 32^12, about 60 bits.
const PAIRING_CODE_LENGTH: usize = 12;

/// The largest byte value that maps onto the alphabet without bias: the
/// alphabet's length times how many whole times it fits into 256.
///
/// **With this alphabet it is 256, so nothing is ever rejected** — 32 divides
/// 256 exactly, which is what makes a bare `% 32` uniform already. The check is
/// kept anyway, and kept correct, because it is the alphabet that makes it
/// unnecessary rather than the arithmetic: shorten
/// [`PAIRING_CODE_ALPHABET`] by one character and a naive `%` starts favouring
/// its first letters, silently, in a way no test of a single code would catch.
const PAIRING_CODE_REJECTION_LIMIT: usize = (256 / PAIRING_CODE_ALPHABET.len()) * PAIRING_CODE_ALPHABET.len();

/// How long a minted pair code stays good for. The reference server's five
/// minutes, and for its reason: this is a user-facing code being carried between
/// two devices, not a credential being stored.
pub const PAIRING_CODE_TTL: Ttl = Ttl("+5 minutes");

/// How long a paired client stays paired. Thirty days, matching the reference
/// server's `SessionStore`.
pub const SESSION_TTL: Ttl = Ttl("+30 days");

/// How long the desktop window's own boot grant lives.
///
/// Twenty-four hours, matching the reference server's
/// `DESKTOP_BOOTSTRAP_TTL_HOURS`. It outlives a page reload, which is the whole
/// reason it exists, and it does not outlive a long-running server by more than
/// a day — so a boot code that leaked out of a log stops working without anyone
/// having to notice it leaked.
///
/// A server that is still up after a day mints a fresh one; see
/// [`DESKTOP_BOOT_SUBJECT`].
pub const DESKTOP_BOOT_TTL: Ttl = Ttl("+24 hours");

/// Who the desktop window's own boot grant is issued to.
///
/// The reference server's `subject: "desktop-bootstrap"`
/// (`PairingGrantStore.ts:319`), and it is load-bearing in two places rather
/// than decorative: [`crate::store::Database::active_pairing_links`] filters on
/// it so Settings never offers the window's own credential as something to hand
/// to a phone, and it is what a reader of the `auth_pairing_links` table sees
/// when wondering why there is a code nobody minted.
///
/// **`method` stays [`ONE_TIME_TOKEN_METHOD`], not `desktop-bootstrap`.** The
/// contract's `ServerAuthBootstrapMethod` has both members, but
/// `desktop-bootstrap` describes upstream's Electron preload hand-off, and what
/// laplus does is put a one-time token in a URL fragment. Saying
/// `desktop-bootstrap` would advertise a mechanism this server does not have.
pub const DESKTOP_BOOT_SUBJECT: &str = "desktop-bootstrap";

/// How long a socket ticket stays good for. Five minutes: long enough for a
/// page to finish loading and open its socket, short enough that a ticket left
/// in a proxy log is worthless by the time anyone reads it.
pub const WEBSOCKET_TICKET_TTL: Ttl = Ttl("+5 minutes");

/// A lifetime, in the form SQLite's `strftime` takes as a modifier.
///
/// A string rather than a `Duration` because the database is this server's
/// clock — see [`crate::store`]'s own note on `strftime` — and an expiry
/// computed in Rust from a `SystemTime` would be a second answer to "what time
/// is it" that could disagree with the `created_at` beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ttl(pub &'static str);

impl fmt::Display for Ttl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

/// The randomness call failed, which on a healthy machine does not happen.
///
/// It is still an error rather than a panic: the caller is an HTTP handler, and
/// a 500 the user can retry beats taking the window down. See the note on
/// `panic = "abort"` in the workspace manifest for why a panic here would be
/// worse than it looks.
#[derive(Debug)]
pub struct RandomError(getrandom::Error);

impl fmt::Display for RandomError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "cannot read randomness from the operating system: {}", self.0)
    }
}

impl std::error::Error for RandomError {}

/// A fresh pair code: twelve characters, uniform over the alphabet.
pub fn pairing_code() -> Result<String, RandomError> {
    let mut code = String::with_capacity(PAIRING_CODE_LENGTH);

    // A loop rather than one fill, because a rejected byte yields no character.
    // With this alphabet no byte is ever rejected and this runs exactly once;
    // it is written to stay correct if that stops being true.
    while code.len() < PAIRING_CODE_LENGTH {
        let mut bytes = [0u8; PAIRING_CODE_LENGTH];
        getrandom::fill(&mut bytes).map_err(RandomError)?;

        for byte in bytes {
            if usize::from(byte) >= PAIRING_CODE_REJECTION_LIMIT {
                continue;
            }
            let index = usize::from(byte) % PAIRING_CODE_ALPHABET.len();
            code.push(char::from(PAIRING_CODE_ALPHABET[index]));
            if code.len() == PAIRING_CODE_LENGTH {
                return Ok(code);
            }
        }
    }

    Ok(code)
}

/// A fresh opaque token — a session bearer, or a socket ticket.
///
/// Thirty-two bytes rendered as hex. Not the pair-code alphabet: nobody types
/// one of these, so there is no reason to spend length on legibility, and every
/// reason to spend it on entropy.
pub fn opaque_token() -> Result<String, RandomError> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(RandomError)?;

    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

/// An identifier for a row the client will name back to us — a pairing link to
/// revoke, or a session to report.
///
/// Sixteen bytes of hex rather than a counter: these appear in the UI and in
/// revoke requests, and a guessable id would let one paired client revoke
/// another's pairing link by naming a number. Not a secret, but not a sequence
/// either.
pub fn record_id() -> Result<String, RandomError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(RandomError)?;

    let mut id = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        id.push_str(&format!("{byte:02x}"));
    }
    Ok(id)
}

/// How a credential came to exist, in the contract's vocabulary.
///
/// The contract's `ServerAuthBootstrapMethod` has two members and this server
/// mints one of them. `desktop-bootstrap` is upstream's trusted hand-off from
/// an Electron main process to its renderer; laplus's window and server are one
/// process reaching each other over loopback, so there is nothing to hand off
/// and the honest answer is that this server does not issue them.
pub const ONE_TIME_TOKEN_METHOD: &str = "one-time-token";

/// How an established client authenticates, in the contract's vocabulary.
///
/// `bearer-access-token` — a session opened at `POST /oauth/token` and presented
/// in an `Authorization` header afterwards. What a client holds when it attached
/// to this machine as a *remote* backend from somewhere else.
pub const BEARER_SESSION_METHOD: &str = "bearer-access-token";

/// The other one: a session opened at `POST /api/auth/browser-session` and
/// presented in a cookie the browser sends by itself.
///
/// **This is the method the desktop window and a phone both actually use.** A
/// browser that loaded the app *from* this server is talking to its primary
/// environment, and the client's primary path is
/// `exchangeBootstrapCredential` → `client.auth.browserSession`
/// (`apps/web/src/environments/primary/auth.ts:231-240`). The bearer above is
/// for a client that added this machine as a second backend. Ticket 73's scope
/// list names four routes and omits this one, which is a gap in the ticket
/// rather than in the contract — see its Findings.
pub const BROWSER_SESSION_METHOD: &str = "browser-session-cookie";

/// Who a locally minted pairing link is issued to.
///
/// The reference server's own default (`PairingGrantStore.ts:401` —
/// `input?.subject ?? "one-time-token"`), which is the same string as the
/// method because there is no identity behind it: this server has no accounts,
/// and the subject exists so that Settings has something to display beside a
/// code. Copied rather than invented so a database written by one and read by
/// the other says the same thing.
pub const PAIRING_SUBJECT: &str = ONE_TIME_TOKEN_METHOD;

/// The three RFC 6749 literals `POST /oauth/token` pins.
///
/// The contract types all three as `Schema.Literal`, so a body carrying
/// anything else is a client that has been changed without this server rather
/// than a client asking for something unsupported — which is why the refusal is
/// [`crate::http::invalid_token_request`] and not an
/// `unsupported_grant_type` the contract has no member for.
pub const TOKEN_EXCHANGE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
pub const BOOTSTRAP_TOKEN_TYPE: &str = "urn:t3:params:oauth:token-type:environment-bootstrap";
pub const ACCESS_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:access_token";

/// Every scope the contract declares, in its own order.
///
/// **Nothing gates on these** — see this module's docs — but the vocabulary is
/// still closed, and closing it here is not enforcement: `AuthEnvironmentScope`
/// is a literal union, so a scope outside this list travelling back in
/// `AuthPairingLink.scopes` or `AuthSessionState.scopes` fails the client's
/// decode and takes the whole Settings panel with it. Refusing an unknown scope
/// at the door is what keeps every field this server fills decodable by the
/// client that reads it. The reference server checks the same set at the same
/// place (`auth/http.ts:262-278`).
pub const ENVIRONMENT_SCOPES: [&str; 8] = [
    "orchestration:read",
    "orchestration:operate",
    "terminal:operate",
    "review:write",
    "access:read",
    "access:write",
    "relay:read",
    "relay:write",
];

/// Is this one of the eight?
pub fn is_environment_scope(scope: &str) -> bool {
    ENVIRONMENT_SCOPES.contains(&scope)
}

/// Does `granted` cover every scope in `requested`?
///
/// The check `POST /oauth/token` makes before it opens a session, and the
/// reference server's (`EnvironmentAuth.ts:697-700`). A code minted to grant
/// three scopes cannot be spent for a fourth — not because anything would stop
/// the fourth from being used, but because a session reporting a scope its
/// pairing code never granted would be this server telling the user something
/// untrue about what they handed out.
pub fn covers(granted: &[String], requested: &[String]) -> bool {
    requested.iter().all(|scope| granted.contains(scope))
}

/// A pairing link as it stands in the database, and as the Settings list shows
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingLink {
    pub id: String,
    /// Stored in plaintext, deliberately.
    ///
    /// The reference server does the same, and for a reason that holds here:
    /// Settings lists an active link so the user can copy the code again from
    /// the machine that minted it, and a hash makes that impossible. The threat
    /// it would defend against is someone reading this file — who already has
    /// every conversation, every file path and every transcript in the same
    /// database, and does not need a five-minute pairing code.
    pub credential: String,
    pub scopes: Vec<String>,
    pub subject: String,
    pub label: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

/// What a consumed pair code entitles its bearer to. The thing a session is
/// minted from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub subject: String,
    pub scopes: Vec<String>,
    pub label: Option<String>,
}

/// A session, as `/oauth/token` reports it and as the socket verifies it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub token: String,
    pub subject: String,
    pub scopes: Vec<String>,
    pub expires_at: String,
    /// Seconds from now until [`Session::expires_at`], which is what the
    /// contract's `AuthAccessTokenResult.expires_in` wants. Computed by the
    /// database alongside the expiry so the two cannot disagree.
    pub expires_in: i64,
}

/// A socket ticket, as `/api/auth/websocket-ticket` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketTicket {
    pub ticket: String,
    pub expires_at: String,
}

/// Why a credential was not accepted.
///
/// Three cases and not one, because they are not the same event: an unknown
/// code is someone typing it wrong or guessing, an expired one is someone who
/// took too long, and a spent one is the second half of a race or a link
/// forwarded twice. All three become the same 401 on the wire — the contract's
/// `EnvironmentAuthInvalidReason` has no member for any of them — so this
/// distinction exists for the log, which is the only place it can do any good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialRefusal {
    Unknown,
    Expired,
    AlreadyUsed,
    Revoked,
}

impl CredentialRefusal {
    pub fn detail(self) -> &'static str {
        match self {
            CredentialRefusal::Unknown => "no such pairing code",
            CredentialRefusal::Expired => "the pairing code has expired",
            CredentialRefusal::AlreadyUsed => "the pairing code was already used",
            CredentialRefusal::Revoked => "the pairing code was revoked",
        }
    }
}

/// The scopes a client gets when it asks for none.
///
/// The contract's `AuthAccessTokenResult.scope` is a non-empty string, so
/// "none" is not a value this server can report even though nothing is
/// enforced. Read-and-operate is what the UI's own "Standard" preset grants.
pub fn default_scopes() -> Vec<String> {
    vec![
        "orchestration:read".to_string(),
        "orchestration:operate".to_string(),
        "terminal:operate".to_string(),
        "review:write".to_string(),
        "relay:read".to_string(),
    ]
}

/// Everything, in the contract's own order.
///
/// What the desktop window's boot grant carries, matching the reference
/// server's `AuthAdministrativeScopes` (`PairingGrantStore.ts:318`). The window
/// is the machine's own console: it manages the pairing links themselves, which
/// is what `access:read` and `access:write` name, and a window that could not
/// open its own Settings panel would be a strange thing to have booted.
///
/// A phone gets [`default_scopes`] instead — the same list the UI's "Standard"
/// preset offers — because a device carried out of the house should not arrive
/// holding the keys to the list of keys.
pub fn administrative_scopes() -> Vec<String> {
    ENVIRONMENT_SCOPES.iter().map(|scope| scope.to_string()).collect()
}

/// Render scopes as RFC 6749 wants them on the wire: space-delimited, in the
/// order they were granted.
pub fn encode_scopes(scopes: &[String]) -> String {
    scopes.join(" ")
}

/// Read an RFC 6749 `scope` parameter, keeping first-seen order and dropping
/// repeats — `parseOAuthScope` in `packages/shared`, which is what the client
/// encodes with — and refusing any scope outside [`ENVIRONMENT_SCOPES`].
///
/// Returns `None` for a value that is not a valid scope list, which the caller
/// turns into the contract's `invalid_scope`. An **empty** string is not a
/// valid list, matching the client: it would mean "grant nothing", and the
/// contract cannot report that.
///
/// The syntax check is kept alongside the vocabulary check rather than replaced
/// by it. They fail the same way here, but they are different mistakes — a
/// quoting bug and a client from a newer contract — and the syntax rule is the
/// one that says what a scope *list* is at all.
pub fn parse_scopes(value: &str) -> Option<Vec<String>> {
    if value.is_empty() {
        return None;
    }

    let mut scopes: Vec<String> = Vec::new();
    for scope in value.split(' ') {
        if scope.is_empty() || !scope.bytes().all(is_oauth_scope_byte) {
            return None;
        }
        if !is_environment_scope(scope) {
            return None;
        }
        let scope = scope.to_string();
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }
    Some(scopes)
}

/// RFC 6749's `scope-token`: visible ASCII except the double quote and the
/// backslash. The same set `OAUTH_SCOPE_TOKEN` matches in
/// `packages/shared/src/oauthScope.ts`.
fn is_oauth_scope_byte(byte: u8) -> bool {
    byte == 0x21 || (0x23..=0x5b).contains(&byte) || (0x5d..=0x7e).contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn a_pair_code_is_twelve_characters_from_the_alphabet() {
        let code = pairing_code().expect("randomness");
        assert_eq!(code.len(), PAIRING_CODE_LENGTH);
        assert!(
            code.bytes().all(|byte| PAIRING_CODE_ALPHABET.contains(&byte)),
            "{code} should use only the pairing alphabet"
        );
    }

    /// The whole reason for the alphabet: a code is read off one screen and
    /// typed into another, and these are the characters that get mistyped.
    ///
    /// **`L` is kept**, though `1` and `I` are not. That is the reference
    /// server's choice and it is not an oversight — dropping a fourth letter
    /// would leave 31, and 31 does not divide 256, which is the arithmetic
    /// [`PAIRING_CODE_REJECTION_LIMIT`] is about. A capital `L` is legible in
    /// the fonts this is displayed in; the digit it resembles is already gone.
    #[test]
    fn the_alphabet_excludes_the_characters_that_look_alike() {
        for confusable in [b'0', b'O', b'1', b'I'] {
            assert!(
                !PAIRING_CODE_ALPHABET.contains(&confusable),
                "{} should not be in the alphabet",
                char::from(confusable)
            );
        }
        assert!(PAIRING_CODE_ALPHABET.contains(&b'L'));
        assert_eq!(PAIRING_CODE_ALPHABET.len(), 32);
    }

    /// Thirty-two divides 256, so the rejection limit is the whole byte range
    /// and the sampling loop never discards anything. Pinned because the
    /// constant reads like it does something, and the day it *should* is the day
    /// someone shortens the alphabet.
    #[test]
    fn nothing_is_rejected_while_the_alphabet_divides_the_byte_range() {
        assert_eq!(PAIRING_CODE_REJECTION_LIMIT, 256);
    }

    /// Not a distribution test — that would be a test that fails on a Tuesday.
    /// This asserts the far weaker thing that actually catches a broken
    /// generator: two codes in a row are not the same one.
    #[test]
    fn codes_do_not_repeat() {
        let codes: HashSet<String> = (0..32).map(|_| pairing_code().expect("randomness")).collect();
        assert_eq!(codes.len(), 32);
    }

    /// Every alphabet position is reachable. A generator that could only ever
    /// produce half its alphabet would still pass every test above.
    #[test]
    fn the_whole_alphabet_is_reachable() {
        let mut seen: HashSet<u8> = HashSet::new();
        for _ in 0..512 {
            seen.extend(pairing_code().expect("randomness").bytes());
        }
        assert_eq!(seen.len(), PAIRING_CODE_ALPHABET.len());
    }

    #[test]
    fn tokens_are_sixty_four_hex_characters_and_do_not_repeat() {
        let first = opaque_token().expect("randomness");
        let second = opaque_token().expect("randomness");
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn record_ids_are_thirty_two_hex_characters_and_do_not_repeat() {
        let first = record_id().expect("randomness");
        let second = record_id().expect("randomness");
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn scopes_round_trip_through_the_oauth_encoding() {
        let scopes = default_scopes();
        let encoded = encode_scopes(&scopes);
        assert_eq!(encoded, "orchestration:read orchestration:operate terminal:operate review:write relay:read");
        assert_eq!(parse_scopes(&encoded), Some(scopes));
    }

    /// First-seen order is kept and repeats are dropped, matching
    /// `parseOAuthScope`. The client builds this string, so a server that
    /// disagreed about what it means would grant something other than what was
    /// asked for.
    #[test]
    fn parsing_keeps_first_seen_order_and_drops_repeats() {
        assert_eq!(
            parse_scopes("relay:read review:write relay:read terminal:operate review:write"),
            Some(vec![
                "relay:read".to_string(),
                "review:write".to_string(),
                "terminal:operate".to_string()
            ])
        );
    }

    #[test]
    fn an_unparseable_scope_list_is_refused() {
        for invalid in [
            "",
            " ",
            "relay:read  review:write",
            "relay:\"read",
            "relay:\\read",
            "relay:read ",
        ] {
            assert_eq!(parse_scopes(invalid), None, "{invalid:?} should not parse");
        }
    }

    /// A scope that is spelled like a scope but is not one of the contract's
    /// eight. It has to be refused rather than recorded, because
    /// `AuthEnvironmentScope` is a literal union on the client and a session
    /// carrying `orchestration:destroy` back would fail the decode that renders
    /// the whole Settings panel.
    #[test]
    fn a_scope_outside_the_contracts_vocabulary_is_refused() {
        assert_eq!(parse_scopes("orchestration:destroy"), None);
        assert_eq!(parse_scopes("relay:read orchestration:destroy"), None);
        assert_eq!(parse_scopes("Relay:Read"), None, "and it is case-sensitive");
    }

    /// Every scope this server hands out by default is one it would accept
    /// back. The two lists are written separately and this is what stops them
    /// drifting.
    #[test]
    fn the_default_scopes_are_all_in_the_contracts_vocabulary() {
        for scope in default_scopes() {
            assert!(is_environment_scope(&scope), "{scope} is not a known scope");
        }
        assert_eq!(parse_scopes(&encode_scopes(&default_scopes())).as_ref(), Some(&default_scopes()));
    }

    /// The check `/oauth/token` makes: a code minted for three scopes cannot be
    /// spent for a fourth.
    #[test]
    fn a_grant_covers_only_what_it_granted() {
        let granted = default_scopes();
        assert!(covers(&granted, &granted));
        assert!(covers(&granted, &["relay:read".to_string()]));
        assert!(covers(&granted, &[]), "asking for nothing asks for nothing new");
        assert!(!covers(&granted, &["access:write".to_string()]));
        assert!(!covers(
            &granted,
            &["relay:read".to_string(), "access:write".to_string()]
        ));
    }
}
