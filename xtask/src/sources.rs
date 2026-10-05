//! The guardrail against code that hides from Clippy.
//!
//! Clippy checks only the code it compiles, and code can choose with conditions (`#[cfg(…)]`, `#[cfg_attr(…)]`, `cfg!(…)`) the configurations in which it is compiled at all. Code under a condition that the Clippy step never sets, or never leaves unset, is compiled by the tests or in a release, but Clippy never sees it, so none of the bans of `clippy.toml` (ADR-0005) apply to it: `#[cfg(not(feature = "x"))] fn sine(x: f64) -> f64 { x.sin() }`, next to a `#[cfg(feature = "x")]` twin that Clippy checks instead, would pass every other step of the gate. Code may therefore use only conditions that the Clippy step checks both ways, written out in full where this check reads them.
//!
//! # Conditions
//!
//! A condition is the argument of `cfg(…)` and `cfg!(…)`, and the first argument of `cfg_attr(…)`. It may use only the approved conditions (`APPROVED_CONDITIONS` in `verify.rs`: `test`, `debug_assertions` and `target_arch = "wasm32"`), combined with `all`, `any`, `not`, `true` and `false`; the lint canary's own crate may also use its switch, `bayan_lint_canary = "…"`. The Clippy step compiles the code under every combination of the approved conditions (`CLIPPY_RUNS` in `verify.rs`), so Clippy sees the code under any condition built from them, and this check needs no reasoning about `not`.
//!
//! Every condition is written out in full where this check reads it:
//!
//! - It may not contain a macro's metavariable (`$`): a macro handed `clippy::disallowed_methods` could otherwise keep only `clippy` and write `#[cfg(not($tool))]`.
//! - The words `cfg` and `cfg_attr` may stand only directly before their condition, as `cfg(`, `cfg!(` and `cfg_attr(`, so that no macro can assemble a condition from pieces, such as `#[$name(not(feature = "x"))]` handed `cfg`.
//! - Every other word that starts with `cfg` is rejected: `cfg_select!` chooses code by conditions that are not written inside `cfg(…)`, and later Rust versions may add more such macros.
//! - The bare identifier `clippy` (not followed by `::`) is rejected anywhere outside comments, strings and character literals, also as a macro argument: Clippy sets the condition `clippy` when it compiles a crate, and a normal build does not. Every lint path starts with `clippy::`, so the bare identifier has no legitimate use.
//!
//! # Attributes that macros write
//!
//! A `macro_rules!` macro could write any attribute from what it is handed, such as `#[$attribute] mod sine;` handed `path = "…"`, which makes the compiler read a file that this check never sees. Outside the patterns that a macro's input must match (its matchers), a metavariable may therefore appear in an attribute only to pass on an attribute exactly as it was written where the macro is called, such as a doc comment, which this check reads there: the matcher binds it with `#[$name:meta]` and the macro writes it as `#[$name]`.
//!
//! # The files it reads
//!
//! The check is only as good as the files it reads. It reads every `.rs` file below every member's folder, and it rejects every way it knows to make the compiler read another Rust file for a workspace member:
//!
//! - The only folder the walk skips is the workspace's build folder, `<root>/target`, and no member's folder may contain that.
//! - Every target's root file (`src_path` in `cargo metadata`, which a path in `[lib]`, `[[bin]]`, `[[test]]`, `[[bench]]` or `[[example]]`, or `build = "…"`, can move) must be a `.rs` file inside its crate's folder. From there, `mod name;` reaches only `name.rs` or `name/mod.rs` below it.
//! - `include!` is rejected: the identifier `include` anywhere, so that a macro cannot be handed it either. `include_str!` and `include_bytes!` read data, not code, and stay allowed.
//! - So is the attribute `#[path = "…"]`, wherever `path =` stands inside an attribute, also in `cfg_attr`, and, by the rule above, any attribute that a macro builds from its arguments.
//! - So are a symbolic link in a member's folder, and a file that the file system also finds under a name ending in `.rs` although its own name does not end so, as Windows and macOS give the compiler `sine.RS` when it asks for `sine.rs`.
//!
//! Procedural macros can write any code without it appearing in a source file; the preflight rejects every procedural-macro crate in the workspace that xtask does not list (`PROC_MACROS` in `verify.rs`).
//!
//! Source that the check cannot follow, such as a comment, string or bracket that is never closed, or a condition or `macro_rules!` definition it cannot read, is an error, so that nothing can hide behind a misreading.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use crate::policy::Package;

/// The identifier that Clippy sets as a condition.
const CLIPPY: &str = "clippy";

/// The macro that compiles another file as if its text stood in this one.
const INCLUDE: &str = "include";

/// The attribute that makes `mod` read its file from another place.
const PATH: &str = "path";

/// The start of every word that writes a condition: `cfg`, `cfg_attr`, and macros such as `cfg_select!`.
const CONDITION_WORD: &str = "cfg";

/// The workspace's build folder, relative to the root: the only folder the walk skips.
const BUILD_FOLDER: &str = "target";

/// A condition that code may use in `cfg(…)`, `cfg!(…)` and the first argument of `cfg_attr(…)`: a name, used alone or compared with a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Condition {
    /// The name, such as `test` or `target_arch`.
    pub name: &'static str,
    /// The value it is compared with, as in `target_arch = "wasm32"`, or `None` for a name used alone, such as `test`.
    pub value: Option<&'static str>,
    /// Why it is approved.
    pub reason: &'static str,
}

impl Condition {
    /// The condition as code writes it, such as `target_arch = "wasm32"`.
    pub fn written(&self) -> String {
        match self.value {
            Some(value) => format!("{} = \"{value}\"", self.name),
            None => self.name.to_owned(),
        }
    }
}

/// The conditions that code may use: the approved ones, and the lint canary's switch, which only the canary's own crate may use, with any value.
#[derive(Debug, Clone, Copy)]
pub struct Allowed<'a> {
    /// The approved conditions (`APPROVED_CONDITIONS` in `verify.rs`).
    pub conditions: &'a [Condition],
    /// The name of the lint canary's switch.
    pub canary_switch: &'a str,
    /// The package that may use the switch.
    pub canary_package: &'a str,
}

/// How each kind of problem is reported: the heading of its list of places, and why it is a problem.
const MISPLACED_ROOTS: (&str, &str) = (
    "the compiler would start from a file that this check does not read",
    "A path in [lib], [[bin]], [[test]], [[bench]] or [[example]] of a crate's Cargo.toml, or `build = \"…\"`, moves the file the compiler starts from, and the compiler then reads every module below that file. This check reads the `.rs` files below each member's folder and skips only the workspace's build folder, target/. Keep every target's root file inside its crate's folder, with the extension `.rs` (Cargo's defaults, such as src/lib.rs, always are), and keep crates out of the folder that holds target/.",
);
const UNREAD_ENTRIES: (&str, &str) = (
    "a member's folder holds entries that the compiler can read but this check does not",
    "The check reads every file whose name ends in `.rs` below the members' folders. It does not follow symbolic links, which can point to a file anywhere, and it skips other names, although a file system that ignores letter case, as on Windows and macOS, gives the compiler `sine.RS` when it asks for `sine.rs`. Replace a link with the file it points to, and give every Rust file the extension `.rs`, in lower case.",
);
const INCLUDE_USED: (&str, &str) = (
    "`include!` (the identifier `include`) is used here",
    "`include!` compiles another file as if its text stood here. That file can lie anywhere and have any name, so this check would not read it. The identifier is rejected anywhere outside comments and strings, so that a macro cannot be handed it either. Make the file a module instead (`mod name;`, with name.rs next to the file that declares it); `include_str!` and `include_bytes!`, which read data, not code, are fine.",
);
const PATH_USED: (&str, &str) = (
    "the attribute `#[path = \"…\"]` is used here",
    "`#[path]` makes `mod` read its file from anywhere and under any name, so this check would not read it. It is rejected wherever `path =` stands inside an attribute, also inside `cfg_attr`. Declare the module with a plain `mod name;` and keep its file where the compiler looks by default (name.rs or name/mod.rs next to the file that declares it).",
);
const CLIPPY_USED: (&str, &str) = (
    "the bare identifier `clippy` (not followed by `::`) is used here",
    "Clippy sets the condition `clippy` when it compiles a crate, and a normal build does not, so code under `#[cfg(not(clippy))]` is compiled and shipped but never checked by Clippy: the bans of clippy.toml (ADR-0005) do not apply to it. The gate therefore rejects the identifier anywhere outside comments and strings, also inside `cfg_attr` and `cfg!` and as a macro argument. Lint paths such as `clippy::unwrap_used` are fine. Remove the condition, or rename an item or variable that happens to be called `clippy`.",
);
const UNAPPROVED_CONDITIONS: &str =
    "a condition outside the approved list, or one this check cannot read, is used here";
