//! The code that ships of the engine's crates names none of the five modules of Rust's standard library that exist for the outside world.
//!
//! The engine never touches the outside world itself (ADR-0012 §5): clocks, environment variables, files, sockets and other processes are the host's, and the host passes the engine what it needs through the protocol's host services. Nothing that affects the engine's output may depend on the clock or the environment (ADR-0005 §5), and the engine reads no environment variables (architecture §12). This check enforces the part of those rules that the source text shows: the code that ships may name none of the five modules of Rust's standard library that exist for the outside world, `std::env` (environment variables, the command line, the current folder), `std::fs` (files), `std::net` (sockets), `std::process` (other processes, and ending this one) and `std::time` (clocks), except in the workspace members that `OUTSIDE_WORLD` in `verify.rs` lists, with the modules that each may use and the reason. Other parts of std, and dependencies, reach the outside world too (see "Known limitations" below), so passing it means that the code names none of these modules, not that the code cannot reach the outside world.
//!
//! # Why it is not a lint
//!
//! A lint at `deny` or `warn` can be switched off by `#[allow(…)]` or `#[expect(…)]`, and a macro can assemble such an attribute in a form that neither the source check nor Clippy's `allow_attributes` lint sees (`sources.rs`, "Known limitation"). Only a lint at `forbid` cannot be lowered, but Clippy reads one configuration for the whole workspace (`clippy.toml`) and gives a lint one level in every crate, so a ban of these modules there would also hold for the members that must reach the outside world, such as xtask and the command-line tools, with no way to exempt them. This check is therefore not a lint at all: xtask reads the source text itself, so no attribute changes what it finds, however it is written or assembled.
//!
//! # The code it reads
//!
//! It reads every line of exactly the Rust files that the compiler compiled into code that ships. After its runs, the Clippy step hands it, for every member, the files that the dependency files list for the member's units that are not tests (not compiled as unit tests, and not a test, benchmark, example or build-script target), among the files that the source check read (`dep_info.rs`). A module that only the tests use, declared with `#[cfg(test)] mod name;`, is left out by that record itself, because the compiler does not read its file for those units. That, or an integration test, is where tests that measure time, read fixtures or run programs belong. Inside the files that remain, nothing is skipped, also not a module written in place under `#[cfg(test)]`: the check reads the source text, and a macro can drop that condition from a module it is handed and ship the module's code, as in `macro_rules! shipped { (#[cfg(test)] mod $name:ident { $($body:tt)* }) => { $($body)* }; }`, so only the compiler's record can tell which code only the tests compile.
//!
//! It also reads the strings in attributes, because a derive macro can read a string of its attribute as Rust and make it code that ships: `#[serde(default = "path")]` makes serde's derive call the function at `path`. Each such string is decoded as the compiler decodes it (escapes, raw strings) and checked by the same rules as code, also for the ways of naming std below. Prose in an attribute, such as the `reason` of an `#[expect]`, is checked the same way, since most prose reads as a series of Rust tokens: it names these modules in words, not as paths, and does not write `std` on its own. A string that cannot be read as Rust at all (an unclosed quote, comment or bracket) is skipped, because a derive macro that reads its string as Rust, as serde's and schemars' do, rejects it too. Documentation is left out (`#[doc = "…"]`, which `///` and `//!` comments become, and `#[doc(…)]`): nothing compiles it into code that ships, and its examples are compiled only as doctests.
//!
//! # What it rejects
//!
//! In the code that ships of a member that `OUTSIDE_WORLD` does not name, and of a member that it names, every module that the entry does not list:
//!
//! - a path from `std` into one of the five modules, such as `std::time::Instant`, `::std::env::var` or `use std::{fs, io}`, also in a group inside a group (`use std::{{net}}`), whatever spaces, comments or line breaks stand between its words, and also with raw identifiers (`r#std::r#env`).
//!
//! In the code that ships of every member, ways to name std, or to build a path in a macro, that would let a later path reach those modules without writing `std` in front of them:
//!
//! - `std` on its own, not followed by `::`: renamed (`use std as host;`, `extern crate std as host;`), handed to a macro (`m!(std)`), or used as a name;
//! - `std::*`, and `*` in a group after `std::`, which import every module of std under its own name, so that `env::var` reaches `std::env`;
//! - `self` in a group after `std::`, as in `use std::{self as host}`, which renames std;
//! - `std::` followed by anything but a name or a group, such as a macro's metavariable (`std::$module`);
//! - in a macro, a path that continues from a metavariable into one of the five names or into another metavariable (`$root::env`, `$root::$module`), because a macro can be handed `std` as the first word of a longer path; and a metavariable anywhere in a `use` or `extern crate` declaration (`use $root as host;`, `use {$root as host};`, `use $root::*;`), which could rename std or import all of it. `$crate`, which always names the macro's own crate, is fine.
//!
//! In the preflight, before anything is built, a member that `OUTSIDE_WORLD` does not name may not depend on one that it names, except for its tests (`[dev-dependencies]`), so that code which ships cannot reach the outside world through a member that may.
//!
//! The list cannot go stale: an entry for a member that does not exist, an entry without modules, a module that is not one of the five, and a module that the member's code that ships no longer uses all fail.
//!
//! # Known limitations
//!
//! These rules model Rust's tokens and paths; they are not the compiler's name resolution, and they read only the workspace's own code.
//!
//! - A path that never writes the module's name is not seen: a dependency that re-exports a module of std under another name, or a macro of a dependency that expands to such a path. Dependencies are reviewed when they are added (ADR-0017).
//! - Functions of dependencies that reach the outside world themselves are not seen either, such as cfb's `cfb::open(path)`, `open_rw(path)` and `create(path)`, which open files with `std::fs`; bayan-opc calls only `CompoundFile::open` on the bytes it is handed (a `Cursor`).
//! - Other parts of std reach the outside world without naming one of the five modules: the methods of `std::path::Path` that ask the file system (`exists`, `try_exists`, `is_file`, `is_dir`, `is_symlink`, `metadata`, `symlink_metadata`, `read_dir`, `read_link` and `canonicalize`), and `std::hash::RandomState` (also named `std::collections::hash_map::RandomState`), which seeds itself from the operating system's randomness. `std::path` as a whole cannot be banned, because bayan-opc uses its `Component` and `PathBuf` (`compound_file.rs`).
//! - An attribute macro on an enclosing module, or any other procedural macro, receives the code it is applied to and can write different code: it could drop `#[cfg(test)]` from a module as `shipped!` does above, or write a path that the source does not show. No workspace member is a procedural-macro crate (`PROC_MACROS` in `verify.rs` is empty), and no dependency of the members offers an attribute macro for modules today (wasm-bindgen's `#[wasm_bindgen]`, the only attribute macro among them, refuses modules); the macros of dependencies are reviewed with them (ADR-0017).
//! - A string in an attribute that a macro assembles from a split form, keeping the `#` of one attribute and writing it before a bracket group handed in separately (AGENTS.md, "Lint rules and exceptions"), is not read as an attribute's string, because the source does not show that bracket group as an attribute. Reviewers check every macro that takes a `#` or a bracket group as a `tt`, as AGENTS.md asks.
//!
//! In the WebAssembly build, where std has no clock, files or environment of its own, the import check of `cargo xtask wasm-package` (`wasm_imports.rs`) shows every function that the module asks of its JavaScript host, whoever calls it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::policy::MemberDependency;
use crate::sources::{self, Kind, Token};

