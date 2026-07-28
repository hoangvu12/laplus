//! Keybindings: what the developer has bound, compiled into what the UI reads.
//!
//! Two method tags land here — `server.upsertKeybinding` and
//! `server.removeKeybinding` — and between them they are the whole of "a
//! developer customises their shortcuts". The third way in is the file itself:
//! `keybindings.json` in the app's data directory, whose path this server has
//! advertised in `server.getConfig` since ticket 03 precisely so that editing it
//! by hand is a supported thing to do.
//!
//! ## Source form and resolved form are different things
//!
//! What is stored is `{"key": "mod+shift+d", "command": "terminal.splitVertical",
//! "when": "terminalFocus"}` — a **rule**, written the way a person writes one.
//! What the UI consumes is a **resolved** rule: the shortcut split into the five
//! booleans a `KeyboardEvent` carries, and the `when` expression parsed into a
//! tree the client can evaluate against its own focus state.
//!
//! Compiling is this module's job because the client cannot do it: it holds no
//! file. And it has to be done *exactly* the way upstream does it, because the
//! client is upstream's — `parseKeybindingShortcut` and
//! `parseKeybindingWhenExpression` in `t3code/packages/shared/src/keybindings.ts`
//! are mirrored here line for line, including the parts that look like
//! accidents:
//!
//! - **`mod` is its own flag**, not "meta on a Mac, control elsewhere". The
//!   client decides which physical key that is, because the client is the one
//!   with a keyboard.
//! - **A trailing `+` binds the plus key.** `"mod++"` is `mod` plus `+`, which is
//!   why the tokeniser counts empty trailing tokens rather than rejecting them —
//!   and one of the defaults relies on it.
//! - **`space` and `esc` are spelled out.** The rest of the key names are
//!   whatever the browser calls them, lower-cased.
//!
//! ## The defaults are mirrored, not invented
//!
//! [`defaults`] is upstream's `DEFAULT_KEYBINDINGS`, and it has to be: the UI has
//! its own copy compiled in, and a shortcut this server did not send is one the
//! developer's muscle memory produces and nothing answers.
//!
//! They are **merged by command, not by key**. A custom rule for
//! `terminal.toggle` replaces the default for `terminal.toggle` however either
//! is spelled — which is what makes rebinding one shortcut leave the other
//! forty alone, and what makes "changed" and "removed" different operations
//! rather than the same one twice.
//!
//! ## Nothing here is fatal, and one thing very nearly is
//!
//! A file that is not JSON, an entry that is not a rule, a `when` expression that
//! does not parse: each is reported as a [`crate::config::ConfigIssue`] beside the
//! keybindings that *did* compile, and the app starts. The one criterion of
//! ticket 22 this is written for says so in as many words — "a corrupt or
//! unreadable settings store falls back to defaults with a warning rather than
//! failing to start" — and the contract has a place to put the warning, which is
//! why it is a place rather than a log line.
//!
//! That place has **two shapes and no others**. `ServerConfigIssue` is a closed
//! union of `keybindings.malformed-config` and `keybindings.invalid-entry`, and
//! `keybindings` itself is an array of `ResolvedKeybindingRule` whose `command`
//! is a closed union of forty-one. So the two ways to turn "one bad line" into
//! "the app will not open" are an issue with a `kind` of this server's own
//! invention, and a command the contract does not name — either fails the
//! client's decode of the whole `server.getConfig` payload. [`check`] and
//! [`invalid_entry`] are where that is prevented, and both say so.
//!
//! Shapes are hand-written from `ResolvedKeybindingRule`, `KeybindingRule`,
//! `ServerUpsertKeybindingInput`, `ServerRemoveKeybindingInput` and
//! `KeybindingsConfigError` in `t3code/packages/contracts/src/keybindings.ts` and
//! `server.ts`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{ConfigIssue, KeybindingShortcut, ResolvedKeybinding};

/// Binding a shortcut, or rebinding one.
pub const UPSERT: &str = "server.upsertKeybinding";

/// Unbinding one.
pub const REMOVE: &str = "server.removeKeybinding";

/// The `_tag` both methods refuse under.
///
/// Note that it is **not** the class name: `KeybindingsConfigError` is declared
/// in the contract with the tag `KeybindingsConfigParseError`, and the client
/// decodes on the tag. A server that sent the class name would fail every
/// refusal to decode.
const ERROR: &str = "KeybindingsConfigParseError";

/// The file, inside the app's data directory.
pub const FILE: &str = "keybindings.json";

/// The most rules one configuration may carry — `MAX_KEYBINDINGS_COUNT`.
///
/// The client checks the same number and refuses a longer array outright, so a
/// configuration past this would not be a long list, it would be *no*
/// keybindings at all. Past it the **latest** rules are kept, because later
/// rules win.
const MAX_RULES: usize = 256;

/// The longest a `key` may be — `MAX_KEYBINDING_VALUE_LENGTH`.
///
/// Enforced in [`check`] and **not** in [`shortcut`], which is a faithful mirror
/// of a parser the client also runs and which has no length rule of its own. The
/// length belongs to the *schema* — `KeybindingValue` — and so belongs where the
/// schema's other rules are.
const MAX_KEY: usize = 64;

/// The longest a `when` may be — `MAX_KEYBINDING_WHEN_LENGTH`. Enforced where
/// [`MAX_KEY`] is, and for the same reason.
const MAX_WHEN: usize = 256;

/// How deep a `when` expression may nest — `MAX_WHEN_EXPRESSION_DEPTH`.
///
/// A bound on the parser's own recursion as much as on the expression: this is
/// a hand-written descent parser reading a string from a file the developer
/// edits, and without a limit `!!!!!!…` is a stack overflow rather than a
/// refusal.
const MAX_DEPTH: usize = 64;