const MACRO_CONDITIONS: (&str, &str) = (
    "a condition is built from a macro's arguments here (it contains a metavariable, `$`)",
    "A macro can be handed anything, so a condition it builds from its arguments is not written out where this check reads it: `#[cfg(not($tool))]`, handed the lint path `clippy::disallowed_methods`, keeps only `clippy` and hides the code from Clippy. Write every condition out in full, also inside macros.",
);
const CONDITION_WORDS: (&str, &str) = (
    "the word `cfg` or `cfg_attr` is used here without its condition directly after it, or another word that starts with `cfg`",
    "`cfg` must be followed directly by `(`, as in `#[cfg(…)]`, or by `!(`, as in `cfg!(…)`, and `cfg_attr` by `(`, so that no macro can assemble a condition from pieces, such as `#[$name(not(feature = \"x\"))]` handed `cfg`. Every other word that starts with `cfg` is rejected too: `cfg_select!` chooses code by conditions that are not written inside `cfg(…)`, which this check would not read, and later Rust versions may add more such macros. Write conditions as `#[cfg(…)]`, `#[cfg_attr(…, …)]` or `cfg!(…)`, and do not give anything a name that starts with `cfg`.",
);
const MACRO_ATTRIBUTES: (&str, &str) = (
    "an attribute is built from a macro's arguments here",
    "A macro can be handed anything, so an attribute that it builds from its arguments, such as `#[$attribute] mod sine;` handed `path = \"…\"`, can make the compiler read a file or choose code that this check never sees. Write every attribute out in full inside a macro. The one exception passes on an attribute exactly as it was written where the macro is called, such as a doc comment, where this check reads it: bind it with `$(#[$name:meta])*` in the macro's pattern and write it as `$(#[$name])*`.",
);

/// Why conditions outside the approved list are rejected, naming the approved ones.
fn unapproved_why(allowed: &Allowed<'_>) -> String {
    let approved: Vec<String> = allowed
        .conditions
        .iter()
        .map(|condition| format!("`{}`", condition.written()))
        .collect();
    format!(
        "Code may choose the configurations in which it is compiled only with the approved conditions, {}, combined with `all`, `any`, `not`, `true` and `false` (and, in the {} crate only, its switch `{} = \"…\"`). The Clippy step compiles the code under every combination of their values, so no code under them escapes Clippy. Code under any other condition, such as `feature = \"…\"`, `windows`, `unix`, `target_os = \"…\"` or a name of your own, is compiled in configurations that Clippy does not check, where the bans of clippy.toml (ADR-0005) would not apply. Remove the condition, or write it with the approved ones. A condition can be added to APPROVED_CONDITIONS in xtask/src/verify.rs only in a reviewed pull request, together with the Clippy runs (CLIPPY_RUNS) that check the code with it both on and off.",
        approved.join(", "),
        allowed.canary_package,
        allowed.canary_switch,
    )
}

/// A token of Rust source, as far as this check needs to tell them apart, and the line on which it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Token<'a> {
    kind: Kind<'a>,
    line: usize,
}

/// The kinds of tokens this check tells apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind<'a> {
    /// An identifier, with `r#` removed from a raw identifier.
    Ident(&'a str),
    /// The path separator `::`.
    PathSeparator,
    /// One other ASCII punctuation character, such as `#`, `[`, `=` or `$`.
    Punct(u8),
    /// A string literal of any kind (plain, raw, byte or C string), exactly as written, prefix and quotes included, such as `"wasm32"`.
    Str(&'a str),
    /// A character or byte literal, a number, a lifetime or label, or other text.
    Other,
}

/// What the check found in one file, as the lines (counting from 1) of each finding.
#[derive(Debug, Default, PartialEq, Eq)]
struct Findings {
    /// The bare identifier `clippy`, not followed by `::`.
    bare_clippy: Vec<usize>,
    /// The identifier `include`.
    include_macro: Vec<usize>,
    /// `path =` inside an attribute.
    path_attribute: Vec<usize>,
    /// Conditions outside the approved list, and conditions this check cannot read: the line and what was found.
    conditions: Vec<(usize, String)>,
    /// Conditions that contain a macro's metavariable (`$`).
    macro_conditions: Vec<usize>,
    /// The words `cfg` and `cfg_attr` without their condition directly after them, and other words that start with `cfg`: the line and the word.
    condition_words: Vec<(usize, String)>,
    /// Attributes that a macro builds from its arguments, other than one passed on as written where the macro is called.
    macro_attributes: Vec<usize>,
}

/// Finds in Rust source everything this check rejects: the bare identifier `clippy` (not followed by `::`), the identifier `include`, `path =` inside an attribute (`#[…]` or `#![…]`, at any depth, so also inside `cfg_attr`), conditions outside the approved list (`allowed`; the canary's switch only where `in_canary` says the file belongs to the lint canary's crate), conditions that contain a metavariable, words starting with `cfg` that do not stand directly before their condition, and attributes that a macro builds from its arguments. Comments, strings (also raw, byte and C strings), character literals and lifetimes are skipped. Source the reader cannot follow is an error, so nothing can hide behind a misreading.
fn findings(source: &str, allowed: &Allowed<'_>, in_canary: bool) -> Result<Findings, String> {
    let tokens = tokenize(source)?;
    let closing = closing_brackets(&tokens)?;
    let attributes = attributes(&tokens, &closing);
    let rules = rules_of_macros(&tokens, &closing, &attributes)?;
    let mut in_attribute = vec![false; tokens.len()];
    for attribute in &attributes {
        for flag in &mut in_attribute[attribute.content.clone()] {
            *flag = true;
        }
    }
    let mut found = Findings::default();
    // Which tokens belong to a condition, so that a metavariable in a condition is reported once, as a condition.
    let mut in_condition = vec![false; tokens.len()];
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1).map(|token| token.kind);
        match token.kind {
            Kind::Ident(CLIPPY) if next != Some(Kind::PathSeparator) => {
                found.bare_clippy.push(token.line);
            }
            Kind::Ident(INCLUDE) => found.include_macro.push(token.line),
            Kind::Ident(PATH) if in_attribute[index] && next == Some(Kind::Punct(b'=')) => {
                found.path_attribute.push(token.line);
            }
            Kind::Ident(word) if word.starts_with(CONDITION_WORD) => {
                if let Some(condition) =
                    check_condition(&tokens, &closing, index, allowed, in_canary, &mut found)
                {
                    for flag in &mut in_condition[condition] {
                        *flag = true;
                    }
                }
            }
            _ => {}
        }
    }
    for attribute in &attributes {
        let built = attribute
            .content
            .clone()
            .any(|at| tokens[at].kind == Kind::Punct(b'$') && !in_condition[at]);
        // A matcher is a pattern for the macro's input, never code, so its attributes are not checked.
        let in_matcher = rules
            .iter()
            .any(|rule| rule.matcher.contains(&attribute.hash));
        if built && !in_matcher && !passed_on(attribute, &tokens, &rules) {
            found.macro_attributes.push(tokens[attribute.hash].line);
        }
    }
    Ok(found)
}

/// Checks the word that starts with `cfg` at `tokens[index]`: `cfg` and `cfg_attr` must stand directly before their condition, which may contain no metavariable and may use only the allowed conditions; every other such word is reported. Returns the positions of the condition's tokens, if the word has one.
fn check_condition(
    tokens: &[Token<'_>],
    closing: &[Option<usize>],
    index: usize,
    allowed: &Allowed<'_>,
    in_canary: bool,
    found: &mut Findings,
) -> Option<Range<usize>> {
    let Token {
        kind: Kind::Ident(word),
        line,
    } = tokens[index]
    else {
        return None;
    };
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    // The `(` that opens the condition: `cfg(`, `cfg!(` or `cfg_attr(`.
    let open = match word {
        "cfg" | "cfg_attr" if kind(index + 1) == Some(Kind::Punct(b'(')) => index + 1,
        "cfg"
            if kind(index + 1) == Some(Kind::Punct(b'!'))
                && kind(index + 2) == Some(Kind::Punct(b'(')) =>
        {
            index + 2
        }
        _ => {
            found.condition_words.push((line, word.to_owned()));
            return None;
        }
    };
    let unreadable = || (line, "a condition this check cannot read".to_owned());
    let Some(close) = closing.get(open).copied().flatten() else {
        found.conditions.push(unreadable());
        return None;
    };
    // The condition is everything between the brackets of `cfg`; for `cfg_attr`, whose other arguments are attributes, it ends at the first comma outside nested brackets.
    let end = if word == "cfg_attr" {
        top_level_comma(tokens, closing, open + 1, close).unwrap_or(close)
    } else {
        close
    };
    let condition = open + 1..end;
    if tokens[condition.clone()]
        .iter()
        .any(|token| token.kind == Kind::Punct(b'$'))
    {
        found.macro_conditions.push(line);
        return Some(condition);
    }
    let mut names = Vec::new();
    if predicate(tokens, closing, condition.start, end, &mut names) != Some(end) {
        found.conditions.push(unreadable());
        return Some(condition);
    }
    for name in &names {
        if let Some(problem) = unapproved(name, allowed, in_canary) {
            found.conditions.push((name.line, problem));
        }
    }
    Some(condition)
}

/// A name that a condition tests, with the value it compares it with, as written (prefix and quotes included), and the line it is on.
struct Name<'a> {
    name: &'a str,
    value: Option<&'a str>,
    line: usize,
}