/// The five modules of Rust's standard library that exist for the outside world, as code writes them.
pub const MODULES: [&str; 5] = [
    "std::env",
    "std::fs",
    "std::net",
    "std::process",
    "std::time",
];

/// The name through which code reaches Rust's standard library.
const STD: &str = "std";

/// A workspace member whose code that ships may use some of [`MODULES`]: one entry of `OUTSIDE_WORLD` in `verify.rs`.
#[derive(Debug, Clone, Copy)]
pub struct Exemption {
    /// The member.
    pub member: &'static str,
    /// The modules it may use, as code writes them, such as `std::fs`.
    pub modules: &'static [&'static str],
    /// Why it needs them.
    pub reason: &'static str,
}

/// Why the modules are rejected, and what to do instead.
const WHY: &str = "The engine never touches the outside world itself (ADR-0012 §5): clocks, environment variables, files, sockets and other processes are the host's, which passes the engine what it needs through the protocol's host services, and nothing that affects the engine's output may depend on the clock or the environment (ADR-0005 §5; the engine reads no environment variables, architecture §12). So the code that ships may name none of the five modules of Rust's standard library that exist for the outside world, std::env, std::fs, std::net, std::process and std::time, except in the members that OUTSIDE_WORLD in xtask/src/verify.rs lists, each with the modules it may use; this includes the strings of attributes, which a derive macro such as serde's can read as paths. Ask the host for what the code needs instead (a time, the bytes of a file, a setting). For a length of time without a clock, use core::time::Duration, the same type as std::time::Duration. If only tests need the module, move them out of the files that ship: into an integration test (tests/), or into a module declared with `#[cfg(test)] mod name;` and written in its own file, which the compiler does not read for the code that ships. A test module written in place, `#[cfg(test)] mod tests { … }`, counts as code that ships, because a macro could drop its condition. A member that really needs a module is added to OUTSIDE_WORLD, with the module and the reason, in a reviewed pull request.";

/// How the uses of the modules are reported: the heading of their list.
const USED: &str =
    "the code that ships names modules of the standard library that exist for the outside world";

/// How the ways of naming std that the check cannot follow are reported: the heading of their list, and what to do.
const HIDDEN: (&str, &str) = (
    "the code that ships names `std`, or builds a path in a macro, in a way that could reach those modules unseen",
    "This check reads the source text, not the compiler's name resolution, so it finds a module of std only where a path names it after `std::`. Naming std in any other way could reach std::env, std::fs, std::net, std::process or std::time without writing that path: `use std as host;` and then `host::env::var`, `use std::*;` and then `env::var`, or a macro that is handed `std` and writes `$root::env`. So std is named only as the first word of a path written out in full, `std::name` or `std::{…}`. Write the path out in full, and rename an item or a variable that happens to be called `std`.",
);