// ---------------------------------------------------------------------------
// A rule, as it is written down
// ---------------------------------------------------------------------------

/// One binding in source form — what is in the file and what the two methods
/// take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub key: String,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub when: Option<String>,
}

impl Rule {
    fn new(key: &str, command: &str) -> Rule {
        Rule {
            key: key.to_string(),
            command: command.to_string(),
            when: None,
        }
    }

    fn when(key: &str, command: &str, when: &str) -> Rule {
        Rule {
            when: Some(when.to_string()),
            ..Rule::new(key, command)
        }
    }

    /// Is this the same binding as `other` — the same key, doing the same
    /// thing, under the same condition?
    ///
    /// All three, because a remove names a binding rather than a command: a
    /// developer who bound `mod+d` to two things under two conditions is
    /// entitled to remove one of them.
    fn is(&self, other: &Rule) -> bool {
        self.key == other.key && self.command == other.command && self.when == other.when
    }

    /// Compile, or say nothing — the caller decides whether a rule that will not
    /// compile is an issue or a silent default.
    fn resolve(&self) -> Option<ResolvedKeybinding> {
        let shortcut = shortcut(&self.key)?;
        let when_ast = match &self.when {
            Some(when) => Some(when_expression(when)?),
            None => None,
        };
        Some(ResolvedKeybinding {
            command: self.command.clone(),
            shortcut,
            when_ast,
        })
    }

    /// Why this rule will not compile, as a sentence for the developer.
    ///
    /// Only asked once [`Rule::resolve`] has already said no, so it is allowed
    /// to work the question out again rather than being threaded through it.
    fn complaint(&self) -> String {
        if self.key.chars().count() > MAX_KEY {
            return format!("'{}' is longer than {MAX_KEY} characters.", self.key);
        }
        if shortcut(&self.key).is_none() {
            return format!("'{}' is not a shortcut this server can read.", self.key);
        }
        match &self.when {
            Some(when) if when.chars().count() > MAX_WHEN => {
                format!("its condition is longer than {MAX_WHEN} characters.")
            }
            Some(when) => format!("'{when}' is not a condition this server can read."),
            None => "it is not a binding this server can read.".to_string(),
        }
    }
}

/// The bindings every developer starts with — upstream's `DEFAULT_KEYBINDINGS`.
///
/// Mirrored rather than derived, and the reason is in this module's
/// documentation: the UI compiles its own copy, so a default missing here is a
/// shortcut the developer presses and nothing answers.
fn defaults() -> Vec<Rule> {
    let mut rules = vec![
        Rule::new("mod+b", "sidebar.toggle"),
        Rule::new("mod+j", "terminal.toggle"),
        Rule::new("mod+alt+b", "rightPanel.toggle"),
        Rule::when("mod+d", "terminal.split", "terminalFocus"),
        Rule::when("mod+shift+d", "terminal.splitVertical", "terminalFocus"),
        Rule::when("mod+n", "terminal.new", "terminalFocus"),
        Rule::when("mod+w", "terminal.close", "terminalFocus"),
        Rule::when("mod+d", "diff.toggle", "!terminalFocus"),
        Rule::new("mod+shift+j", "preview.toggle"),
        Rule::when("mod+r", "preview.refresh", "previewFocus"),
        Rule::when("mod+l", "preview.focusUrl", "previewFocus"),
        Rule::when("mod+=", "preview.zoomIn", "previewFocus"),
        // The trailing-plus case, and it is load-bearing rather than a curiosity:
        // see this module's documentation and [`shortcut`].
        Rule::when("mod++", "preview.zoomIn", "previewFocus"),
        Rule::when("mod+-", "preview.zoomOut", "previewFocus"),
        Rule::when("mod+0", "preview.resetZoom", "previewFocus"),
        Rule::when("mod+k", "commandPalette.toggle", "!terminalFocus"),
        Rule::when("mod+n", "chat.new", "!terminalFocus"),
        Rule::when("mod+shift+o", "chat.new", "!terminalFocus"),
        Rule::when("mod+shift+n", "chat.newLocal", "!terminalFocus"),
        Rule::when("mod+shift+m", "modelPicker.toggle", "!terminalFocus"),
        Rule::new("mod+o", "editor.openFavorite"),
        Rule::new("mod+shift+[", "thread.previous"),
        Rule::new("mod+shift+]", "thread.next"),
    ];
    for jump in 1..=9 {
        rules.push(Rule::new(&format!("mod+{jump}"), &format!("thread.jump.{jump}")));
    }
    for jump in 1..=9 {
        rules.push(Rule::when(
            &format!("mod+{jump}"),
            &format!("modelPicker.jump.{jump}"),
            "modelPickerOpen",
        ));
    }
    rules
}

// ---------------------------------------------------------------------------
// Compiling one
// ---------------------------------------------------------------------------

/// Split `mod+shift+d` into the flags a `KeyboardEvent` carries.
///
/// Upstream's `parseKeybindingShortcut`, mirrored. The trailing-plus rule is the
/// only part that is not obvious: `"mod++"` splits to `["mod", "", ""]`, and
/// rather than rejecting the empty tokens the parser counts them and puts a
/// single `+` back — which is how the plus key is bound at all.
fn shortcut(value: &str) -> Option<KeybindingShortcut> {
    let lowered = value.to_lowercase();
    let mut tokens: Vec<&str> = lowered.split('+').map(str::trim).collect();

    let mut trailing = 0;
    while tokens.last() == Some(&"") {
        trailing += 1;
        tokens.pop();
    }
    if trailing > 0 {
        tokens.push("+");
    }
    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        return None;
    }

    let mut shortcut = KeybindingShortcut {
        key: String::new(),
        meta_key: false,
        ctrl_key: false,
        shift_key: false,
        alt_key: false,
        mod_key: false,
    };
    let mut key: Option<&str> = None;
    for token in tokens {
        match token {
            "cmd" | "meta" => shortcut.meta_key = true,
            "ctrl" | "control" => shortcut.ctrl_key = true,
            "shift" => shortcut.shift_key = true,
            "alt" | "option" => shortcut.alt_key = true,
            "mod" => shortcut.mod_key = true,
            // A second one is a shortcut naming two keys, which is not a
            // shortcut. Refused rather than resolved to the last of them.
            _ if key.is_some() => return None,
            named => key = Some(named),
        }
    }

    shortcut.key = match key? {
        "space" => " ".to_string(),
        "esc" => "escape".to_string(),
        named => named.to_string(),
    };
    Some(shortcut)
}