/// Reads one condition that starts at `tokens[at]` and ends before `end`: `all(…)`, `any(…)`, `not(…)`, `true`, `false`, a name, or a name compared with a string. Collects the names it tests into `names` and returns the position after the condition, or `None` if it cannot read the condition.
fn predicate<'a>(
    tokens: &[Token<'a>],
    closing: &[Option<usize>],
    at: usize,
    end: usize,
    names: &mut Vec<Name<'a>>,
) -> Option<usize> {
    let kind = |at: usize| (at < end).then(|| tokens[at].kind);
    let line = tokens.get(at)?.line;
    match kind(at)? {
        Kind::Ident(combinator @ ("all" | "any" | "not"))
            if kind(at + 1) == Some(Kind::Punct(b'(')) =>
        {
            let close = closing[at + 1]?;
            let mut position = at + 2;
            let mut count = 0;
            while position < close {
                position = predicate(tokens, closing, position, close, names)?;
                count += 1;
                if position < close {
                    if tokens[position].kind != Kind::Punct(b',') {
                        return None;
                    }
                    position += 1;
                }
            }
            (combinator != "not" || count == 1).then_some(close + 1)
        }
        Kind::Ident("true" | "false") => Some(at + 1),
        Kind::Ident(name) if kind(at + 1) == Some(Kind::Punct(b'=')) => {
            let Some(Kind::Str(value)) = kind(at + 2) else {
                return None;
            };
            names.push(Name {
                name,
                value: Some(value),
                line,
            });
            Some(at + 3)
        }
        Kind::Ident(name) => {
            names.push(Name {
                name,
                value: None,
                line,
            });
            Some(at + 1)
        }
        _ => None,
    }
}

/// Why `name` may not be tested here, or `None` if it may.
fn unapproved(name: &Name<'_>, allowed: &Allowed<'_>, in_canary: bool) -> Option<String> {
    let approved = allowed.conditions.iter().any(|condition| {
        condition.name == name.name
            && match (condition.value, name.value) {
                (None, None) => true,
                (Some(expected), Some(written)) => written == format!("\"{expected}\""),
                _ => false,
            }
    });
    let canary = name.name == allowed.canary_switch && name.value.is_some();
    if approved || (canary && in_canary) {
        return None;
    }
    let written = match name.value {
        Some(value) => format!("`{} = {value}`", name.name),
        None => format!("`{}`", name.name),
    };
    Some(if name.name == allowed.canary_switch {
        format!(
            "{written} (the lint canary's switch, which only the {} crate may use, with a value)",
            allowed.canary_package
        )
    } else {
        written
    })
}

/// The position of the first comma between `start` and `end` that is not inside nested brackets.
fn top_level_comma(
    tokens: &[Token<'_>],
    closing: &[Option<usize>],
    start: usize,
    end: usize,
) -> Option<usize> {
    let mut at = start;
    while at < end {
        match tokens[at].kind {
            Kind::Punct(b',') => return Some(at),
            Kind::Punct(b'(' | b'[' | b'{') => at = closing[at]? + 1,
            _ => at += 1,
        }
    }
    None
}

/// An attribute, `#[…]` or `#![…]`.
struct Attribute {
    /// The position of its `#`.
    hash: usize,
    /// The positions of the tokens between its brackets.
    content: Range<usize>,
}

/// Every attribute in `tokens`, also those nested in other attributes or in macros.
fn attributes(tokens: &[Token<'_>], closing: &[Option<usize>]) -> Vec<Attribute> {
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    let mut found = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != Kind::Punct(b'#') {
            continue;
        }
        let bracket = if kind(index + 1) == Some(Kind::Punct(b'!')) {
            index + 2
        } else {
            index + 1
        };
        if kind(bracket) == Some(Kind::Punct(b'['))
            && let Some(close) = closing[bracket]
        {
            found.push(Attribute {
                hash: index,
                content: bracket + 1..close,
            });
        }
    }
    found
}

/// A rule of a `macro_rules!` definition.
struct Rule<'a> {
    /// The positions of the tokens of the pattern that the macro's input must match (the matcher), brackets excluded.
    matcher: Range<usize>,
    /// The positions of the tokens of the code the rule writes (the transcriber), brackets excluded.
    transcriber: Range<usize>,
    /// The metavariables that the matcher binds to a whole attribute written where the macro is called: `name` in `#[$name:meta]` or `#![$name:meta]`.
    attributes: BTreeSet<&'a str>,
}

/// Every rule of every `macro_rules!` definition in `tokens`, also of definitions inside other macros. A definition that this reader cannot follow is an error, so that no attribute can hide in it. (The word `macro_rules` without `!` defines nothing, for the compiler as here. Reading too few definitions could only make the check stricter: an attribute outside every rule may not be built from a metavariable at all.)
fn rules_of_macros<'a>(
    tokens: &[Token<'a>],
    closing: &[Option<usize>],
    attributes: &[Attribute],
) -> Result<Vec<Rule<'a>>, String> {
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    let mut rules = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.kind != Kind::Ident("macro_rules") || kind(index + 1) != Some(Kind::Punct(b'!')) {
            continue;
        }
        let unreadable = || {
            format!(
                "line {}: a `macro_rules!` definition this check cannot read",
                token.line
            )
        };
        // `macro_rules! name { … }`, or with `( … );` or `[ … ];`. A macro that writes macros may name them with a metavariable, `$name`.
        let mut at = index + 2;
        if kind(at) == Some(Kind::Punct(b'$')) {
            at += 1;
        }
        if !matches!(kind(at), Some(Kind::Ident(_))) {
            return Err(unreadable());
        }
        at += 1;
        let body_end = closing.get(at).copied().flatten().ok_or_else(unreadable)?;
        // The rules, `(matcher) => {transcriber}`, separated by `;`.
        at += 1;
        while at < body_end {
            let matcher_end = closing[at].ok_or_else(unreadable)?;
            if kind(matcher_end + 1) != Some(Kind::Punct(b'='))
                || kind(matcher_end + 2) != Some(Kind::Punct(b'>'))
            {
                return Err(unreadable());
            }
            let transcriber_start = matcher_end + 3;
            let transcriber_end = closing
                .get(transcriber_start)
                .copied()
                .flatten()
                .ok_or_else(unreadable)?;
            let matcher = at + 1..matcher_end;
            rules.push(Rule {
                attributes: attribute_bindings(tokens, attributes, &matcher),
                matcher,
                transcriber: transcriber_start + 1..transcriber_end,
            });
            at = transcriber_end + 1;
            if at < body_end {
                if kind(at) != Some(Kind::Punct(b';')) {
                    return Err(unreadable());
                }
                at += 1;
            }
        }
    }
    Ok(rules)
}

/// The tokens between an attribute's brackets.
fn content<'a>(attribute: &Attribute, tokens: &[Token<'a>]) -> Vec<Kind<'a>> {
    tokens[attribute.content.clone()]
        .iter()
        .map(|token| token.kind)
        .collect()
}

/// The names that the matcher at `matcher` binds to a whole attribute: `name` in `#[$name:meta]` or `#![$name:meta]`.
fn attribute_bindings<'a>(
    tokens: &[Token<'a>],
    attributes: &[Attribute],
    matcher: &Range<usize>,
) -> BTreeSet<&'a str> {
    attributes
        .iter()
        .filter(|attribute| matcher.contains(&attribute.hash))
        .filter_map(|attribute| match content(attribute, tokens)[..] {
            [
                Kind::Punct(b'$'),
                Kind::Ident(name),
                Kind::Punct(b':'),
                Kind::Ident("meta"),
            ] => Some(name),
            _ => None,
        })
        .collect()
}

/// Whether `attribute` is `#[$name]` or `#![$name]` in the code a macro rule writes, where the rule's matcher binds `$name` to a whole attribute written where the macro is called (`#[$name:meta]`): the attribute is passed on as written there, where this check reads it.
fn passed_on(attribute: &Attribute, tokens: &[Token<'_>], rules: &[Rule<'_>]) -> bool {
    let [Kind::Punct(b'$'), Kind::Ident(name)] = content(attribute, tokens)[..] else {
        return false;
    };
    // The innermost rule whose transcriber holds the attribute: rules nest when a macro writes a macro.
    rules
        .iter()
        .filter(|rule| rule.transcriber.contains(&attribute.hash))
        .max_by_key(|rule| rule.transcriber.start)
        .is_some_and(|rule| rule.attributes.contains(name))
}

/// For each opening bracket (`(`, `[` or `{`) in `tokens`, the position of the bracket that closes it. Rust itself requires brackets to be balanced, so brackets that are not mean this reader has lost track of the source, which is an error.
fn closing_brackets(tokens: &[Token<'_>]) -> Result<Vec<Option<usize>>, String> {
    let mut closing = vec![None; tokens.len()];
    let mut open: Vec<(usize, u8)> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        let Kind::Punct(byte) = token.kind else {
            continue;
        };
        let expected = match byte {
            b'(' => b')',
            b'[' => b']',
            b'{' => b'}',
            b')' | b']' | b'}' => {
                match open.pop() {
                    Some((start, expected)) if expected == byte => closing[start] = Some(index),
                    _ => {
                        return Err(format!(
                            "line {}: `{}` does not match the bracket it should close",
                            token.line,
                            char::from(byte)
                        ));
                    }
                }
                continue;
            }
            _ => continue,
        };
        open.push((index, expected));
    }
    match open.last() {
        Some((start, _)) => Err(format!(
            "line {}: a bracket is never closed",
            tokens[*start].line
        )),
        None => Ok(closing),
    }
}

