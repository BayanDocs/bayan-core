//! The engine never reaches the outside world through Rust's standard library.
//!
//! The engine never touches the outside world itself (ADR-0012 §5): clocks, environment variables, files, sockets and other processes are the host's, and the host passes the engine what it needs through the protocol's host services. Nothing that affects the engine's output may depend on the clock or the environment (ADR-0005 §5), and the engine reads no environment variables (architecture §12). Rust's standard library reaches all of these through five modules: `std::env` (environment variables, the command line, the current folder), `std::fs` (files), `std::net` (sockets), `std::process` (other processes, and ending this one) and `std::time` (clocks). So the code that ships may name none of them, except in the workspace members that `OUTSIDE_WORLD` in `verify.rs` lists, with the modules that each may use and the reason.
//!
//! # Why it is not a lint
//!
//! A lint at `deny` or `warn` can be switched off by `#[allow(…)]` or `#[expect(…)]`, and a macro can assemble such an attribute in a form that neither the source check nor Clippy's `allow_attributes` lint sees (`sources.rs`, "Known limitation"). Only a lint at `forbid` cannot be lowered, but Clippy reads one configuration for the whole workspace (`clippy.toml`) and gives a lint one level in every crate, so a ban of these modules there would also hold for the members that must reach the outside world, such as xtask and the command-line tools, with no way to exempt them. This check is therefore not a lint at all: xtask reads the source text itself, so no attribute changes what it finds, however it is written or assembled.
//!
//! # The code it reads
//!
//! It reads exactly the Rust files that the compiler compiled into code that ships. After its runs, the Clippy step hands it, for every member, the files that the dependency files list for the member's units that are not tests (not compiled as unit tests, and not a test, benchmark, example or build-script target), among the files that the source check read (`dep_info.rs`). A module that only the tests use, declared with `#[cfg(test)] mod name;`, is left out by that record itself, because the compiler does not read its file for those units. Inside the files that remain, a module written in place under `#[cfg(test)]`, as in `#[cfg(test)] mod tests { … }`, is skipped too; every other item counts, also one under `#[cfg(test)]`. So tests may still measure time, read fixtures and run programs.
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
//! **Known limitations.** These rules model Rust's paths; they are not the compiler's name resolution. A path that never writes the module's name is not seen: a dependency that re-exports a module of std under another name, or a macro of a dependency that expands to such a path. Dependencies are reviewed when they are added (ADR-0017). In the WebAssembly build, where std has no clock, files or environment of its own, the import check of `cargo xtask wasm-package` (`wasm_imports.rs`) shows every function that the module asks of its JavaScript host, whoever calls it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::policy::MemberDependency;
use crate::sources::{self, Kind, Token};

/// The modules of Rust's standard library that reach the outside world, as code writes them.
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
const WHY: &str = "The engine never touches the outside world itself (ADR-0012 §5): clocks, environment variables, files, sockets and other processes are the host's, which passes the engine what it needs through the protocol's host services, and nothing that affects the engine's output may depend on the clock or the environment (ADR-0005 §5; the engine reads no environment variables, architecture §12). Rust's standard library reaches the outside world through std::env, std::fs, std::net, std::process and std::time, so the code that ships may name none of them, except in the members that OUTSIDE_WORLD in xtask/src/verify.rs lists, each with the modules it may use. Ask the host for what the code needs instead (a time, the bytes of a file, a setting), or, if only tests need it, move it into a test: integration tests, and modules written in place under #[cfg(test)], may use these modules. A member that really needs one is added to OUTSIDE_WORLD, with the module and the reason, in a reviewed pull request.";

/// How the uses of the modules are reported: the heading of their list.
const USED: &str =
    "the code that ships uses modules of the standard library that reach the outside world";

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

/// The module of [`MODULES`] whose last word is `name`, if there is one.
fn module(name: &str) -> Option<&'static str> {
    MODULES
        .iter()
        .copied()
        .find(|module| module.strip_prefix("std::") == Some(name))
}