/// One node of a `when` expression, as `KeybindingWhenNode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenNode {
    Identifier(String),
    Not(Box<WhenNode>),
    And(Box<WhenNode>, Box<WhenNode>),
    Or(Box<WhenNode>, Box<WhenNode>),
}

impl Serialize for WhenNode {
    /// Serialized through [`WhenNode::to_value`] rather than derived, because
    /// the contract's union is tagged on `type` with a differently-shaped body
    /// per member and serde's own tagging would need four structs to say what
    /// one `match` says.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_value().serialize(serializer)
    }
}

impl WhenNode {
    pub fn to_value(&self) -> Value {
        match self {
            WhenNode::Identifier(name) => json!({"type": "identifier", "name": name}),
            WhenNode::Not(node) => json!({"type": "not", "node": node.to_value()}),
            WhenNode::And(left, right) => json!({
                "type": "and",
                "left": left.to_value(),
                "right": right.to_value(),
            }),
            WhenNode::Or(left, right) => json!({
                "type": "or",
                "left": left.to_value(),
                "right": right.to_value(),
            }),
        }
    }
}

/// One token of a `when` expression.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Identifier(String),
    Not,
    And,
    Or,
    Open,
    Close,
}

/// Split a `when` expression into tokens, or refuse it whole.
///
/// `&&` and `||` rather than `and`/`or`, because those are the spellings
/// upstream's tokeniser accepts and the developer's file is shared with nothing
/// else. An identifier is `[A-Za-z_][A-Za-z0-9_.-]*`, which is what makes
/// `terminalFocus` and `preview.focus` both names and `1focus` a refusal.
fn tokenize(expression: &str) -> Option<Vec<Token>> {
    let characters: Vec<char> = expression.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;

    while at < characters.len() {
        let current = characters[at];
        if current.is_whitespace() {
            at += 1;
            continue;
        }
        if characters[at..].starts_with(&['&', '&']) {
            tokens.push(Token::And);
            at += 2;
            continue;
        }
        if characters[at..].starts_with(&['|', '|']) {
            tokens.push(Token::Or);
            at += 2;
            continue;
        }
        match current {
            '!' => {
                tokens.push(Token::Not);
                at += 1;
                continue;
            }
            '(' => {
                tokens.push(Token::Open);
                at += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::Close);
                at += 1;
                continue;
            }
            _ => {}
        }

        if !(current.is_ascii_alphabetic() || current == '_') {
            return None;
        }
        let mut name = String::new();
        while at < characters.len() {
            let character = characters[at];
            if character.is_ascii_alphanumeric()
                || character == '_'
                || character == '.'
                || character == '-'
            {
                name.push(character);
                at += 1;
            } else {
                break;
            }
        }
        tokens.push(Token::Identifier(name));
    }

    Some(tokens)
}

/// Parse a `when` expression into the tree the client evaluates.
///
/// `||` binds loosest, then `&&`, then `!`, then parentheses — the ordinary
/// precedence, and upstream's. Refused whole rather than in part: an expression
/// this cannot read is a binding whose condition nobody knows, and applying it
/// unconditionally would fire a shortcut in the context it was written to avoid.
fn when_expression(expression: &str) -> Option<WhenNode> {
    let tokens = tokenize(expression)?;
    if tokens.is_empty() {
        return None;
    }

    let mut reading = Reading { tokens, at: 0 };
    let parsed = reading.or(0)?;
    // Trailing tokens mean the expression did not parse, it merely *started*
    // parsing — `a b` would otherwise resolve to `a`.
    match reading.at == reading.tokens.len() {
        true => Some(parsed),
        false => None,
    }
}

/// A `when` expression part-way through being read.
struct Reading {
    tokens: Vec<Token>,
    at: usize,
}