/// Splits Rust source into the tokens this check needs.
fn tokenize(source: &str) -> Result<Vec<Token<'_>>, String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut position = 0;
    let mut line = 1;
    // Counts the line breaks in `source[from..to]`.
    let breaks = |from: usize, to: usize| source[from..to].matches('\n').count();
    while position < bytes.len() {
        let start = position;
        let byte = bytes[position];
        let rest = &source[position..];
        let kind = if rest.starts_with("//") {
            position += rest.find('\n').unwrap_or(rest.len());
            None
        } else if rest.starts_with("/*") {
            position = block_comment_end(source, position)
                .ok_or_else(|| format!("line {line}: a block comment is never closed"))?;
            None
        } else if byte == b'"' {
            position = string_end(source, position + 1)
                .ok_or_else(|| format!("line {line}: a string is never closed"))?;
            Some(Kind::Str(&source[start..position]))
        } else if byte == b'\'' {
            position = quote_end(source, position)
                .ok_or_else(|| format!("line {line}: a character literal is never closed"))?;
            Some(Kind::Other)
        } else if byte.is_ascii_alphabetic() || byte == b'_' {
            let word_end = ident_end(source, position);
            let word = &source[position..word_end];
            let after = &source[word_end..];
            let hashes = after.len() - after.trim_start_matches('#').len();
            let raw_prefix = matches!(word, "r" | "br" | "cr");
            if raw_prefix && after[hashes..].starts_with('"') {
                // A raw string: r"…", r#"…"#, br"…", cr"…".
                let body = word_end + hashes + 1;
                let closing = format!("\"{}", "#".repeat(hashes));
                let found = source[body..]
                    .find(&closing)
                    .ok_or_else(|| format!("line {line}: a raw string is never closed"))?;
                position = body + found + closing.len();
                Some(Kind::Str(&source[start..position]))
            } else if matches!(word, "b" | "c") && after.starts_with('"') {
                position = string_end(source, word_end + 1)
                    .ok_or_else(|| format!("line {line}: a string is never closed"))?;
                Some(Kind::Str(&source[start..position]))
            } else if word == "b" && after.starts_with('\'') {
                position = quote_end(source, word_end)
                    .ok_or_else(|| format!("line {line}: a byte literal is never closed"))?;
                Some(Kind::Other)
            } else if word == "r"
                && hashes == 1
                && after[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            {
                // A raw identifier, r#name.
                let name_end = ident_end(source, word_end + 1);
                position = name_end;
                Some(Kind::Ident(&source[word_end + 1..name_end]))
            } else {
                position = word_end;
                Some(Kind::Ident(word))
            }
        } else if byte.is_ascii_digit() {
            position = ident_end(source, position);
            Some(Kind::Other)
        } else if rest.starts_with("::") {
            position += 2;
            Some(Kind::PathSeparator)
        } else if byte.is_ascii_whitespace() {
            position += 1;
            None
        } else if byte.is_ascii() {
            position += 1;
            Some(Kind::Punct(byte))
        } else {
            // Non-ASCII text outside comments and strings; identifiers are ASCII (`non_ascii_idents` is denied).
            position += rest.chars().next().map_or(1, char::len_utf8);
            Some(Kind::Other)
        };
        if let Some(kind) = kind {
            tokens.push(Token { kind, line });
        }
        line += breaks(start, position);
    }
    Ok(tokens)
}

/// The end of the identifier or number that starts at `start`.
fn ident_end(source: &str, start: usize) -> usize {
    source[start..]
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .map_or(source.len(), |length| start + length)
}

/// The position after a block comment that starts at `start`; block comments nest.
fn block_comment_end(source: &str, start: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut position = start;
    while position < source.len() {
        let rest = &source[position..];
        if rest.starts_with("/*") {
            depth += 1;
            position += 2;
        } else if rest.starts_with("*/") {
            depth -= 1;
            position += 2;
            if depth == 0 {
                return Some(position);
            }
        } else {
            position += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    None
}

/// The position after the closing quote of a string whose opening quote ends at `start`.
fn string_end(source: &str, start: usize) -> Option<usize> {
    let mut characters = source[start..].char_indices();
    while let Some((offset, character)) = characters.next() {
        match character {
            '\\' => {
                characters.next();
            }
            '"' => return Some(start + offset + 1),
            _ => {}
        }
    }
    None
}

/// The position after a character literal (`'a'`, `'\n'`, `'\u{1F600}'`) or a lifetime or label (`'a`, `'static`) that starts with the quote at `start`.
fn quote_end(source: &str, start: usize) -> Option<usize> {
    let rest = &source[start + 1..];
    if let Some(escaped) = rest.strip_prefix('\\') {
        // An escape: everything up to the closing quote.
        let mut characters = escaped.char_indices();
        characters.next()?;
        let close = characters.find(|(_, c)| *c == '\'')?.0;
        return Some(start + 2 + close + 1);
    }
    let first = rest.chars().next()?;
    let after_first = start + 1 + first.len_utf8();
    if source[after_first..].starts_with('\'') {
        return Some(after_first + 1);
    }
    if first.is_ascii_alphabetic() || first == '_' {
        // A lifetime or label: the quote and the identifier.
        return Some(ident_end(source, start + 1));
    }
    None
}

/// Checks the Rust sources of the workspace members (`packages`, from `cargo metadata`): that the compiler is not made to read a Rust file of theirs that this check does not read, and that none of the files it reads uses the bare identifier `clippy`, a condition outside `allowed` or not written out in full, or an attribute that a macro builds from its arguments. Returns one line describing what was checked, or every problem found.
pub fn check(root: &Path, packages: &[Package], allowed: &Allowed<'_>) -> Result<String, String> {
    let build_folder = root.join(BUILD_FOLDER);
    let misplaced = misplaced_roots(root, &build_folder, packages);
    let mut walk = Walk::default();
    for package in packages {
        walk_folder(&package.folder, &build_folder, &mut walk)?;
    }
    let mut include_places = Vec::new();
    let mut path_places = Vec::new();
    let mut clippy_places = Vec::new();
    let mut condition_places = Vec::new();
    let mut macro_condition_places = Vec::new();
    let mut word_places = Vec::new();
    let mut macro_attribute_places = Vec::new();
    for file in &walk.files {
        let source = std::fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let shown = shown(root, file);
        let in_canary =
            owner(file, packages).is_some_and(|package| package.name == allowed.canary_package);
        let found = findings(&source, allowed, in_canary).map_err(|error| {
            format!("{shown}: {error}. The gate must be able to read every Rust file to check it.")
        })?;
        let places = |lines: &[usize]| {
            lines
                .iter()
                .map(|line| format!("  {shown}:{line}"))
                .collect::<Vec<_>>()
        };
        let described = |found: &[(usize, String)], show: fn(&str) -> String| {
            found
                .iter()
                .map(|(line, what)| format!("  {shown}:{line}: {}", show(what)))
                .collect::<Vec<_>>()
        };
        include_places.extend(places(&found.include_macro));
        path_places.extend(places(&found.path_attribute));
        clippy_places.extend(places(&found.bare_clippy));
        condition_places.extend(described(&found.conditions, str::to_owned));
        macro_condition_places.extend(places(&found.macro_conditions));
        word_places.extend(described(&found.condition_words, |word| {
            format!("`{word}`")
        }));
        macro_attribute_places.extend(places(&found.macro_attributes));
    }
    let unread: Vec<String> = walk
        .unread
        .iter()
        .map(|(entry, why)| format!("  {} ({why})", shown(root, entry)))
        .collect();
    let problems: Vec<String> = [
        (misplaced, MISPLACED_ROOTS.0, MISPLACED_ROOTS.1.to_owned()),
        (unread, UNREAD_ENTRIES.0, UNREAD_ENTRIES.1.to_owned()),
        (include_places, INCLUDE_USED.0, INCLUDE_USED.1.to_owned()),
        (path_places, PATH_USED.0, PATH_USED.1.to_owned()),
        (clippy_places, CLIPPY_USED.0, CLIPPY_USED.1.to_owned()),
        (
            condition_places,
            UNAPPROVED_CONDITIONS,
            unapproved_why(allowed),
        ),
        (
            macro_condition_places,
            MACRO_CONDITIONS.0,
            MACRO_CONDITIONS.1.to_owned(),
        ),
        (word_places, CONDITION_WORDS.0, CONDITION_WORDS.1.to_owned()),
        (
            macro_attribute_places,
            MACRO_ATTRIBUTES.0,
            MACRO_ATTRIBUTES.1.to_owned(),
        ),
    ]
    .into_iter()
    .filter(|(places, _, _)| !places.is_empty())
    .map(|(places, heading, why)| format!("{heading}:\n{}\n{why}", places.join("\n")))
    .collect();
    if problems.is_empty() {
        let approved: Vec<String> = allowed.conditions.iter().map(Condition::written).collect();
        return Ok(format!(
            "Rust sources: the {} `.rs` files below the members' folders use only the approved conditions ({}), written out in full, and no macro builds a condition or an attribute from its arguments; none uses the bare identifier `clippy`, `include!` or `#[path]`; every target starts from one of them, and no member's folder holds a symbolic link",
            walk.files.len(),
            approved.join(", ")
        ));
    }
    Err(problems.join("\n"))
}

/// The member that `file` belongs to: the one whose folder is the nearest above it, because members can lie inside each other's folders, as xtask/lint-canary lies inside xtask.
fn owner<'a>(file: &Path, packages: &'a [Package]) -> Option<&'a Package> {
    packages
        .iter()
        .filter(|package| file.starts_with(&package.folder))
        .max_by_key(|package| package.folder.components().count())
}