/// How the modules that a member no longer uses are reported: the heading of their list, and what to do.
const STALE: (&str, &str) = (
    "OUTSIDE_WORLD lists modules that the member's code that ships does not use",
    "Each entry of OUTSIDE_WORLD in xtask/src/verify.rs names a member and the modules of the standard library that its code that ships may use. Remove each module above from its entry, and the entry when no module is left, so that the list names only what the members need and cannot go stale.",
);

/// How the dependencies of other members on the listed ones are reported: the heading of their list, and what to do.
const DEPENDS: (&str, &str) = (
    "members that OUTSIDE_WORLD does not name depend on members that it names",
    "A member that OUTSIDE_WORLD in xtask/src/verify.rs names may reach the outside world, so code that ships of any other member could reach it through such a dependency, without naming a module of the standard library itself. Only tests may depend on them (`[dev-dependencies]`). Move the code that both need into a member outside the list, or remove the dependency.",
);

/// What the scan found in one file, each finding with the line it is on.
#[derive(Debug, Default, PartialEq, Eq)]
struct Found {
    /// The uses of [`MODULES`], each by the module as code writes it.
    uses: Vec<(usize, &'static str)>,
    /// The ways of naming std, or of building a path in a macro, that could reach those modules unseen, each described.
    hidden: Vec<(usize, &'static str)>,
}

/// `std` on its own.
const BARE_STD: &str = "`std` on its own, not followed by `::`";
/// `std::*`, or `*` in a group after `std::`.
const STD_GLOB: &str = "`*` after `std::`, which imports every module of std under its own name";
/// `self` in a group after `std::`.
const STD_SELF: &str = "`self` in a group after `std::`, which can rename std";
/// `std::` followed by something else.
const STD_UNREADABLE: &str =
    "`std::` followed by something other than a name or a group, such as a metavariable";
/// A path from a metavariable on.
const MACRO_PATH: &str =
    "a path from a macro's metavariable into one of the five names or another metavariable";
/// `use` or `extern crate` with a metavariable.
const MACRO_USE: &str = "a `use` or `extern crate` declaration with a macro's metavariable in it";

/// The attribute of documentation, whose strings are never code (`#[doc = "…"]`, `#[doc(…)]`).
const DOC: &str = "doc";

/// The module of [`MODULES`] whose last word is `name`, if there is one.
fn module(name: &str) -> Option<&'static str> {
    MODULES
        .iter()
        .copied()
        .find(|module| module.strip_prefix("std::") == Some(name))
}

/// Finds in Rust source the uses of [`MODULES`] and the ways of naming std, or of building a path in a macro, that could reach them unseen, in every line, and in the strings of attributes other than documentation, each decoded and read as Rust (see the module's documentation). Comments, other strings and character literals are skipped, as the source check skips them. Source the reader cannot follow is an error.
fn scan(source: &str) -> Result<Found, String> {
    let tokens = sources::tokenize(source)?;
    let closing = sources::closing_brackets(&tokens)?;
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    let mut found = Found::default();
    for (at, token) in tokens.iter().enumerate() {
        let line = token.line;
        match token.kind {
            Kind::Ident(STD) if kind(at + 1) == Some(Kind::PathSeparator) => {
                after_std(&tokens, &closing, at + 2, &mut found);
            }
            Kind::Ident(STD) => found.hidden.push((line, BARE_STD)),
            Kind::Punct(b'$') => {
                if let Some(Kind::Ident(name)) = kind(at + 1)
                    && name != "crate"
                    && kind(at + 2) == Some(Kind::PathSeparator)
                {
                    let continues = match kind(at + 3) {
                        Some(Kind::Ident(next)) => module(next).is_some(),
                        Some(Kind::Punct(b'$')) => true,
                        _ => false,
                    };
                    if continues {
                        found.hidden.push((line, MACRO_PATH));
                    }
                }
            }
            Kind::Ident(word @ ("use" | "crate"))
                if word == "use" || (at >= 1 && tokens[at - 1].kind == Kind::Ident("extern")) =>
            {
                // A declaration ends at its `;`, which no use tree contains.
                let metavariable = tokens[at + 1..]
                    .iter()
                    .take_while(|token| token.kind != Kind::Punct(b';'))
                    .enumerate()
                    .any(|(offset, token)| {
                        token.kind == Kind::Punct(b'$')
                            && kind(at + 2 + offset) != Some(Kind::Ident("crate"))
                    });
                if metavariable {
                    found.hidden.push((line, MACRO_USE));
                }
            }
            _ => {}
        }
    }
    // Each string once, also when attributes nest.
    let strings: BTreeSet<usize> = sources::attribute_contents(&tokens, &closing)
        .into_iter()
        .filter(|content| kind(content.start) != Some(Kind::Ident(DOC)))
        .flatten()
        .filter(|&at| matches!(tokens[at].kind, Kind::Str(_)))
        .collect();
    for at in strings {
        let Kind::Str(literal) = tokens[at].kind else {
            continue;
        };
        // A string that is not Rust is skipped: whatever reads it as Rust rejects it too.
        let Some(Ok(inner)) = string_value(literal).map(|text| scan(&text)) else {
            continue;
        };
        // Reported on the line where the string starts.
        let line = tokens[at].line;
        found
            .uses
            .extend(inner.uses.into_iter().map(|(_, module)| (line, module)));
        found
            .hidden
            .extend(inner.hidden.into_iter().map(|(_, what)| (line, what)));
    }
    found.uses.sort_by_key(|&(line, _)| line);
    found.hidden.sort_by_key(|&(line, _)| line);
    Ok(found)
}

/// The text of a string literal, written as in the source with its prefix and quotes (`"…"`, `b"…"`, `c"…"`, `r#"…"#`, `br"…"`, `cr"…"`), with its escapes decoded as the compiler decodes them. `None` for an escape that this reader does not know; the compiler rejects every such escape.
fn string_value(literal: &str) -> Option<String> {
    let literal = literal.strip_prefix(['b', 'c']).unwrap_or(literal);
    if let Some(raw) = literal.strip_prefix('r') {
        // A raw string: its text is exactly what stands between `r#…#"` and `"#…#`.
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        return raw
            .get(hashes + 1..raw.len().checked_sub(hashes + 1)?)
            .map(str::to_owned);
    }
    let mut characters = literal.strip_prefix('"')?.strip_suffix('"')?.chars();
    let mut text = String::new();
    while let Some(character) = characters.next() {
        if character != '\\' {
            text.push(character);
            continue;
        }
        match characters.next()? {
            'n' => text.push('\n'),
            'r' => text.push('\r'),
            't' => text.push('\t'),
            '0' => text.push('\0'),
            escaped @ ('\\' | '\'' | '"') => text.push(escaped),
            'x' => {
                let digits: String = characters.by_ref().take(2).collect();
                text.push(char::from(u8::from_str_radix(&digits, 16).ok()?));
            }
            'u' => {
                if characters.next()? != '{' {
                    return None;
                }
                let digits: String = characters
                    .by_ref()
                    .take_while(|&digit| digit != '}')
                    .filter(|&digit| digit != '_')
                    .collect();
                text.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            // A line break after a backslash continues the string on the next line, without the break and the spaces that begin that line.
            '\n' => {
                let rest = characters.as_str();
                characters = rest.trim_start_matches([' ', '\t', '\n', '\r']).chars();
            }
            _ => return None,
        }
    }
    Some(text)
}

/// Reads what follows `std::`, starting at `tokens[at]`: a name, which may be one of [`MODULES`], or a group, whose entries are read by [`group`]. Anything else is reported.
fn after_std(tokens: &[Token<'_>], closing: &[Option<usize>], at: usize, found: &mut Found) {
    let line = tokens[at - 1].line;
    match tokens.get(at).map(|token| token.kind) {
        Some(Kind::Ident(name)) => {
            if let Some(module) = module(name) {
                found.uses.push((tokens[at].line, module));
            }
        }
        Some(Kind::Punct(b'{')) => group(tokens, closing, at, found),
        Some(Kind::Punct(b'*')) => found.hidden.push((line, STD_GLOB)),
        _ => found.hidden.push((line, STD_UNREADABLE)),
    }
}

/// Reads the entries of the group `{ … }` that opens at `tokens[open]` after `std::`: the first word of each entry names a module of std, so it is checked against [`MODULES`]; `self` and `*` are reported, and a group that is itself an entry, with no path before it (`std::{{env}}`), is read in the same way. The rest of each entry belongs to the module it starts with.
fn group(tokens: &[Token<'_>], closing: &[Option<usize>], open: usize, found: &mut Found) {
    // `closing_brackets` succeeded, so every bracket is closed.
    let Some(close) = closing[open] else {
        return;
    };
    let mut at = open + 1;
    let mut starts_entry = true;
    while at < close {
        let token = tokens[at];
        if starts_entry {
            starts_entry = false;
            match token.kind {
                Kind::Ident("self") => found.hidden.push((token.line, STD_SELF)),
                Kind::Ident(name) => {
                    if let Some(module) = module(name) {
                        found.uses.push((token.line, module));
                    }
                }
                Kind::Punct(b'{') => group(tokens, closing, at, found),
                Kind::Punct(b'*') => found.hidden.push((token.line, STD_GLOB)),
                _ => found.hidden.push((token.line, STD_UNREADABLE)),
            }
        }
        match token.kind {
            Kind::Punct(b',') => starts_entry = true,
            // A nested group belongs to the entry it is part of, and one that is an entry was read above.
            Kind::Punct(b'(' | b'[' | b'{') => {
                if let Some(end) = closing[at] {
                    at = end;
                }
            }
            _ => {}
        }
        at += 1;
    }
}

/// Checks `OUTSIDE_WORLD` (`exemptions`) before anything is built: each entry names a workspace member (of `members`) once, and lists at least one module, each one of [`MODULES`] and only once; and no member that the list does not name depends on one that it names (`dependencies`, from `cargo metadata`), except as a dev-dependency. Returns one line describing what was checked, or every problem found.
pub fn check_list(
    members: &[&str],
    dependencies: &[MemberDependency],
    exemptions: &[Exemption],
) -> Result<String, String> {
    let mut problems = Vec::new();
    for (index, exemption) in exemptions.iter().enumerate() {
        let member = exemption.member;
        if !members.contains(&member) {
            problems.push(format!(
                "  `{member}` is not a workspace member; update the list"
            ));
        }
        if exemptions[..index]
            .iter()
            .any(|earlier| earlier.member == member)
        {
            problems.push(format!("  `{member}` is listed twice"));
        }
        if exemption.modules.is_empty() {
            problems.push(format!(
                "  the entry for `{member}` lists no module; remove it"
            ));
        }
        for (position, written) in exemption.modules.iter().enumerate() {
            if !MODULES.contains(written) {
                problems.push(format!(
                    "  the entry for `{member}` lists `{written}`, which is not one of {}",
                    MODULES.join(", ")
                ));
            }
            if exemption.modules[..position].contains(written) {
                problems.push(format!(
                    "  the entry for `{member}` lists `{written}` twice"
                ));
            }
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "OUTSIDE_WORLD in xtask/src/verify.rs is not right:\n{}\nEach entry names a workspace member, the modules of the standard library that exist for the outside world ({}) that its code that ships may use, and the reason.",
            problems.join("\n"),
            MODULES.join(", ")
        ));
    }
    let listed = |name: &str| exemptions.iter().any(|exemption| exemption.member == name);
    let reaching: Vec<String> = dependencies
        .iter()
        .filter(|dependency| !dependency.dev && !listed(&dependency.from) && listed(&dependency.on))
        .map(|dependency| format!("  {} depends on {}", dependency.from, dependency.on))
        .collect();
    if !reaching.is_empty() {
        return Err(format!(
            "{}:\n{}\n{}",
            DEPENDS.0,
            reaching.join("\n"),
            DEPENDS.1
        ));
    }
    let names: Vec<&str> = exemptions
        .iter()
        .map(|exemption| exemption.member)
        .collect();
    Ok(format!(
        "outside world: every member that OUTSIDE_WORLD names is a workspace member ({}), and no other member depends on one of them except for its tests",
        names.join(", ")
    ))
}

/// Checks the code that ships of every workspace member at `root`: `shipped` maps each member to the Rust files that the compiler compiled into its units that are not tests (`dep_info::Comparison::shipped`). A member that `exemptions` does not name may use none of [`MODULES`], one that it names only those that its entry lists, and every member names std only as the first word of a path written out in full. Each module that an entry lists must be used. Returns one line describing what was checked, or every problem found.
pub fn check_code(
    root: &Path,
    shipped: &BTreeMap<String, BTreeSet<PathBuf>>,
    exemptions: &[Exemption],
) -> Result<String, String> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("cannot find {}: {error}", root.display()))?;
    let shown = |file: &Path| {
        file.strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string()
    };
    let mut used_places = Vec::new();
    let mut hidden_places = Vec::new();
    let mut used: BTreeSet<(&str, &str)> = BTreeSet::new();
    // The files read, of the members that the list does not name and of those it names.
    let mut files_read = (0, 0);
    for (member, files) in shipped {
        let exemption = exemptions
            .iter()
            .find(|exemption| exemption.member == member);
        for file in files {
            let source = std::fs::read_to_string(file)
                .map_err(|error| format!("cannot read {}: {error}", shown(file)))?;
            let found = scan(&source).map_err(|error| {
                format!(
                    "{}: {error}. The gate must be able to read every Rust file to check it.",
                    shown(file)
                )
            })?;
            if exemption.is_some() {
                files_read.1 += 1;
            } else {
                files_read.0 += 1;
            }
            for (line, module) in found.uses {
                used.insert((member.as_str(), module));
                match exemption {
                    Some(exemption) if exemption.modules.contains(&module) => {}
                    Some(exemption) => used_places.push(format!(
                        "  {}:{line}: {module} (OUTSIDE_WORLD lists only {} for {member})",
                        shown(file),
                        exemption.modules.join(", ")
                    )),
                    None => used_places.push(format!("  {}:{line}: {module}", shown(file))),
                }
            }
            for (line, what) in found.hidden {
                hidden_places.push(format!("  {}:{line}: {what}", shown(file)));
            }
        }
    }
    let mut stale = Vec::new();
    for exemption in exemptions {
        for module in exemption.modules {
            if !used.contains(&(exemption.member, *module)) {
                stale.push(format!(
                    "  {module}, for {} (listed because {})",
                    exemption.member, exemption.reason
                ));
            }
        }
    }
    let problems: Vec<String> = [
        (used_places, USED, WHY),
        (hidden_places, HIDDEN.0, HIDDEN.1),
        (stale, STALE.0, STALE.1),
    ]
    .into_iter()
    .filter(|(places, _, _)| !places.is_empty())
    .map(|(places, heading, why)| format!("{heading}:\n{}\n{why}", places.join("\n")))
    .collect();
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let others: Vec<&str> = shipped
        .keys()
        .map(String::as_str)
        .filter(|member| {
            !exemptions
                .iter()
                .any(|exemption| exemption.member == *member)
        })
        .collect();
    Ok(format!(
        "outside world: the code that ships of the members that OUTSIDE_WORLD does not name ({}; {} files) names none of {}, and that of the members it names ({} files) names exactly the modules it lists for them",
        others.join(", "),
        files_read.0,
        MODULES.join(", "),
        files_read.1
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `scan` finds in `source`, which it must be able to read.
    fn scanned(source: &str) -> Found {
        scan(source).unwrap_or_else(|problem| panic!("{problem}"))
    }

    /// The modules that `scan` finds in `source`, which must name std in no hidden way.
    fn uses(source: &str) -> Vec<&'static str> {
        let found = scanned(source);
        assert!(found.hidden.is_empty(), "{source}: {:?}", found.hidden);
        found.uses.into_iter().map(|(_, module)| module).collect()
    }

    /// The descriptions of the hidden ways that `scan` finds in `source`.
    fn hidden(source: &str) -> Vec<&'static str> {
        scanned(source)
            .hidden
            .into_iter()
            .map(|(_, what)| what)
            .collect()
    }

    #[test]
    fn finds_every_path_into_the_five_modules() {
        assert_eq!(
            uses("fn now() -> u64 { std::time::Instant::now(); 0 }"),
            ["std::time"]
        );
        assert_eq!(uses("use ::std::env::var;"), ["std::env"]);
        assert_eq!(
            uses("use std::{fs, io::{self, Read}, net::TcpStream};"),
            ["std::fs", "std::net"]
        );
        // A group inside a group, with no path before it, still names std's modules.
        assert_eq!(
            uses("use std::{{process}, {{time as clock}}};"),
            ["std::process", "std::time"]
        );
        // Spaces, comments, line breaks and raw identifiers change nothing.
        assert_eq!(
            uses("let home = std /* the library */ ::\n    r#env :: var(\"HOME\");"),
            ["std::env"]
        );
        assert_eq!(uses("r#std::fs::read(path)"), ["std::fs"]);
        // In a group, only the first word of an entry is a module of std; `self` inside a nested group belongs to that module.
        assert_eq!(
            uses("use std::{io::{self, Write}, sync::Arc};"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn reports_the_line_of_each_use() {
        let found = scanned(
            "use std::sync::Arc;\n\nuse std::{\n    collections::BTreeMap,\n    time::Duration,\n};\nfn f() { std::process::abort() }\n",
        );
        assert_eq!(
            found,
            Found {
                uses: vec![(5, "std::time"), (7, "std::process")],
                hidden: Vec::new(),
            }
        );
    }

    #[test]
    fn leaves_other_modules_and_other_crates_alone() {
        assert_eq!(
            uses(
                "use std::collections::BTreeMap; use std::sync::{Arc, Mutex}; use std::thread::Builder; use core::time::Duration; use crate::time::Clock; let fs = 1; fn process() {} const NAME: &str = \"std::env\"; // std::fs\n/* std::net */"
            ),
            Vec::<&str>::new()
        );
        // The compile-time macros `env!` and `option_env!` read Cargo's variables while the crate is compiled, not the environment of the running engine.
        assert_eq!(
            uses("pub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn reads_every_line_also_under_cfg_test() {
        // The compiler's record leaves out the files of the modules that only the tests declare (`#[cfg(test)] mod name;`); in the files that ship, every line counts, also a test module written in place.
        assert_eq!(
            uses(
                "use std::sync::Arc;\n#[cfg(test)]\n#[cfg(not(target_arch = \"wasm32\"))]\nmod tests {\n    use std::time::Duration;\n}\nfn after() { std::env::args(); }\n"
            ),
            ["std::time", "std::env"]
        );
        assert_eq!(uses("#[cfg(test)] use std::time::Instant;"), ["std::time"]);
        assert_eq!(
            uses("#[cfg(any(test, debug_assertions))] mod checks { use std::env; }"),
            ["std::env"]
        );
    }

    #[test]
    fn reads_the_test_modules_that_a_macro_could_ship() {
        // A macro can drop `#[cfg(test)]` from the module it is handed and ship the module's code, so the test module in its call counts.
        let shipped = "macro_rules! shipped {\n    (#[cfg(test)] mod $name:ident { $($body:tt)* }) => { $($body)* };\n}\nshipped! {\n    #[cfg(test)]\n    mod tests {\n        pub fn now() -> std::time::Instant { std::time::Instant::now() }\n    }\n}\n";
        assert_eq!(uses(shipped), ["std::time", "std::time"]);
        // The same with a matcher that takes any attributes.
        let any_attributes = "macro_rules! shipped {\n    ($(#[$meta:meta])* mod $name:ident { $($body:tt)* }) => { $($body)* };\n}\nshipped! { #[cfg(test)] mod tests { pub use std::env::var; } }\n";
        assert_eq!(uses(any_attributes), ["std::env"]);
    }

    #[test]
    fn reads_the_strings_of_attributes_as_code() {
        // serde's derive reads these strings as paths: the first names no module of std, the second calls `std::process::id`.
        assert_eq!(
            uses(
                "#[derive(Serialize)]\nstruct Answer {\n    #[serde(default, skip_serializing_if = \"std::ops::Not::not\")]\n    flag: bool,\n}\n"
            ),
            Vec::<&str>::new()
        );
        assert_eq!(
            scanned(
                "#[derive(Deserialize)]\nstruct Request {\n    #[serde(default = \"std::process::id\")]\n    id: u32,\n}\n"
            ),
            Found {
                uses: vec![(3, "std::process")],
                hidden: Vec::new(),
            }
        );
        // Escapes, raw strings, a line continued after a backslash, an attribute inside `cfg_attr` and an inner attribute are read as the compiler reads them.
        assert_eq!(
            uses(
                "#[serde(default = \"std\\x3a\\x3aprocess::id\")]\n#[serde(with = r#\"std::time\"#)]\n#[cfg_attr(test, serde(default = \"std::\\\n    env::args\"))]\n#![serde(default = \"\\u{73}td::fs::read\")]\n"
            ),
            ["std::process", "std::time", "std::env", "std::fs"]
        );
        // So are the ways of naming std, also in a `[…]` after a metavariable, which can become an attribute.
        assert_eq!(hidden("#[serde(with = \"std\")]"), [BARE_STD]);
        assert_eq!(
            hidden("macro_rules! m { ($hash:tt) => { $hash[serde(with = \"std::*\")] }; }"),
            [STD_GLOB]
        );
        // Prose that reads as Rust is checked like code; a string that cannot be read as Rust, documentation and the strings outside attributes are not.
        assert_eq!(
            uses("#[expect(dead_code, reason = \"never calls std::process::exit\")]"),
            ["std::process"]
        );
        assert_eq!(
            uses(
                "#[expect(dead_code, reason = \"it's 'quoted' prose about std::fs, which no derive reads\")]\n#[doc = \"Calls std::env::var.\"]\n/// Calls std::time::Instant::now.\n#[doc(alias = \"std::net\")]\nconst PATH: &str = \"std::process::id\";\n"
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn decodes_strings_as_the_compiler_does() {
        for (literal, text) in [
            ("\"plain\"", "plain"),
            ("\"a\\n\\r\\t\\0\\\\\\'\\\"z\"", "a\n\r\t\0\\'\"z"),
            ("\"\\x41\\u{1F600}\\u{6_1}\"", "A\u{1F600}a"),
            ("\"one \\\n      two\"", "one two"),
            ("b\"bytes\"", "bytes"),
            ("c\"text\"", "text"),
            ("r\"no \\n escape\"", "no \\n escape"),
            ("r##\"a \"# quote\"##", "a \"# quote"),
            ("br#\"raw bytes\"#", "raw bytes"),
            ("cr\"raw text\"", "raw text"),
        ] {
            assert_eq!(string_value(literal).as_deref(), Some(text), "{literal}");
        }
        // An escape the compiler does not know is not read.
        assert_eq!(string_value("\"\\q\""), None);
    }

    #[test]
    fn rejects_every_other_way_to_name_std() {
        for (source, expected) in [
            (
                "use std as host; fn f() { host::env::var(\"X\"); }",
                BARE_STD,
            ),
            ("extern crate std as host;", BARE_STD),
            ("use ::std as host;", BARE_STD),
            ("library!(std);", BARE_STD),
            ("let std = 1;", BARE_STD),
            ("use std::*;", STD_GLOB),
            ("use std::{sync::Arc, *};", STD_GLOB),
            ("use std::{self as host};", STD_SELF),
            (
                "macro_rules! m { ($module:ident) => { std::$module::var(\"X\") }; }",
                STD_UNREADABLE,
            ),
            ("library!(std::);", STD_UNREADABLE),
        ] {
            assert_eq!(hidden(source), [expected], "{source}");
        }
    }

    #[test]
    fn rejects_macros_that_could_continue_a_path_from_std() {
        for source in [
            "macro_rules! m { ($root:ident) => { $root::env::var(\"X\") }; }",
            "macro_rules! m { ($root:ident, $module:ident) => { $root::$module::var(\"X\") }; }",
        ] {
            assert_eq!(hidden(source), [MACRO_PATH], "{source}");
        }
        for source in [
            "macro_rules! m { ($root:ident) => { use $root as host; }; }",
            "macro_rules! m { ($root:ident) => { use ::$root as host; }; }",
            "macro_rules! m { ($root:ident) => { use {$root as host}; }; }",
            "macro_rules! m { ($root:ident) => { pub use $root::{env}; }; }",
            "macro_rules! m { ($root:ident) => { use $root::*; }; }",
            "macro_rules! m { ($root:ident) => { extern crate $root as host; }; }",
            "macro_rules! m { ($($path:ident)::+) => { use $($path)::+ as host; }; }",
        ] {
            assert_eq!(hidden(source), [MACRO_USE], "{source}");
        }
        // `$crate` names the macro's own crate, and other paths from a metavariable reach no module of std.
        assert_eq!(
            hidden(
                "macro_rules! m { ($name:ident, $value:expr) => { use $crate::limits::MAX; $name::new($value as u64); }; }"
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn refuses_source_it_cannot_follow() {
        assert!(scan("fn f() { /* never closed }").is_err());
        assert!(scan("fn f() { (] }").is_err());
    }

    /// An entry of `OUTSIDE_WORLD` for `member` with `modules`.
    fn exemption(member: &'static str, modules: &'static [&'static str]) -> Exemption {
        Exemption {
            member,
            modules,
            reason: "a tool that runs on the developer's machine",
        }
    }

    /// A dependency of `from` on `on`.
    fn dependency(from: &str, on: &str, dev: bool) -> MemberDependency {
        MemberDependency {
            from: from.to_owned(),
            on: on.to_owned(),
            dev,
        }
    }

    #[test]
    fn accepts_a_list_of_members_whose_dependents_are_only_tests() {
        let line = check_list(
            &["engine", "tool"],
            &[
                dependency("tool", "engine", false),
                dependency("engine", "tool", true),
            ],
            &[exemption("tool", &["std::fs", "std::env"])],
        )
        .unwrap();
        assert!(line.contains("is a workspace member (tool)"), "{line}");
    }

    #[test]
    fn refuses_a_list_that_is_not_right() {
        let problem = check_list(
            &["engine", "tool"],
            &[],
            &[
                exemption("gone", &["std::fs"]),
                exemption("tool", &[]),
                exemption("tool", &["std::io", "std::fs", "std::fs"]),
            ],
        )
        .unwrap_err();
        for expected in [
            "`gone` is not a workspace member",
            "`tool` is listed twice",
            "the entry for `tool` lists no module",
            "lists `std::io`, which is not one of std::env, std::fs, std::net, std::process, std::time",
            "lists `std::fs` twice",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
    }

    #[test]
    fn refuses_code_that_ships_depending_on_a_listed_member() {
        let problem = check_list(
            &["engine", "tool"],
            &[dependency("engine", "tool", false)],
            &[exemption("tool", &["std::fs"])],
        )
        .unwrap_err();
        assert!(problem.contains("  engine depends on tool"), "{problem}");
    }

    /// A folder for one test's files, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let folder = std::env::temp_dir()
                .join(format!("bayan-outside-world-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            Self(std::fs::canonicalize(&folder).unwrap())
        }

        /// Writes `text` to `name` in the folder and returns its path.
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn checks_the_code_that_ships_against_the_list() {
        let scratch = Scratch::new("code");
        let engine = scratch.write("engine.rs", "use std::sync::Arc;\nfn now() { std::time::Instant::now(); }\n#[cfg(test)]\nmod tests { use std::fs; }\n");
        let tool = scratch.write(
            "tool.rs",
            "use std::{env, fs};\nfn main() { let _ = std::net::TcpStream::connect(\"x:1\"); }\n",
        );
        let hiding = scratch.write("hiding.rs", "use std as host;\n");
        let shipped = BTreeMap::from([
            ("engine".to_owned(), BTreeSet::from([engine])),
            ("tool".to_owned(), BTreeSet::from([tool])),
            ("other".to_owned(), BTreeSet::from([hiding])),
        ]);
        let problem = check_code(
            &scratch.0,
            &shipped,
            &[exemption("tool", &["std::env", "std::fs", "std::process"])],
        )
        .unwrap_err();
        for expected in [
            USED,
            "  engine.rs:2: std::time\n",
            // A test module written in place ships as far as the check can tell.
            "  engine.rs:4: std::fs\n",
            "  tool.rs:2: std::net (OUTSIDE_WORLD lists only std::env, std::fs, std::process for tool)",
            HIDDEN.0,
            "  hiding.rs:1: `std` on its own",
            STALE.0,
            "  std::process, for tool",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
    }

    #[test]
    fn passes_code_that_uses_exactly_what_the_list_says() {
        let scratch = Scratch::new("passes");
        let engine = scratch.write(
            "engine.rs",
            "use std::sync::Arc;\npub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");\n",
        );
        let tool = scratch.write("tool.rs", "use std::{env, fs};\n");
        let shipped = BTreeMap::from([
            ("engine".to_owned(), BTreeSet::from([engine])),
            ("tool".to_owned(), BTreeSet::from([tool])),
        ]);
        let line = check_code(
            &scratch.0,
            &shipped,
            &[exemption("tool", &["std::env", "std::fs"])],
        )
        .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.contains(
                "the members that OUTSIDE_WORLD does not name (engine; 1 files) names none of std::env, std::fs, std::net, std::process, std::time, and that of the members it names (1 files) names exactly"
            ),
            "{line}"
        );
    }
}