impl Reading {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn or(&mut self, depth: usize) -> Option<WhenNode> {
        let mut left = self.and(depth)?;
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            let right = self.and(depth)?;
            left = WhenNode::Or(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn and(&mut self, depth: usize) -> Option<WhenNode> {
        let mut left = self.unary(depth)?;
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            let right = self.unary(depth)?;
            left = WhenNode::And(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn unary(&mut self, depth: usize) -> Option<WhenNode> {
        let mut negations = 0;
        while self.peek() == Some(&Token::Not) {
            self.at += 1;
            negations += 1;
            if negations > MAX_DEPTH {
                return None;
            }
        }
        let mut node = self.primary(depth)?;
        for _ in 0..negations {
            node = WhenNode::Not(Box::new(node));
        }
        Some(node)
    }

    fn primary(&mut self, depth: usize) -> Option<WhenNode> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.peek()? {
            Token::Identifier(name) => {
                let node = WhenNode::Identifier(name.clone());
                self.at += 1;
                Some(node)
            }
            Token::Open => {
                self.at += 1;
                let inside = self.or(depth + 1)?;
                if self.peek() != Some(&Token::Close) {
                    return None;
                }
                self.at += 1;
                Some(inside)
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

/// The developer's keybindings, compiled, with whatever went wrong reading them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    pub keybindings: Vec<ResolvedKeybinding>,
    pub issues: Vec<ConfigIssue>,
    /// The rules as they are on disk, which is what an upsert edits. Separate
    /// from `keybindings` because that one has the defaults merged in, and
    /// writing those back would freeze today's defaults into the developer's
    /// file forever.
    pub rules: Vec<Rule>,
}

/// Read the keybindings file and compile it, or say what stopped that.
///
/// **Never fails.** A missing file is the ordinary case — the developer has not
/// customised anything — and every other way this can go wrong produces the
/// defaults plus an issue. That is the criterion this exists for; see the module
/// documentation.
pub fn load(directory: &Path) -> Loaded {
    let path = directory.join(FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        // Nothing written yet, or nothing readable. The first is normal and the
        // second is worth saying, and `NotFound` is the only way to tell them
        // apart.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return compiled(Vec::new(), Vec::new())
        }
        Err(error) => {
            return compiled(
                Vec::new(),
                vec![malformed(&path, &format!("it could not be read: {error}"))],
            )
        }
    };

    let parsed: Value = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(error) => {
            return compiled(
                Vec::new(),
                vec![malformed(&path, &format!("it is not valid JSON: {error}"))],
            )
        }
    };
    let Some(entries) = parsed.as_array() else {
        return compiled(
            Vec::new(),
            vec![malformed(&path, "it is not a list of keybindings.")],
        );
    };

    // Read one at a time rather than as an array, so one bad entry costs its own
    // row and not the whole file — the same rule the working tree status follows
    // for a truncated record.
    let mut rules = Vec::new();
    let mut issues = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match serde_json::from_value::<Rule>(entry.clone()) {
            Ok(rule) => match check(&rule) {
                Ok(()) => rules.push(rule),
                Err(why) => issues.push(invalid_entry(&path, index, &why)),
            },
            Err(error) => issues.push(invalid_entry(
                &path,
                index,
                &format!("it is not a keybinding: {error}"),
            )),
        }
    }

    compiled(rules, issues)
}

/// Write the developer's rules back, and hand back what the UI should now see.
///
/// The file is written **whole**, because it is the developer's own document:
/// merging into it would mean parsing what could not be parsed. A write that
/// fails is the one thing here that *is* an error, because the developer asked
/// for a change and did not get one.
fn save(directory: &Path, rules: &[Rule]) -> Result<(), String> {
    let path = directory.join(FILE);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("its directory could not be made: {error}"))?;
    }
    let written = serde_json::to_string_pretty(rules)
        .map_err(|error| format!("it could not be written out: {error}"))?;
    std::fs::write(&path, written + "\n")
        .map_err(|error| format!("it could not be written: {error}"))
}

/// Merge custom rules over the defaults and compile the result.
///
/// **By command**, which is the whole of what makes rebinding one shortcut leave
/// the rest alone — see the module documentation. A rule that will not compile
/// has already been reported by the caller and is dropped here.
fn compiled(rules: Vec<Rule>, issues: Vec<ConfigIssue>) -> Loaded {
    let overridden: Vec<&str> = rules.iter().map(|rule| rule.command.as_str()).collect();
    let mut merged: Vec<Rule> = defaults()
        .into_iter()
        .filter(|default| !overridden.contains(&default.command.as_str()))
        .collect();
    merged.extend(rules.iter().cloned());

    // Later rules win, so a configuration past the ceiling keeps its newest.
    let keep = merged.len().saturating_sub(MAX_RULES);
    let keybindings = merged[keep..].iter().filter_map(Rule::resolve).collect();

    Loaded {
        keybindings,
        issues,
        rules,
    }
}

/// The whole file was unusable.
///
/// `keybindings.malformed-config` is one of the two literals
/// `ServerConfigIssue` allows — see [`ConfigIssue`], where the cost of
/// inventing a third is spelled out.
fn malformed(path: &Path, detail: &str) -> ConfigIssue {
    ConfigIssue {
        kind: "keybindings.malformed-config",
        message: format!("The keybindings file at {} was not used: {detail}", path.display()),
        index: None,
    }
}

/// One entry of the file was unusable, and the rest of it was fine.
///
/// The other of the two literals. `index` is required by it and is what lets
/// the UI point at the line rather than at the file.
fn invalid_entry(path: &Path, index: usize, detail: &str) -> ConfigIssue {
    ConfigIssue {
        kind: "keybindings.invalid-entry",
        message: format!(
            "Entry {index} of the keybindings file at {} was ignored: {detail}",
            path.display()
        ),
        index: Some(index),
    }
}

// ---------------------------------------------------------------------------
// The calls
// ---------------------------------------------------------------------------

/// A validated `server.upsertKeybinding`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upsert {
    rule: Rule,
    /// The binding this one is *replacing*, when the developer is rebinding
    /// rather than adding.
    ///
    /// Named separately from the new rule because the two differ in exactly the
    /// case that matters: changing `mod+b` to `mod+shift+b` is a remove and an
    /// add, and without the target the old one would be left behind and both
    /// would fire.
    replace: Option<Rule>,
}

/// A validated `server.removeKeybinding`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remove {
    rule: Rule,
}

#[derive(Debug, Deserialize)]
struct RulePayload {
    key: String,
    command: String,
    when: Option<String>,
    replace: Option<Box<RulePayload>>,
}

impl RulePayload {
    fn rule(&self) -> Rule {
        Rule {
            key: self.key.trim().to_string(),
            command: self.command.trim().to_string(),
            // Absent and blank are the same thing — an unconditional binding —
            // because the contract types `when` as a non-empty trimmed string
            // and a client that sent `""` means "no condition" rather than "a
            // condition nothing satisfies".
            when: self
                .when
                .as_ref()
                .map(|when| when.trim().to_string())
                .filter(|when| !when.is_empty()),
        }
    }
}