/// Describes every target whose root file the walk would not read, because it lies outside its crate's folder or does not end in `.rs`, and every member whose folder contains the build folder, which the walk skips.
fn misplaced_roots(root: &Path, build_folder: &Path, packages: &[Package]) -> Vec<String> {
    let build_folder = normalize(build_folder);
    let mut problems = Vec::new();
    for package in packages {
        let folder = normalize(&package.folder);
        if build_folder.starts_with(&folder) {
            problems.push(format!(
                "  {}: its folder, {}, contains the build folder target/",
                package.name,
                shown(root, &folder)
            ));
        }
        for target in &package.targets {
            let file = normalize(&target.root_file);
            let mut wrong = Vec::new();
            if !file.starts_with(&folder) {
                wrong.push(format!(
                    "lies outside the crate's folder, {}",
                    shown(root, &folder)
                ));
            }
            if !is_rust_file(&file) {
                wrong.push("does not end in `.rs`".to_owned());
            }
            if !wrong.is_empty() {
                problems.push(format!(
                    "  {}, {} target `{}`: {} {}",
                    package.name,
                    target.kinds.join(", "),
                    target.name,
                    shown(root, &file),
                    wrong.join(" and ")
                ));
            }
        }
    }
    problems
}

/// What the walk through the members' folders found.
#[derive(Debug, Default)]
struct Walk {
    /// Every `.rs` file.
    files: BTreeSet<PathBuf>,
    /// Every entry that the compiler can read although the check does not read it, with the reason.
    unread: BTreeMap<PathBuf, String>,
}

/// Adds every `.rs` file below `folder` to `walk.files`, and to `walk.unread` every entry that the compiler can read although the check would not: a symbolic link (which is not followed), anything that is neither a file nor a folder, and a file that the file system also finds under a name ending in `.rs` (see [`rust_alias`]). Skips `build_folder`.
fn walk_folder(folder: &Path, build_folder: &Path, walk: &mut Walk) -> Result<(), String> {
    let cannot_read =
        |path: &Path, error: std::io::Error| format!("cannot read {}: {error}", path.display());
    let mut rust_names = Vec::new();
    let mut other_files = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(|error| cannot_read(folder, error))? {
        let entry = entry.map_err(|error| cannot_read(folder, error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| cannot_read(&path, error))?;
        if file_type.is_symlink() {
            walk.unread.insert(path, "a symbolic link".to_owned());
        } else if file_type.is_dir() {
            if path != build_folder {
                walk_folder(&path, build_folder, walk)?;
            }
        } else if !file_type.is_file() {
            walk.unread
                .insert(path, "neither a file nor a folder".to_owned());
        } else if is_rust_file(&path) {
            rust_names.push(entry.file_name());
            walk.files.insert(path);
        } else {
            other_files.push(path);
        }
    }
    for path in other_files {
        if let Some(name) = rust_alias(&path, &rust_names, Path::is_file) {
            let why = format!(
                "the file system also finds it as {}",
                name.to_string_lossy()
            );
            walk.unread.insert(path, why);
        }
    }
    Ok(())
}

/// Whether the walk reads `path`: its name ends in `.rs`, which is what the compiler asks for when it looks for a module.
fn is_rust_file(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("rs"))
}

/// The name ending in `.rs` under which the file system also finds `file`, although that is not its name, if there is one. On the case-insensitive file systems of Windows and macOS, `mod sine;` makes the compiler read a file named `sine.RS`, which the walk, reading the names that end in `.rs`, would skip. `rust_names` are the names of the `.rs` files in the same folder, one of which the file system may have found instead; `is_file` asks the file system whether a file of a given name exists.
fn rust_alias(
    file: &Path,
    rust_names: &[OsString],
    is_file: impl Fn(&Path) -> bool,
) -> Option<OsString> {
    let mut name = file.file_stem()?.to_os_string();
    name.push(".rs");
    let found_another = rust_names
        .iter()
        .any(|rust_name| rust_name.eq_ignore_ascii_case(&name));
    (!found_another && is_file(&file.with_file_name(&name))).then_some(name)
}

/// `path` with `.` and `..` resolved from the path alone, without asking the file system. Cargo reports a target's root file as the package's folder joined with the path in the manifest, so `…/crates/bayan-units/../../shared/units.rs` would otherwise look like a file inside `crates/bayan-units`. For the absolute paths that Cargo reports, this gives the same result as the file system inside a member's folder, because the walk rejects symbolic links there.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other),
        }
    }
    normal
}