/// Finds in Rust source the uses of [`MODULES`] and the ways of naming std, or of building a path in a macro, that could reach them unseen, outside the modules written in place under `#[cfg(test)]`. Comments, strings and character literals are skipped, as the source check skips them. Source the reader cannot follow is an error.
fn scan(source: &str) -> Result<Found, String> {
    let tokens = sources::tokenize(source)?;
    let closing = sources::closing_brackets(&tokens)?;
    let skipped = test_modules(&tokens, &closing);
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    let mut found = Found::default();
    let mut at = 0;
    while at < tokens.len() {
        if let Some(&end) = skipped.get(&at) {
            at = end + 1;
            continue;
        }
        let line = tokens[at].line;
        match tokens[at].kind {
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
        at += 1;
    }
    Ok(found)
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

/// The modules written in place under `#[cfg(test)]` (`#[cfg(test)] mod name { … }`, with any other attributes before or after it and any visibility), as the position of the `#` of the first attribute before each, mapped to the position of its closing `}`. Only the plain condition counts, as only it makes Clippy treat a module as test code (AGENTS.md).
fn test_modules(tokens: &[Token<'_>], closing: &[Option<usize>]) -> BTreeMap<usize, usize> {
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    let test_condition = [
        Kind::Ident("cfg"),
        Kind::Punct(b'('),
        Kind::Ident("test"),
        Kind::Punct(b')'),
    ];
    let mut found = BTreeMap::new();
    let mut at = 0;
    while at < tokens.len() {
        // A run of outer attributes, `#[…] #[…] …`.
        let start = at;
        let mut test = false;
        while kind(at) == Some(Kind::Punct(b'#')) && kind(at + 1) == Some(Kind::Punct(b'[')) {
            let Some(close) = closing[at + 1] else {
                break;
            };
            test |= tokens[at + 2..close]
                .iter()
                .map(|token| token.kind)
                .eq(test_condition);
            at = close + 1;
        }
        if at == start {
            at += 1;
            continue;
        }
        if !test {
            continue;
        }
        let mut item = at;
        if kind(item) == Some(Kind::Ident("pub")) {
            item += 1;
            if kind(item) == Some(Kind::Punct(b'('))
                && let Some(close) = closing[item]
            {
                item = close + 1;
            }
        }
        if kind(item) == Some(Kind::Ident("mod"))
            && matches!(kind(item + 1), Some(Kind::Ident(_)))
            && kind(item + 2) == Some(Kind::Punct(b'{'))
            && let Some(close) = closing[item + 2]
        {
            found.insert(start, close);
            at = close + 1;
        }
    }
    found
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
            "OUTSIDE_WORLD in xtask/src/verify.rs is not right:\n{}\nEach entry names a workspace member, the modules of the standard library that reach the outside world ({}) that its code that ships may use, and the reason.",
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
        "outside world: the code that ships of the members that OUTSIDE_WORLD does not name ({}; {} files) uses none of {}, and that of the members it names ({} files) uses exactly the modules it lists for them",
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
    fn skips_the_modules_written_in_place_under_cfg_test() {
        let source = "use std::sync::Arc;\n#[cfg(test)]\n#[cfg(not(target_arch = \"wasm32\"))]\nmod tests {\n    use std::time::Duration;\n    #[test] fn waits() { std::thread::sleep(Duration::from_millis(1)); }\n}\n#[doc = \"helpers\"]\n#[cfg(test)]\npub(crate) mod helpers { use std::fs; }\nfn after() { std::env::args(); }\n";
        assert_eq!(uses(source), ["std::env"]);
    }

    #[test]
    fn counts_everything_else_under_cfg_test() {
        // Only a module written in place counts as test code; other items under the condition, and other conditions, do not.
        assert_eq!(uses("#[cfg(test)] use std::time::Instant;"), ["std::time"]);
        assert_eq!(
            uses("#[cfg(test)] fn clock() { std::time::Instant::now(); }"),
            ["std::time"]
        );
        assert_eq!(
            uses("#[cfg(any(test, debug_assertions))] mod checks { use std::env; }"),
            ["std::env"]
        );
        assert_eq!(
            uses("#[cfg(not(test))] mod real { use std::fs; }"),
            ["std::fs"]
        );
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
            "  tool.rs:2: std::net (OUTSIDE_WORLD lists only std::env, std::fs, std::process for tool)",
            HIDDEN.0,
            "  hiding.rs:1: `std` on its own",
            STALE.0,
            "  std::process, for tool",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
        // The test module of engine.rs is not code that ships.
        assert!(!problem.contains("std::fs\n"), "{problem}");
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
                "the members that OUTSIDE_WORLD does not name (engine; 1 files) uses none of std::env, std::fs, std::net, std::process, std::time, and that of the members it names (1 files) uses exactly"
            ),
            "{line}"
        );
    }
}