impl Upsert {
    /// Read one, naming the file a refusal should send the developer to.
    ///
    /// The directory is an argument rather than looked up here because a
    /// refusal's `configPath` is the one useful thing in a
    /// `KeybindingsConfigParseError` — the developer's next move is to open that
    /// file — and a call that could not name it would be refusing with a blank.
    pub fn read(payload: &Value, directory: &Path) -> Result<Upsert, Value> {
        let read: RulePayload = serde_json::from_value(payload.clone()).map_err(|error| {
            refused_at(directory, &format!("the request is malformed: {error}"))
        })?;
        let rule = read.rule();
        check(&rule).map_err(|why| refused_at(directory, &why))?;
        Ok(Upsert {
            rule,
            replace: read.replace.as_ref().map(|target| target.rule()),
        })
    }

    /// Bind it, and hand back everything the UI should now show.
    ///
    /// Deferred — see [`crate::rpc::Deferred`] — because it reads and writes a
    /// file. Small work, but the read loop owes the next frame and a disk that
    /// is busy is not bounded by anything this server controls.
    pub fn run(self, store: &crate::config_store::ConfigStore) -> Result<Value, Value> {
        store.rebind(|rules| {
            // **Both** are taken out before the new one goes in, which is
            // upstream's rule (`isSameKeybindingRule` against the replace target
            // *and* against the rule itself). The target is what stops a rebind
            // leaving the old key firing; the rule's own identity is what stops
            // binding something already bound from appending a duplicate — and a
            // rebind that names a target can be both at once.
            rules.retain(|held| {
                !held.is(&self.rule)
                    && !self.replace.as_ref().is_some_and(|target| held.is(target))
            });
            rules.push(self.rule.clone());
        })
    }
}

impl Remove {
    /// Read one, naming the file a refusal should send the developer to. See
    /// [`Upsert::read`].
    pub fn read(payload: &Value, directory: &Path) -> Result<Remove, Value> {
        let read: RulePayload = serde_json::from_value(payload.clone()).map_err(|error| {
            refused_at(directory, &format!("the request is malformed: {error}"))
        })?;
        let rule = read.rule();
        check(&rule).map_err(|why| refused_at(directory, &why))?;
        Ok(Remove { rule })
    }

    /// Unbind it.
    ///
    /// **Removing something that is not bound succeeds**, and that is the
    /// behaviour rather than a shortcut: the developer's intent is "this should
    /// not be bound", and it already is not. Failing would be telling them their
    /// click did not work when the world is exactly as they asked.
    ///
    /// What it cannot do is remove a *default*. Defaults are not in the file, so
    /// there is nothing to take out — the way to lose one is to bind its command
    /// to something else, which is what the merge is by command for.
    pub fn run(self, store: &crate::config_store::ConfigStore) -> Result<Value, Value> {
        store.rebind(|rules| rules.retain(|held| !held.is(&self.rule)))
    }
}

/// What has to be true of a rule, wherever it came from.
///
/// The same check reads the file and reads a request, so a hand-edited entry is
/// refused by the same rule as one that arrived over the socket — and a rule
/// that reaches the file is one the developer will find bound.
///
/// **The command has to be one the contract names.** `KeybindingCommand` is a
/// closed union — twenty-one literals, two families of nine, and
/// `script.<id>.run` — inside `ResolvedKeybindingRule`, and `keybindings` is an
/// *array* of those. So one unrecognised command is not an inert shortcut: it
/// fails the client's decode of every binding beside it, and the developer loses
/// all forty-one over a typo. That is also why a *newer* UI's unknown command is
/// refused rather than stored — storing it would break the older client that has
/// to read the file back.
fn check(rule: &Rule) -> Result<(), String> {
    if rule.command.is_empty() {
        return Err("a keybinding has to name a command.".to_string());
    }
    if !known_command(&rule.command) {
        return Err(format!(
            "'{}' is not a command this server can bind; a custom script is bound as \
             'script.<name>.run'.",
            rule.command
        ));
    }
    // `KeybindingValue` and `KeybindingWhen` are trimmed strings with a length
    // range. All three checks are here rather than in the parsers, which are
    // faithful mirrors of functions the client also runs — see [`MAX_KEY`]. The
    // empty case is the one worth naming: [`shortcut`] reads `""` as the plus
    // key, exactly as upstream's does.
    if rule.key.is_empty() || rule.key.chars().count() > MAX_KEY {
        return Err(format!(
            "a keybinding's shortcut has to be between 1 and {MAX_KEY} characters."
        ));
    }
    if rule.when.as_ref().is_some_and(|when| when.chars().count() > MAX_WHEN) {
        return Err(format!(
            "a keybinding's condition has to be at most {MAX_WHEN} characters."
        ));
    }
    if rule.resolve().is_none() {
        return Err(rule.complaint());
    }
    Ok(())
}