/// `path` relative to the root if it lies inside, as messages show it.
fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Target;
    use crate::verify::{APPROVED_CONDITIONS, CANARY_PACKAGE, CANARY_SWITCH};

    /// The conditions the gate allows.
    const ALLOWED: Allowed<'static> = Allowed {
        conditions: &APPROVED_CONDITIONS,
        canary_switch: CANARY_SWITCH,
        canary_package: CANARY_PACKAGE,
    };

    /// What the check finds in `source`, outside the lint canary's crate.
    fn found(source: &str) -> Findings {
        findings(source, &ALLOWED, false).unwrap_or_else(|problem| panic!("{problem}"))
    }

    /// A path relative to the root, built from separate parts, so that the expected messages match on Windows too.
    fn relative(parts: &[&str]) -> String {
        parts.iter().collect::<PathBuf>().display().to_string()
    }

    #[test]
    fn finds_every_way_to_hide_from_clippy() {
        let cases = [
            "#[cfg(not(clippy))]\npub fn sine(x: f64) -> f64 { x.sin() }",
            "#[cfg(clippy)]\nfn twin() {}",
            "#[cfg_attr(not(clippy), allow(dead_code))]",
            "if cfg!(clippy) {}",
            "m!(clippy);",
            "#[cfg(not(r#clippy))]",
            "#[cfg(any(feature = \"x\", clippy))]",
            "macro_rules! hide { () => { #[cfg(not(clippy))] fn f() {} } }",
            "let clippy = 1;",
            "fn clippy() {}",
            "#[cfg(not(clippy ))]",
            // A single colon is not a path.
            "struct S { clippy: u8 }",
        ];
        for source in cases {
            assert_eq!(found(&format!("\n{source}\n")).bare_clippy, [2], "{source}");
        }
    }

    #[test]
    fn finds_include_wherever_it_stands() {
        let cases = [
            "include!(\"sine.in\");",
            "std::include!(\"sine.in\");",
            "r#include!(\"sine.in\");",
            "use std::include as inline;",
            // Handed to a macro, which could call it.
            "m!(include);",
            "m!(include::sine);",
            "let include = 1;",
        ];
        for source in cases {
            assert_eq!(
                found(&format!("\n{source}\n")).include_macro,
                [2],
                "{source}"
            );
        }
    }

    #[test]
    fn finds_path_attributes_also_inside_cfg_attr() {
        let cases = [
            "#[path = \"../../../shared/sine.rs\"] mod sine;",
            "#![path = \"sine.in\"]",
            "#[cfg_attr(all(), path = \"sine.in\")] mod sine;",
            "#[cfg_attr(test, cfg_attr(all(), path = \"sine.in\"))] mod sine;",
            "#[cfg_attr(all(), allow(dead_code), path = \"sine.in\")] mod sine;",
            "# [ path= \"sine.in\" ] mod sine;",
            "#[r#path = \"sine.in\"] mod sine;",
            "#[doc = \"Sine.\"] #[path = \"target/sine.rs\"] mod sine;",
            "#[test_case([1, 2])] #[path = \"sine.in\"] mod sine;",
            "macro_rules! m { () => { #[path = \"sine.in\"] mod sine; } }",
        ];
        for source in cases {
            assert_eq!(
                found(&format!("\n{source}\n")).path_attribute,
                [2],
                "{source}"
            );
        }
        let spread = "#[cfg_attr(\n    all(),\n    path = \"sine.in\"\n)]\nmod sine;";
        assert_eq!(found(spread).path_attribute, [3]);
    }

    #[test]
    fn allows_path_outside_attributes() {
        let source = r#"
#[derive(Debug)]
struct Config { path: PathBuf }
#[expect(clippy::needless_pass_by_value, reason = "path = …")]
fn read(path: &Path, list: [u8; 2]) -> Config {
    let path = path.join("a");
    let mut other = path.clone();
    #[test_case([1, [2]])]
    other = path;
    path = other;
    let first = list[0];
    Config { path: other }
}
"#;
        assert_eq!(found(source), Findings::default());
    }

    #[test]
    fn allows_lint_paths_comments_and_strings() {
        let source = r##"
#![expect(clippy::print_stdout, reason = "the clippy step prints")]
#![doc = include_str!("../README.md")]
#[expect(clippy :: unwrap_used, reason = "spacing around :: is still a path")]
// A comment may say cfg(not(clippy)), cfg_select!, include!("x") or #[path = "x"].
/* A block comment /* nested */ may say clippy or #[cfg(windows)] too. */
/// So may documentation: `#[cfg(clippy)]`, `#[$attribute]`.
fn run() {
    let command = "cargo clippy -- -D warnings --cfg feature=\"x\"";
    let raw = r#"cfg(not(clippy)) #[path = "x"] include!("x")"#;
    let bytes = b"clippy";
    let raw_bytes = br"clippy";
    let c_string = c"clippy";
    let data = include_bytes!("data.bin");
    let quote = '"';
    let escaped = '\'';
    let byte = b'\'';
    let bracket = '(';
    let unicode = '\u{1F600}';
    let label: &'static str = "clippy";
    'outer: loop { break 'outer; }
    let clippy_config = 1; // a different identifier
    let not_clippy = clippy_config;
    let included = include_str!("data.txt");
    let config = not_clippy; // `cfg` only at the start of a word counts
}
"##;
        assert_eq!(found(source), Findings::default());
    }

    #[test]
    fn counts_lines_across_multi_line_comments_and_strings() {
        let source = "/* one\ntwo */ let s = \"a\nb\";\nr#\"x\ny\"#;\nclippy";
        assert_eq!(found(source).bare_clippy, [6]);
    }

    #[test]
    fn refuses_source_it_cannot_read() {
        for source in [
            "/* never closed",
            "/* nested /* only one */ closed",
            "\"never closed",
            "r#\"never closed\"",
            "let c = '",
            // Brackets that are never closed, or close nothing.
            "fn f() {",
            "fn f() ]",
            "fn f() { (}",
            // `macro_rules!` definitions this check cannot follow.
            "macro_rules! m",
            "macro_rules! { () => {} }",
            "macro_rules! m = 1;",
            "macro_rules! m { () }",
            "macro_rules! m { () => }",
            "macro_rules! m { () => {} () => {} }",
        ] {
            assert!(findings(source, &ALLOWED, false).is_err(), "{source:?}");
        }
        // Without `!`, the word defines no macro, for the compiler as for this check.
        assert_eq!(found("let macro_rules = 1;"), Findings::default());
    }

    #[test]
    fn accepts_the_approved_conditions_in_any_combination() {
        let source = r#"
#![cfg_attr(not(test), forbid(missing_docs))]
#[cfg(test)] mod tests {}
#[cfg(not(test))] fn a() {}
#[cfg(debug_assertions)] fn b() {}
#[cfg(not(debug_assertions))] fn b() {}
#[cfg(target_arch = "wasm32")] fn c() {}
#[cfg(all(test, not(debug_assertions), any(target_arch = "wasm32", not(target_arch = "wasm32"))))] fn d() {}
#[cfg(any())] fn e() {}
#[cfg(all())] fn f() {}
#[cfg(true)] fn g() {}
#[cfg(not(false))] fn h() {}
#[cfg(all(test, debug_assertions,))] fn i() {}
#[cfg_attr(test, derive(Debug))] struct S;
#[cfg_attr(all(test, target_arch = "wasm32"), cfg_attr(debug_assertions, inline))] fn j() {}
#[cfg_attr(not(debug_assertions), must_use, inline)] fn k() {}
fn l() -> bool { cfg!(debug_assertions) || std::cfg!(target_arch = "wasm32") || core::cfg ! (test) }
#[cfg (
    test
)] fn m() {}
#[cfg(/* a comment */ test)] fn n() {}
macro_rules! tests {
    ($($name:ident),*) => { $( #[cfg(test)] #[test] fn $name() {} )* };
}
"#;
        assert_eq!(found(source), Findings::default());
    }

    #[test]
    fn rejects_every_condition_outside_the_approved_list() {
        let cases = [
            ("#[cfg(feature = \"x\")]", "`feature = \"x\"`"),
            ("#[cfg(not(feature = \"x\"))]", "`feature = \"x\"`"),
            ("#[cfg(windows)]", "`windows`"),
            ("#[cfg(unix)]", "`unix`"),
            ("#[cfg(target_os = \"linux\")]", "`target_os = \"linux\"`"),
            (
                "#[cfg(target_arch = \"x86_64\")]",
                "`target_arch = \"x86_64\"`",
            ),
            (
                "#[cfg(target_arch = \"wasm64\")]",
                "`target_arch = \"wasm64\"`",
            ),
            ("#[cfg(target_arch)]", "`target_arch`"),
            // Written differently from the approved value, even if the compiler reads the same value.
            (
                "#[cfg(target_arch = r\"wasm32\")]",
                "`target_arch = r\"wasm32\"`",
            ),
            (
                "#[cfg(target_arch = \"wasm\\x332\")]",
                "`target_arch = \"wasm\\x332\"`",
            ),
            ("#[cfg(test = \"x\")]", "`test = \"x\"`"),
            (
                "#[cfg(debug_assertions = \"on\")]",
                "`debug_assertions = \"on\"`",
            ),
            ("#[cfg(miri)]", "`miri`"),
            ("#[cfg(doc)]", "`doc`"),
            ("#[cfg(doctest)]", "`doctest`"),
            ("#[cfg(bayan_hidden)]", "`bayan_hidden`"),
            ("#[cfg(clippy)]", "`clippy`"),
            ("#[cfg(all(test, not(windows)))]", "`windows`"),
            (
                "#[cfg(any(debug_assertions, feature = \"x\"))]",
                "`feature = \"x\"`",
            ),
            ("#[cfg_attr(windows, inline)]", "`windows`"),
            ("#[cfg_attr(test, cfg_attr(unix, inline))]", "`unix`"),
            ("#[cfg_attr(test, cfg(windows))]", "`windows`"),
            ("if cfg!(windows) {}", "`windows`"),
            ("#![cfg(unix)]", "`unix`"),
            ("m!(cfg(windows));", "`windows`"),
            (
                "#[cfg(bayan_lint_canary = \"disallowed_methods\")]",
                "the lint canary's switch",
            ),
        ];
        for (source, expected) in cases {
            let conditions = found(&format!("\n{source}\n")).conditions;
            assert_eq!(conditions.len(), 1, "{source}: {conditions:?}");
            assert_eq!(conditions[0].0, 2, "{source}: {conditions:?}");
            assert!(
                conditions[0].1.contains(expected),
                "{source}: {conditions:?}"
            );
        }
        // Every condition outside the list is reported, each on its own line.
        let several = "#[cfg(any(\n    windows,\n    unix,\n))]";
        let lines: Vec<usize> = found(several)
            .conditions
            .iter()
            .map(|(line, _)| *line)
            .collect();
        assert_eq!(lines, [2, 3]);
    }

    #[test]
    fn allows_the_canary_switch_only_in_the_canary_crate() {
        let source = "#![cfg_attr(\n    bayan_lint_canary = \"expect_disallowed_methods\",\n    expect(clippy::disallowed_methods, reason = \"canary: crate\")\n)]\n#[cfg(bayan_lint_canary = \"unsafe_block\")]\npub mod unsafe_block;\n";
        let in_canary = findings(source, &ALLOWED, true).unwrap();
        assert_eq!(in_canary, Findings::default());
        let elsewhere = findings(source, &ALLOWED, false).unwrap();
        let lines: Vec<usize> = elsewhere.conditions.iter().map(|(line, _)| *line).collect();
        assert_eq!(lines, [2, 5]);
        // In the canary too, the switch needs a value, and the other conditions are those of every crate.
        let wrong = findings(
            "#[cfg(bayan_lint_canary)]\n#[cfg(windows)]\n",
            &ALLOWED,
            true,
        )
        .unwrap();
        assert_eq!(wrong.conditions.len(), 2, "{wrong:?}");
    }

    #[test]
    fn rejects_conditions_that_macros_build() {
        let cases = [
            // The reviewer's macro: handed `clippy::disallowed_methods`, it keeps only `clippy` (row 26 of #4).
            "macro_rules! lint_exempt { ($tool:ident :: $lint:ident, $item:item, $twin:item) => { #[cfg(not($tool))] $item #[cfg($tool)] $twin }; }",
            "macro_rules! m { ($c:meta) => { #[cfg($c)] fn f() {} }; }",
            "macro_rules! m { ($c:meta) => { #[cfg_attr($c, inline)] fn f() {} }; }",
            "macro_rules! m { ($c:meta) => { if cfg!($c) {} }; }",
            "macro_rules! m { ($a:literal) => { #[cfg(target_arch = $a)] fn f() {} }; }",
            "macro_rules! m { ($($c:tt)*) => { #[cfg(all(test, $($c)*))] fn f() {} }; }",
            "#[cfg($crate)] fn f() {}",
        ];
        for source in cases {
            let found = found(source);
            assert!(!found.macro_conditions.is_empty(), "{source}: {found:?}");
            assert_eq!(found.conditions, [], "{source}");
            // Reported once, as a condition, even where the condition stands in an attribute.
            assert_eq!(found.macro_attributes, [], "{source}");
        }
        // A metavariable among the attributes of `cfg_attr` is not part of the condition; the rule for attributes rejects it.
        let attribute =
            found("macro_rules! m { ($a:meta) => { #[cfg_attr(test, $a)] fn f() {} }; }");
        assert_eq!(attribute.macro_conditions, []);
        assert_eq!(attribute.macro_attributes, [1]);
    }

    #[test]
    fn rejects_cfg_words_without_their_condition() {
        let cases = [
            ("m!(cfg);", "cfg"),
            ("m!(cfg, not(feature = \"x\"));", "cfg"),
            ("m!(cfg_attr);", "cfg_attr"),
            ("let cfg = 1;", "cfg"),
            ("struct S { cfg_attr: u8 }", "cfg_attr"),
            ("if cfg![test] {}", "cfg"),
            ("if cfg!{test} {}", "cfg"),
            ("#[cfg = \"test\"] fn f() {}", "cfg"),
            ("#[cfg] fn f() {}", "cfg"),
            ("#[r#cfg_attr] fn f() {}", "cfg_attr"),
            (
                "cfg_select! { windows => { fn f() {} } _ => { fn f() {} } }",
                "cfg_select",
            ),
            ("std::cfg_select! { _ => {} }", "cfg_select"),
            ("#[cfg_eval] fn f() {}", "cfg_eval"),
            ("let cfg_value = 1;", "cfg_value"),
        ];
        for (source, word) in cases {
            assert_eq!(
                found(&format!("\n{source}\n")).condition_words,
                [(2, word.to_owned())],
                "{source}"
            );
        }
    }

    #[test]
    fn refuses_conditions_it_cannot_read() {
        for source in [
            "#[cfg(test debug_assertions)]",
            "#[cfg()]",
            "#[cfg(1)]",
            "#[cfg(std::test)]",
            "#[cfg(not())]",
            "#[cfg(not(test, debug_assertions))]",
            "#[cfg(test, debug_assertions)]",
            "#[cfg(all(test debug_assertions))]",
            "#[cfg(all(, test))]",
            "#[cfg(target_arch = wasm32)]",
            "#[cfg_attr()]",
            "#[cfg_attr(, inline)]",
            // Functions named `cfg` or `cfg_attr` look like conditions to this check, and are rejected as unreadable ones.
            "fn cfg() {}",
            "fn cfg_attr(x: u8) {}",
        ] {
            assert_eq!(
                found(source).conditions,
                [(1, "a condition this check cannot read".to_owned())],
                "{source}"
            );
        }
    }

    #[test]
    fn rejects_attributes_that_macros_build_from_their_arguments() {
        let cases = [
            // Row 27 of #4: handed `path = "…"`.
            "macro_rules! module_at { ($attribute:meta) => { #[$attribute] pub mod sine; }; }",
            // Row 28 of #4: handed `path` and the file name.
            "macro_rules! module_at { ($name:ident, $file:literal) => { #[$name = $file] pub mod sine; }; }",
            "macro_rules! m { ($a:meta) => { #[cfg_attr(test, $a)] mod sine; }; }",
            "macro_rules! m { ($d:literal) => { #[doc = $d] pub fn f() {} }; }",
            "macro_rules! m { ($($a:tt)*) => { #[$($a)*] mod sine; }; }",
            // Patterns that match tokens of an attribute rather than a whole attribute can recombine them.
            "macro_rules! m { (#[$a:ident = $f:literal]) => { #[$a = $f] mod sine; }; }",
            "macro_rules! m { (#[$a:ident]) => { #[$a] mod sine; }; }",
            "macro_rules! m { ($(#[$($a:tt)*])*) => { $(#[$($a)*])* mod sine; }; }",
            // Bound to a whole attribute, but by another rule.
            "macro_rules! m { (#[$a:meta]) => {}; ($a:meta) => { #[$a] mod sine; }; }",
            // Bound to a whole attribute by an enclosing macro's rule, not by the rule that writes the attribute.
            "macro_rules! outer { (#[$a:meta]) => { macro_rules! inner { () => { #[$a] mod sine; } } }; }",
            // A metavariable outside every macro definition.
            "m! { #[$a] mod sine; }",
            // `$` handed to a macro that writes macros.
            "macro_rules! make { ($d:tt) => { macro_rules! inner { ($d m:meta) => { #[$d m] mod sine; } } }; }",
            "macro_rules! m { () => { #[$crate::attribute] fn f() {} }; }",
            "macro_rules! m { ($a:meta) => { #![$a] }; }",
        ];
        for source in cases {
            let found = found(source);
            assert_eq!(found.macro_attributes.len(), 1, "{source}: {found:?}");
        }
    }

    #[test]
    fn allows_attributes_passed_on_as_written_where_the_macro_is_called() {
        let source = r#"
macro_rules! unit {
    ($(#[$meta:meta])* $name:ident = $blu:expr) => {
        $(#[$meta])*
        pub const $name: i64 = $blu;
    };
    ($(#![$inner:meta])*) => { $(#![$inner])* };
}
unit! {
    /// One inch.
    #[doc(alias = "in")]
    INCH = 1_828_800
}
macro_rules! outer {
    ($(#[$m:meta])* $name:ident) => {
        macro_rules! inner { (#[$x:meta]) => { #[$x] pub struct $name; } }
        $(inner! { #[$m] })*
    };
}
"#;
        assert_eq!(found(source), Findings::default());
    }

    #[test]
    fn checks_attributes_where_the_macro_is_called() {
        // The attribute that a macro passes on is checked where it is written.
        let source = "unit! {\n    #[path = \"../../../shared/sine.rs\"]\n    #[cfg(windows)]\n    SINE = 1\n}\n";
        let found = found(source);
        assert_eq!(found.path_attribute, [2]);
        assert_eq!(found.conditions, [(3, "`windows`".to_owned())]);
    }

    /// A scratch folder for one test, removed again at the end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("bayan-xtask-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// Writes `text` to the file at `parts`, relative to the scratch folder, and returns its path.
        fn write(&self, parts: &[&str], text: &str) -> PathBuf {
            let path = parts
                .iter()
                .fold(self.0.clone(), |path, part| path.join(part));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        /// A member in the folder at `parts`, relative to the scratch folder, whose library starts from `root_file`, a path relative to the member's folder as it would be written in `[lib]`.
        fn member_at(&self, name: &str, parts: &[&str], root_file: &str) -> Package {
            let folder = parts
                .iter()
                .fold(self.0.clone(), |path, part| path.join(part));
            Package {
                name: name.to_owned(),
                targets: vec![Target {
                    kinds: vec!["lib".to_owned()],
                    name: name.to_owned(),
                    root_file: folder.join(root_file),
                }],
                folder,
            }
        }

        /// A member at `crates/<name>`.
        fn member(&self, name: &str, root_file: &str) -> Package {
            self.member_at(name, &["crates", name], root_file)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn checks_every_rust_file_below_the_members() {
        let scratch = Scratch::new("sources");
        scratch.write(&["crates", "a", "src", "lib.rs"], "//! A.\n");
        scratch.write(&["crates", "a", "tests", "t.rs"], "#[test]\nfn t() {}\n");
        // Only the workspace's build folder is skipped; any other folder named `target` is read.
        scratch.write(&["crates", "a", "src", "target", "sine.rs"], "//! Read.\n");
        scratch.write(
            &["crates", "a", "notes.txt"],
            "cfg(not(clippy)) include!(\"x\")\n",
        );
        // A member inside another one (like xtask/lint-canary) is read once.
        scratch.write(&["crates", "a", "inner", "src", "lib.rs"], "//! Inner.\n");
        let packages = [
            scratch.member("a", "src/lib.rs"),
            scratch.member("a/inner", "src/lib.rs"),
        ];
        let line =
            check(&scratch.0, &packages, &ALLOWED).unwrap_or_else(|problem| panic!("{problem}"));
        assert!(line.contains("the 4 `.rs` files"), "{line}");
        assert!(
            line.contains(
                "use only the approved conditions (test, debug_assertions, target_arch = \"wasm32\")"
            ),
            "{line}"
        );

        scratch.write(
            &["crates", "a", "src", "target", "hidden.rs"],
            "\n#[cfg(not(clippy))]\npub fn sine(x: f64) -> f64 { x.sin() }\n",
        );
        let problem = check(&scratch.0, &packages, &ALLOWED).unwrap_err();
        let shown = format!(
            "  {}:2",
            relative(&["crates", "a", "src", "target", "hidden.rs"])
        );
        assert!(problem.contains(&shown), "{problem}");
        assert!(problem.contains("never checked by Clippy"), "{problem}");
    }

    #[test]
    fn reports_every_problem_at_once() {
        let scratch = Scratch::new("everything");
        scratch.write(
            &["crates", "a", "src", "lib.rs"],
            "//! A.\ninclude!(\"sine.in\");\n#[path = \"../../../shared/sine.rs\"]\nmod sine;\n#[cfg(clippy)]\nfn twin() {}\n#[cfg(windows)]\nfn w() {}\nmacro_rules! m { ($c:meta) => { #[cfg($c)] fn f() {} #[$c] fn g() {} }; }\nm!(cfg);\n",
        );
        let mut moved = scratch.member("b", "../../shared/units.rs");
        moved.targets.push(Target {
            kinds: vec!["bin".to_owned()],
            name: "tool".to_owned(),
            root_file: moved.folder.join("src").join("main.in"),
        });
        scratch.write(&["crates", "b", "src", "main.in"], "fn main() {}\n");
        let problem = check(
            &scratch.0,
            &[scratch.member("a", "src/lib.rs"), moved],
            &ALLOWED,
        )
        .unwrap_err();
        let lib = relative(&["crates", "a", "src", "lib.rs"]);
        for expected in [
            format!("`include!` (the identifier `include`) is used here:\n  {lib}:2\n"),
            format!("the attribute `#[path = \"…\"]` is used here:\n  {lib}:3\n"),
            format!(
                "the bare identifier `clippy` (not followed by `::`) is used here:\n  {lib}:5\n"
            ),
            format!("{UNAPPROVED_CONDITIONS}:\n  {lib}:5: `clippy`\n  {lib}:7: `windows`\n"),
            format!("{}:\n  {lib}:9\n", MACRO_CONDITIONS.0),
            format!("{}:\n  {lib}:10: `cfg`\n", CONDITION_WORDS.0),
            format!("{}:\n  {lib}:9\n", MACRO_ATTRIBUTES.0),
            format!(
                "  b, lib target `b`: {} lies outside the crate's folder, {}\n",
                relative(&["shared", "units.rs"]),
                relative(&["crates", "b"])
            ),
            format!(
                "  b, bin target `tool`: {} does not end in `.rs`\n",
                relative(&["crates", "b", "src", "main.in"])
            ),
        ] {
            assert!(
                problem.contains(&expected),
                "expected {expected:?} in {problem}"
            );
        }
        // Each kind of problem says why, and how a condition can be approved.
        assert!(
            problem.contains("only in a reviewed pull request, together with the Clippy runs"),
            "{problem}"
        );
    }

    #[test]
    fn allows_the_canary_switch_only_below_the_canary_crate() {
        let scratch = Scratch::new("canary");
        let switch = "#[cfg(bayan_lint_canary = \"unsafe_block\")]\npub mod unsafe_block;\n";
        scratch.write(&["xtask", "src", "main.rs"], "fn main() {}\n");
        scratch.write(&["xtask", "lint-canary", "src", "lib.rs"], switch);
        scratch.write(&["xtask", "lint-canary", "src", "unsafe_block.rs"], "");
        let xtask = Package {
            name: "xtask".to_owned(),
            folder: scratch.0.join("xtask"),
            targets: Vec::new(),
        };
        let canary = scratch.member_at("lint-canary", &["xtask", "lint-canary"], "src/lib.rs");
        let packages = [xtask, canary];
        check(&scratch.0, &packages, &ALLOWED).unwrap_or_else(|problem| panic!("{problem}"));
        // xtask's own files lie above the canary's folder, so they belong to xtask.
        scratch.write(&["xtask", "src", "main.rs"], switch);
        let problem = check(&scratch.0, &packages, &ALLOWED).unwrap_err();
        let main = relative(&["xtask", "src", "main.rs"]);
        assert!(
            problem.contains(&format!(
                "  {main}:1: `bayan_lint_canary = \"unsafe_block\"` (the lint canary's switch"
            )),
            "{problem}"
        );
        let canary_lib = relative(&["xtask", "lint-canary", "src", "lib.rs"]);
        assert!(!problem.contains(&canary_lib), "{problem}");
    }

    #[test]
    fn rejects_target_root_files_the_walk_does_not_read() {
        let root = Path::new("/repo");
        let build_folder = root.join("target");
        let package = |name: &str, folder: PathBuf, files: &[&str]| Package {
            name: name.to_owned(),
            targets: files
                .iter()
                .map(|file| Target {
                    kinds: vec!["lib".to_owned()],
                    name: name.to_owned(),
                    root_file: folder.join(file),
                })
                .collect(),
            folder,
        };
        let units = root.join("crates").join("bayan-units");
        // Cargo's defaults, and paths that stay inside the crate's folder once `..` is resolved.
        let fine = package(
            "bayan-units",
            units.clone(),
            &["src/lib.rs", "build.rs", "src/../tests/t.rs"],
        );
        assert_eq!(
            misplaced_roots(root, &build_folder, &[fine]),
            Vec::<String>::new()
        );
        // Cargo does not resolve `..`, so a moved file looks like it is inside the crate's folder.
        for (file, why) in [
            ("../../shared/units.rs", "lies outside the crate's folder"),
            (
                "src/../../bayan-model/src/lib.rs",
                "lies outside the crate's folder",
            ),
            ("src/lib.in", "does not end in `.rs`"),
            ("src/lib.RS", "does not end in `.rs`"),
            ("src/.rs", "does not end in `.rs`"),
            (
                "../../shared/units.in",
                "lies outside the crate's folder, crates/bayan-units and does not end in `.rs`",
            ),
        ] {
            let problems = misplaced_roots(
                root,
                &build_folder,
                &[package("bayan-units", units.clone(), &[file])],
            );
            assert_eq!(problems.len(), 1, "{file}: {problems:?}");
            assert!(
                problems[0].contains(&why.replace('/', std::path::MAIN_SEPARATOR_STR)),
                "{file}: {problems:?}"
            );
        }
        // A crate in the root's own folder would contain the build folder, which the walk skips.
        let problems = misplaced_roots(
            root,
            &build_folder,
            &[package("root", root.to_path_buf(), &["lib.rs"])],
        );
        assert!(
            problems[0].contains("contains the build folder target/"),
            "{problems:?}"
        );
    }

    #[test]
    fn skips_only_the_build_folder() {
        let scratch = Scratch::new("build-folder");
        scratch.write(&["src", "lib.rs"], "//! Root.\n");
        scratch.write(
            &["target", "debug", "build", "out.rs"],
            "#[cfg(clippy)] fn skipped() {}\n",
        );
        let package = Package {
            name: "root".to_owned(),
            folder: scratch.0.clone(),
            targets: Vec::new(),
        };
        let problem = check(&scratch.0, &[package], &ALLOWED).unwrap_err();
        assert!(
            problem.contains("contains the build folder target/"),
            "{problem}"
        );
        assert!(!problem.contains("out.rs"), "{problem}");
    }

    /// Creates a symbolic link to a file, or returns false where the system does not allow it (Windows without developer mode).
    #[expect(
        deprecated,
        reason = "`soft_link` is the one function that creates a symbolic link on every platform; the platform-specific ones (`std::os::unix::fs::symlink` and `std::os::windows::fs::symlink_file`, which it calls) would need the conditions `unix` and `windows`, which the gate rejects"
    )]
    fn symlink(target: &Path, link: &Path) -> bool {
        std::fs::soft_link(target, link).is_ok()
    }

    #[test]
    fn rejects_symbolic_links_in_members() {
        let scratch = Scratch::new("links");
        scratch.write(&["crates", "a", "src", "lib.rs"], "//! A.\npub mod sine;\n");
        let elsewhere = scratch.write(
            &["elsewhere", "sine.txt"],
            "#[cfg(not(clippy))] pub fn sine() {}\n",
        );
        let link = scratch
            .0
            .join("crates")
            .join("a")
            .join("src")
            .join("sine.rs");
        if !symlink(&elsewhere, &link) {
            eprintln!("not checked: this system does not let the test create a symbolic link");
            return;
        }
        let problem =
            check(&scratch.0, &[scratch.member("a", "src/lib.rs")], &ALLOWED).unwrap_err();
        let shown = format!(
            "  {} (a symbolic link)",
            relative(&["crates", "a", "src", "sine.rs"])
        );
        assert!(problem.contains(&shown), "{problem}");
        assert!(
            problem.contains("Replace a link with the file it points to"),
            "{problem}"
        );
    }

    #[test]
    fn rejects_entries_that_are_neither_files_nor_folders() {
        // Only Linux and macOS have such entries; the test asks at run time rather than with the condition `unix`, which the gate rejects.
        if std::env::consts::FAMILY != "unix" {
            eprintln!(
                "not checked: only Unix systems have entries that are neither files, folders nor links"
            );
            return;
        }
        let scratch = Scratch::new("pipe");
        scratch.write(&["crates", "a", "src", "lib.rs"], "//! A.\n");
        // A named pipe, made by the standard Unix program `mkfifo`.
        let pipe = scratch.0.join("crates").join("a").join("s");
        let status = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo failed: {status}");
        let problem =
            check(&scratch.0, &[scratch.member("a", "src/lib.rs")], &ALLOWED).unwrap_err();
        assert!(
            problem.contains("  crates/a/s (neither a file nor a folder)"),
            "{problem}"
        );
    }

    #[test]
    fn finds_files_that_the_file_system_also_finds_as_rust_files() {
        let folder = Path::new("crates").join("a").join("src");
        let ignoring_case = |path: &Path| path == folder.join("sine.rs").as_path();
        let sine_upper = folder.join("sine.RS");
        assert_eq!(
            rust_alias(&sine_upper, &[], ignoring_case),
            Some(OsString::from("sine.rs"))
        );
        // If the folder has a `.rs` file of that name, in any letter case, the compiler reads that one, which the walk reads too.
        for name in ["sine.rs", "Sine.rs"] {
            assert_eq!(
                rust_alias(&sine_upper, &[OsString::from(name)], ignoring_case),
                None
            );
        }
        // A file system that respects letter case does not find it.
        assert_eq!(rust_alias(&sine_upper, &[], |_| false), None);
        assert_eq!(
            rust_alias(&folder.join("notes.txt"), &[], ignoring_case),
            None
        );
    }

    #[test]
    fn rejects_files_that_the_compiler_reads_under_another_name() {
        let scratch = Scratch::new("letter-case");
        scratch.write(&["crates", "a", "src", "lib.rs"], "//! A.\npub mod sine;\n");
        let file = scratch.write(
            &["crates", "a", "src", "sine.RS"],
            "#[cfg(not(clippy))] pub fn sine() {}\n",
        );
        let result = check(&scratch.0, &[scratch.member("a", "src/lib.rs")], &ALLOWED);
        if file.with_file_name("sine.rs").is_file() {
            // Windows and macOS: the file system ignores letter case, so `mod sine;` reads sine.RS.
            let problem = result.unwrap_err();
            let shown = format!(
                "  {} (the file system also finds it as sine.rs)",
                relative(&["crates", "a", "src", "sine.RS"])
            );
            assert!(problem.contains(&shown), "{problem}");
        } else {
            // Linux: the compiler cannot read sine.RS for `mod sine;`, so it is only data.
            result.unwrap_or_else(|problem| panic!("{problem}"));
        }
    }

    #[test]
    fn resolves_dots_from_the_path_alone() {
        let path = Path::new("/repo/crates/bayan-units/./src/../../../shared/units.rs");
        assert_eq!(normalize(path), Path::new("/repo/shared/units.rs"));
        assert_eq!(normalize(Path::new("/../repo")), Path::new("/repo"));
    }

    #[test]
    fn the_real_workspace_passes() {
        let root = crate::workspace_root();
        let packages = crate::policy::packages(&crate::policy::metadata(&root).unwrap()).unwrap();
        check(&root, &packages, &ALLOWED).unwrap_or_else(|problem| panic!("{problem}"));
    }
}