/// Is this one of the commands `KeybindingCommand` allows?
///
/// `STATIC_KEYBINDING_COMMANDS` plus the two jump families, mirrored from
/// `t3code/packages/contracts/src/keybindings.ts` — the same mirroring, and for
/// the same reason, as [`defaults`]. Every one of them appears there; this list
/// and that one are checked against each other by
/// `every_default_binding_is_a_command_the_contract_names`.
fn known_command(command: &str) -> bool {
    const STATIC: &[&str] = &[
        "sidebar.toggle",
        "terminal.toggle",
        "terminal.split",
        "terminal.splitVertical",
        "terminal.new",
        "terminal.close",
        "rightPanel.toggle",
        "diff.toggle",
        "preview.toggle",
        "preview.refresh",
        "preview.focusUrl",
        "preview.zoomIn",
        "preview.zoomOut",
        "preview.resetZoom",
        "commandPalette.toggle",
        "chat.new",
        "chat.newLocal",
        "editor.openFavorite",
        "modelPicker.toggle",
        "thread.previous",
        "thread.next",
    ];
    if STATIC.contains(&command) {
        return true;
    }
    // `thread.jump.1` … `thread.jump.9`, and the model picker's own nine.
    for family in ["thread.jump.", "modelPicker.jump."] {
        if let Some(jump) = command.strip_prefix(family) {
            return matches!(jump, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9");
        }
    }
    script_command(command)
}

/// `script.<id>.run` — `SCRIPT_RUN_COMMAND_PATTERN`.
///
/// The id is at most `MAX_SCRIPT_ID_LENGTH` characters matching
/// `^[a-z0-9][a-z0-9-]*$`. Written out rather than pattern-matched because this
/// crate has no regex dependency and the pattern is four rules.
fn script_command(command: &str) -> bool {
    /// `MAX_SCRIPT_ID_LENGTH`.
    const MAX_ID: usize = 24;

    let Some(id) = command
        .strip_prefix("script.")
        .and_then(|rest| rest.strip_suffix(".run"))
    else {
        return false;
    };
    let usable = |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    !id.is_empty()
        && id.chars().count() <= MAX_ID
        && id.chars().next().is_some_and(usable)
        && id.chars().all(|character| usable(character) || character == '-')
}

/// The typed refusal, in the shape the client decodes.
///
/// `configPath` is always the real file, because it is the one useful thing in
/// this error: `KeybindingsConfigError`'s own `message` getter composes "Unable
/// to parse keybindings config at {configPath}: {detail}", and a blank there is
/// a sentence that says nothing.
pub(crate) fn refused_at(directory: &Path, detail: &str) -> Value {
    json!({
        "_tag": ERROR,
        "configPath": directory.join(FILE).display().to_string(),
        "detail": detail,
    })
}

/// What both methods answer with: the whole configuration, not a delta.
pub(crate) fn to_result(loaded: &Loaded) -> Value {
    json!({
        "keybindings": loaded.keybindings,
        "issues": loaded.issues,
    })
}

/// Read the file, change it, write it back.
///
/// A free function so that [`crate::config_store`] can own the *ordering* — one
/// rebind at a time, and the store updated before anyone is told — without
/// owning the file format.
pub(crate) fn rebind(
    directory: &Path,
    change: impl FnOnce(&mut Vec<Rule>),
) -> Result<Loaded, Value> {
    let loaded = load(directory);
    let mut rules = loaded.rules;
    change(&mut rules);

    // Bounded before the write rather than after, so the file on disk is one
    // this server would read back unchanged.
    let keep = rules.len().saturating_sub(MAX_RULES);
    let rules: Vec<Rule> = rules[keep..].to_vec();

    save(directory, &rules).map_err(|why| refused_at(directory, &why))?;
    // Read back rather than assumed: the issues a caller is handed have to be
    // the ones a restart would find, and the merge is not something to do twice
    // in two places.
    Ok(load(directory))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(loaded: &Loaded, command: &str) -> Option<ResolvedKeybinding> {
        loaded
            .keybindings
            .iter()
            .find(|binding| binding.command == command)
            .cloned()
    }

    /// The five flags a `KeyboardEvent` carries, and `mod` staying its own —
    /// the client is the one with a keyboard and decides what `mod` is.
    #[test]
    fn a_shortcut_becomes_the_flags_the_client_matches_on() {
        let parsed = shortcut("mod+shift+d").expect("a shortcut");
        assert_eq!(parsed.key, "d");
        assert!(parsed.mod_key && parsed.shift_key);
        assert!(!parsed.meta_key && !parsed.ctrl_key && !parsed.alt_key);

        // Both spellings of each modifier, because the file is hand-edited.
        for spelling in ["cmd+k", "meta+k"] {
            assert!(shortcut(spelling).expect("a shortcut").meta_key, "{spelling}");
        }
        for spelling in ["ctrl+k", "control+k"] {
            assert!(shortcut(spelling).expect("a shortcut").ctrl_key, "{spelling}");
        }
        for spelling in ["alt+k", "option+k"] {
            assert!(shortcut(spelling).expect("a shortcut").alt_key, "{spelling}");
        }
    }

    /// The trailing-plus rule, which one of the defaults relies on. Without it
    /// `mod++` is a refusal and zooming in has one binding instead of two.
    #[test]
    fn a_trailing_plus_binds_the_plus_key() {
        let parsed = shortcut("mod++").expect("a shortcut");
        assert_eq!(parsed.key, "+");
        assert!(parsed.mod_key);
    }

    /// The two key names that are spelled rather than typed.
    #[test]
    fn space_and_escape_are_named() {
        assert_eq!(shortcut("mod+space").expect("a shortcut").key, " ");
        assert_eq!(shortcut("esc").expect("a shortcut").key, "escape");
    }

    /// A shortcut with no key, or with two, is not a shortcut. The second is
    /// the one worth pinning: resolving `a+b` to `b` would silently bind
    /// something the developer did not write.
    #[test]
    fn a_shortcut_names_exactly_one_key() {
        for refused in ["mod", "shift+ctrl", "a+b"] {
            assert!(shortcut(refused).is_none(), "{refused} was accepted");
        }
    }

    /// **An empty string parses, as the plus key.** Upstream's parser does the
    /// same thing and for the same reason — the trailing-plus rule above cannot
    /// tell "the developer wrote nothing" from "the developer wrote `+`" — so
    /// this is mirrored rather than corrected.
    ///
    /// What stops an empty binding reaching the file is the *contract*, not the
    /// parser: `KeybindingValue` is a trimmed string of at least one character,
    /// and [`check`] enforces it. Keeping the two apart is what lets this stay a
    /// faithful mirror of a function the client also runs.
    #[test]
    fn an_empty_shortcut_parses_the_way_upstreams_does_and_is_refused_elsewhere() {
        assert_eq!(shortcut("").expect("upstream parses this too").key, "+");

        let error = Upsert::read(&json!({"key": "", "command": "sidebar.toggle"}), Path::new("."))
            .expect_err("the contract's own rule refuses it");
        assert_eq!(error["_tag"], ERROR);
    }

    /// Precedence, and the tree the client walks. `||` loosest, then `&&`, then
    /// `!` — so this is `or(a, and(not(b), c))` and not anything else.
    #[test]
    fn a_condition_parses_with_the_ordinary_precedence() {
        let parsed = when_expression("a || !b && c").expect("an expression");
        assert_eq!(
            parsed,
            WhenNode::Or(
                Box::new(WhenNode::Identifier("a".to_string())),
                Box::new(WhenNode::And(
                    Box::new(WhenNode::Not(Box::new(WhenNode::Identifier("b".to_string())))),
                    Box::new(WhenNode::Identifier("c".to_string())),
                )),
            )
        );
    }

    /// Parentheses beat precedence, which is what they are for.
    #[test]
    fn parentheses_group_a_condition() {
        assert_eq!(
            when_expression("(a || b) && c").expect("an expression"),
            WhenNode::And(
                Box::new(WhenNode::Or(
                    Box::new(WhenNode::Identifier("a".to_string())),
                    Box::new(WhenNode::Identifier("b".to_string())),
                )),
                Box::new(WhenNode::Identifier("c".to_string())),
            )
        );
    }

    /// An expression this cannot read is refused whole. Reading `a b` as `a`
    /// would fire a shortcut in the context its condition was written to avoid.
    #[test]
    fn a_condition_that_does_not_parse_is_refused_rather_than_half_read() {
        for refused in ["a b", "a &&", "(a", "a)", "!", "1focus", "a || ", ""] {
            assert!(
                when_expression(refused).is_none(),
                "{refused:?} was accepted as {:?}",
                when_expression(refused)
            );
        }
    }

    /// The recursion bound. A hand-edited file is untrusted input, and without
    /// this `!!!!…` is a stack overflow rather than an ignored line.
    #[test]
    fn a_condition_cannot_nest_past_the_bound() {
        let deep = "!".repeat(MAX_DEPTH + 1) + "a";
        assert!(when_expression(&deep).is_none());

        let nested = "(".repeat(MAX_DEPTH + 2) + "a" + &")".repeat(MAX_DEPTH + 2);
        assert!(when_expression(&nested).is_none());
    }

    /// Every default compiles. They are mirrored by hand from another
    /// repository's TypeScript, so a typo is a shortcut that silently vanishes.
    #[test]
    fn every_default_binding_compiles() {
        let defaults = defaults();
        assert_eq!(defaults.len(), 41, "the mirrored list changed size");
        for rule in defaults {
            assert!(rule.resolve().is_some(), "{rule:?} does not compile");
        }
    }

    /// The two mirrored lists checked against each other.
    ///
    /// [`defaults`] and [`known_command`] are transcribed from the same
    /// TypeScript file, and a command in one and not the other is the failure
    /// that costs the most: a default this build refuses is *dropped at load*,
    /// so the developer would silently lose a shortcut they never touched.
    #[test]
    fn every_default_binding_is_a_command_the_contract_names() {
        for rule in defaults() {
            assert!(
                known_command(&rule.command),
                "'{}' is a default and not a command this build will bind",
                rule.command
            );
        }
    }

    /// The closed union, both ways. An unrecognised command cannot be stored,
    /// because `keybindings` is an array of `ResolvedKeybindingRule` and one bad
    /// member fails the client's decode of all forty-one.
    #[test]
    fn only_the_commands_the_contract_names_can_be_bound() {
        for known in [
            "sidebar.toggle",
            "thread.jump.9",
            "modelPicker.jump.1",
            "script.deploy.run",
            "script.a-b-9.run",
        ] {
            assert!(known_command(known), "{known} was refused");
        }

        for refused in [
            "sidebar.toogle",
            "thread.jump.0",
            "thread.jump.10",
            "modelPicker.jump.",
            "script..run",
            "script.Deploy.run",
            "script.-x.run",
            "script.averyveryverylongscriptname.run",
            "",
        ] {
            assert!(!known_command(refused), "{refused:?} was accepted");
        }
    }

    /// A workspace with no file at all is the ordinary case: the developer has
    /// customised nothing, and gets every default with nothing to report.
    #[test]
    fn a_project_with_no_file_gets_the_defaults_and_no_complaint() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let loaded = load(directory.path());

        assert_eq!(loaded.keybindings.len(), defaults().len());
        assert!(loaded.issues.is_empty(), "{:?}", loaded.issues);
        assert!(loaded.rules.is_empty());
    }

    /// The merge rule: by command, so rebinding one shortcut leaves the other
    /// forty alone.
    #[test]
    fn a_custom_binding_replaces_the_default_for_its_command_and_nothing_else() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            directory.path().join(FILE),
            r#"[{"key": "ctrl+alt+s", "command": "sidebar.toggle"}]"#,
        )
        .expect("writes the file");

        let loaded = load(directory.path());
        let sidebar = resolved(&loaded, "sidebar.toggle").expect("the rebound command");
        assert_eq!(sidebar.shortcut.key, "s");
        assert!(sidebar.shortcut.ctrl_key && sidebar.shortcut.alt_key);
        assert!(!sidebar.shortcut.mod_key, "the default survived the override");

        // …and the neighbour is untouched.
        let terminal = resolved(&loaded, "terminal.toggle").expect("an untouched default");
        assert_eq!(terminal.shortcut.key, "j");
        assert!(terminal.shortcut.mod_key);
        assert_eq!(loaded.keybindings.len(), defaults().len());
    }

    /// A file that is not JSON costs its contents and not the app. The
    /// criterion this exists for is ticket 22's "falls back to defaults with a
    /// warning rather than failing to start".
    #[test]
    fn a_corrupt_file_falls_back_to_the_defaults_with_a_warning() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(directory.path().join(FILE), "{ not json at all").expect("writes");

        let loaded = load(directory.path());
        assert_eq!(loaded.keybindings.len(), defaults().len());
        assert_eq!(loaded.issues.len(), 1, "{:?}", loaded.issues);
        assert_eq!(
            loaded.issues[0].kind, "keybindings.malformed-config",
            "the kind has to be one of the contract's two literals, or it costs the              whole config payload"
        );
        assert_eq!(loaded.issues[0].index, None);
        assert!(
            loaded.issues[0].message.contains("not valid JSON"),
            "{}",
            loaded.issues[0].message
        );
    }

    /// One bad entry costs its own row rather than the whole file — the same
    /// rule the working tree status follows for a truncated record.
    #[test]
    fn one_unreadable_entry_costs_itself_and_not_the_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        std::fs::write(
            directory.path().join(FILE),
            r#"[
                {"key": "ctrl+alt+s", "command": "sidebar.toggle"},
                {"key": "a+b", "command": "terminal.toggle"},
                {"key": "ctrl+alt+t", "command": "diff.toggle"}
            ]"#,
        )
        .expect("writes the file");

        let loaded = load(directory.path());
        assert_eq!(loaded.issues.len(), 1, "{:?}", loaded.issues);
        assert_eq!(loaded.issues[0].kind, "keybindings.invalid-entry");
        assert_eq!(
            loaded.issues[0].index,
            Some(1),
            "the index is what lets the UI point at the line rather than the file"
        );

        // The two either side of it are bound.
        assert!(resolved(&loaded, "sidebar.toggle").expect("bound").shortcut.ctrl_key);
        assert!(resolved(&loaded, "diff.toggle").expect("bound").shortcut.ctrl_key);
        // …and the one that did not read keeps its default.
        assert!(resolved(&loaded, "terminal.toggle").expect("bound").shortcut.mod_key);
    }

    /// Binding, rebinding and unbinding, against a real file — the three
    /// operations the ticket names, and the round trip through disk that makes
    /// them survive a restart.
    #[test]
    fn a_binding_can_be_added_changed_and_removed() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let added = Rule::new("ctrl+alt+s", "sidebar.toggle");

        let loaded = rebind(directory.path(), |rules| rules.push(added.clone())).expect("binds");
        assert_eq!(loaded.rules, vec![added.clone()]);
        assert_eq!(
            resolved(&loaded, "sidebar.toggle").expect("bound").shortcut.key,
            "s"
        );

        // Changed: the old rule goes with the new one arriving, or both fire.
        let changed = Rule::new("ctrl+alt+z", "sidebar.toggle");
        let loaded = rebind(directory.path(), |rules| {
            rules.retain(|rule| !rule.is(&added));
            rules.push(changed.clone());
        })
        .expect("rebinds");
        assert_eq!(loaded.rules, vec![changed.clone()]);
        assert_eq!(
            resolved(&loaded, "sidebar.toggle").expect("bound").shortcut.key,
            "z"
        );

        // Removed: the default comes back, because it was never gone — it was
        // shadowed by command.
        let loaded = rebind(directory.path(), |rules| rules.retain(|rule| !rule.is(&changed)))
            .expect("unbinds");
        assert!(loaded.rules.is_empty());
        let sidebar = resolved(&loaded, "sidebar.toggle").expect("the default is back");
        assert!(sidebar.shortcut.mod_key && sidebar.shortcut.key == "b");
    }

    /// What is written is what is read back. The file is the developer's own
    /// document and a round trip that changed it would be this server editing
    /// something it does not own.
    #[test]
    fn what_is_written_is_read_back_unchanged() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let rules = vec![
            Rule::new("ctrl+alt+s", "sidebar.toggle"),
            Rule::when("ctrl+alt+d", "diff.toggle", "!terminalFocus"),
        ];

        save(directory.path(), &rules).expect("writes");
        assert_eq!(load(directory.path()).rules, rules);
    }

    /// **The defaults are not written down.** Only what the developer chose is,
    /// or tomorrow's defaults would never reach a machine that had opened the
    /// app today.
    #[test]
    fn the_defaults_are_never_written_to_the_developers_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        rebind(directory.path(), |rules| {
            rules.push(Rule::new("ctrl+alt+s", "sidebar.toggle"))
        })
        .expect("binds");

        let written = std::fs::read_to_string(directory.path().join(FILE)).expect("the file");
        assert!(written.contains("sidebar.toggle"), "{written}");
        assert!(
            !written.contains("terminal.toggle"),
            "a default was written into the developer's file: {written}"
        );
    }

    /// A rule the file could not hold is refused before it reaches the file,
    /// under the tag the client decodes.
    #[test]
    fn a_binding_that_cannot_compile_is_refused_by_name() {
        for payload in [
            json!({"key": "a+b", "command": "sidebar.toggle"}),
            json!({"key": "", "command": "sidebar.toggle"}),
            json!({"key": "mod+b", "command": "  "}),
            json!({"key": "mod+b", "command": "sidebar.toggle", "when": "1focus"}),
        ] {
            let error = Upsert::read(&payload, Path::new(".")).expect_err("a refusal");
            assert_eq!(error["_tag"], ERROR, "{payload}");
            assert!(error["detail"].is_string(), "{payload}");
        }
    }

    /// A blank `when` is no condition, not a condition nothing satisfies. The
    /// contract types it as a non-empty string, so a client sending `""` means
    /// the former.
    #[test]
    fn a_blank_condition_is_no_condition() {
        let read = Upsert::read(
            &json!({"key": "mod+b", "command": "sidebar.toggle", "when": "   "}),
            Path::new("."),
        )
        .expect("well formed");
        assert_eq!(read.rule.when, None);
    }
}
