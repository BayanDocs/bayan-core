//! The guardrail against code that hides from Clippy.
//!
//! When Clippy compiles a crate, it sets the condition `clippy`; a normal build does not. Code under `#[cfg(not(clippy))]` is therefore compiled by the tests and in every release, but Clippy never sees it, so none of the bans of `clippy.toml` apply to it: `#[cfg(not(clippy))] fn sine(x: f64) -> f64 { x.sin() }`, next to a `#[cfg(clippy)]` twin that Clippy checks instead, passes every other step of the gate.
//!
//! Every lint path starts with `clippy::`, so the bare identifier `clippy` (not followed by `::`) has no legitimate use in this repository's code. This check rejects it anywhere outside comments, strings and character literals: in `cfg`, `cfg_attr` and `cfg!`, as a raw identifier (`r#clippy`), as the name of an item or variable, and as an argument handed to a macro, such as `m!(clippy)`.
//!
//! # The files it reads
//!
//! The check is only as good as the files it reads. It reads every `.rs` file below every member's folder, and it rejects every way it knows to make the compiler read another Rust file for a workspace member, except the macros described under "Not covered":
//!
//! - The only folder the walk skips is the workspace's build folder, `<root>/target`, and no member's folder may contain that.
//! - Every target's root file (`src_path` in `cargo metadata`, which a path in `[lib]`, `[[bin]]`, `[[test]]`, `[[bench]]` or `[[example]]`, or `build = "…"`, can move) must be a `.rs` file inside its crate's folder. From there, `mod name;` reaches only `name.rs` or `name/mod.rs` below it.
//! - `include!` is rejected: the identifier `include` anywhere, so that a macro cannot be handed it either. `include_str!` and `include_bytes!` read data, not code, and stay allowed.
//! - So is the attribute `#[path = "…"]`, wherever `path =` stands inside an attribute, also in `cfg_attr`.
//! - So are a symbolic link in a member's folder, and a file that the file system also finds under a name ending in `.rs` although its own name does not end so, as Windows and macOS give the compiler `sine.RS` when it asks for `sine.rs`.
//!
//! # Not covered
//!
//! A macro can build the condition, or the attribute, from tokens that look harmless where it is called. `macro_rules! lint_exempt { ($tool:ident :: $lint:ident, $item:item, $twin:item) => { #[cfg(not($tool))] $item #[cfg($tool)] $twin }; }`, called as `lint_exempt!(clippy::disallowed_methods, …)`, is handed a lint path, which this check allows, and keeps only `clippy`. In the same way, a macro that writes `#[$attribute] mod name;` can be handed `path = "…"`. Both pass the gate. Closing them needs a decision on what macros may write, so until then reviewers look for macros that build attributes from their arguments (AGENTS.md, "Lint rules and exceptions").
//!
//! Source that the check cannot follow, such as a comment or string that is never closed, is an error, so that nothing can hide behind a misreading.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use crate::policy::Package;

/// The identifier that Clippy sets as a condition.
const CLIPPY: &str = "clippy";

/// The macro that compiles another file as if its text stood in this one.
const INCLUDE: &str = "include";

/// The attribute that makes `mod` read its file from another place.
const PATH: &str = "path";

/// The workspace's build folder, relative to the root: the only folder the walk skips.
const BUILD_FOLDER: &str = "target";

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

/// A token of Rust source, as far as this check needs to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// An identifier, with `r#` removed from a raw identifier, and the line it is on.
    Ident(&'a str, usize),
    /// The path separator `::`.
    PathSeparator,
    /// One other ASCII punctuation character, such as `#`, `[` or `=`.
    Punct(u8),
    /// A literal, a number, a lifetime or label, or other text.
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
}

/// Finds in Rust source the bare identifier `clippy` (not followed by `::`), the identifier `include`, and `path =` inside an attribute (`#[…]` or `#![…]`, at any depth, so also inside `cfg_attr`). Comments, strings (also raw, byte and C strings), character literals and lifetimes are skipped. Source the reader cannot follow, such as a comment or string that is never closed, is an error, so nothing can hide behind a misreading.
fn findings(source: &str) -> Result<Findings, String> {
    let tokens = tokenize(source)?;
    let mut found = Findings::default();
    // How deeply `[` brackets nest at this point, and, inside an attribute, the depth at which it ends.
    let mut depth = 0_usize;
    let mut attribute_ends = None;
    for (index, token) in tokens.iter().enumerate() {
        let next = tokens.get(index + 1).copied();
        match *token {
            Token::Ident(CLIPPY, line) if next != Some(Token::PathSeparator) => {
                found.bare_clippy.push(line);
            }
            Token::Ident(INCLUDE, line) => found.include_macro.push(line),
            Token::Ident(PATH, line)
                if attribute_ends.is_some() && next == Some(Token::Punct(b'=')) =>
            {
                found.path_attribute.push(line);
            }
            Token::Punct(b'#') if attribute_ends.is_none() => {
                // `#[` or `#![` opens an attribute.
                let bracket = if next == Some(Token::Punct(b'!')) {
                    index + 2
                } else {
                    index + 1
                };
                if tokens.get(bracket) == Some(&Token::Punct(b'[')) {
                    attribute_ends = Some(depth);
                }
            }
            Token::Punct(b'[') => depth += 1,
            Token::Punct(b']') => {
                depth = depth.saturating_sub(1);
                if attribute_ends == Some(depth) {
                    attribute_ends = None;
                }
            }
            _ => {}
        }
    }
    Ok(found)
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
        if rest.starts_with("//") {
            position += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            position = block_comment_end(source, position)
                .ok_or_else(|| format!("line {line}: a block comment is never closed"))?;
        } else if byte == b'"' {
            position = string_end(source, position + 1)
                .ok_or_else(|| format!("line {line}: a string is never closed"))?;
            tokens.push(Token::Other);
        } else if byte == b'\'' {
            position = quote_end(source, position)
                .ok_or_else(|| format!("line {line}: a character literal is never closed"))?;
            tokens.push(Token::Other);
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
                tokens.push(Token::Other);
            } else if matches!(word, "b" | "c") && after.starts_with('"') {
                position = string_end(source, word_end + 1)
                    .ok_or_else(|| format!("line {line}: a string is never closed"))?;
                tokens.push(Token::Other);
            } else if word == "b" && after.starts_with('\'') {
                position = quote_end(source, word_end)
                    .ok_or_else(|| format!("line {line}: a byte literal is never closed"))?;
                tokens.push(Token::Other);
            } else if word == "r"
                && hashes == 1
                && after[1..].starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            {
                // A raw identifier, r#name.
                let name_end = ident_end(source, word_end + 1);
                tokens.push(Token::Ident(&source[word_end + 1..name_end], line));
                position = name_end;
            } else {
                tokens.push(Token::Ident(word, line));
                position = word_end;
            }
        } else if byte.is_ascii_digit() {
            position = ident_end(source, position);
            tokens.push(Token::Other);
        } else if rest.starts_with("::") {
            tokens.push(Token::PathSeparator);
            position += 2;
        } else if byte.is_ascii_whitespace() {
            position += 1;
        } else if byte.is_ascii() {
            tokens.push(Token::Punct(byte));
            position += 1;
        } else {
            // Non-ASCII text outside comments and strings; identifiers are ASCII (`non_ascii_idents` is denied).
            position += rest.chars().next().map_or(1, char::len_utf8);
            tokens.push(Token::Other);
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

/// Checks the Rust sources of the workspace members (`packages`, from `cargo metadata`): that the compiler is not made to read a Rust file of theirs that this check does not read (short of the macros described above), and that none of the files it reads uses the bare identifier `clippy`. Returns one line describing what was checked, or every problem found.
pub fn check(root: &Path, packages: &[Package]) -> Result<String, String> {
    let build_folder = root.join(BUILD_FOLDER);
    let misplaced = misplaced_roots(root, &build_folder, packages);
    let mut walk = Walk::default();
    for package in packages {
        walk_folder(&package.folder, &build_folder, &mut walk)?;
    }
    let mut include_places = Vec::new();
    let mut path_places = Vec::new();
    let mut clippy_places = Vec::new();
    for file in &walk.files {
        let source = std::fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let shown = shown(root, file);
        let found = findings(&source).map_err(|error| {
            format!("{shown}: {error}. The gate must be able to read every Rust file to check it.")
        })?;
        let places = |lines: &[usize]| {
            lines
                .iter()
                .map(|line| format!("  {shown}:{line}"))
                .collect::<Vec<_>>()
        };
        include_places.extend(places(&found.include_macro));
        path_places.extend(places(&found.path_attribute));
        clippy_places.extend(places(&found.bare_clippy));
    }
    let unread: Vec<String> = walk
        .unread
        .iter()
        .map(|(entry, why)| format!("  {} ({why})", shown(root, entry)))
        .collect();
    let problems: Vec<String> = [
        (misplaced, MISPLACED_ROOTS),
        (unread, UNREAD_ENTRIES),
        (include_places, INCLUDE_USED),
        (path_places, PATH_USED),
        (clippy_places, CLIPPY_USED),
    ]
    .into_iter()
    .filter(|(places, _)| !places.is_empty())
    .map(|(places, (heading, why))| format!("{heading}:\n{}\n{why}", places.join("\n")))
    .collect();
    if problems.is_empty() {
        return Ok(format!(
            "Rust sources: none of the {} `.rs` files below the members' folders uses the bare identifier `clippy`, `include!` or `#[path]`; every target starts from one of them, and no member's folder holds a symbolic link",
            walk.files.len()
        ));
    }
    Err(problems.join("\n"))
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

    fn found(source: &str) -> Findings {
        findings(source).unwrap_or_else(|problem| panic!("{problem}"))
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
            "#[cfg_attr(unix, cfg_attr(all(), path = \"sine.in\"))] mod sine;",
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
// A comment may say cfg(not(clippy)), include!("x") or #[path = "x"].
/* A block comment /* nested */ may say clippy too. */
/// So may documentation: `#[cfg(clippy)]`.
fn run() {
    let command = "cargo clippy -- -D warnings";
    let raw = r#"cfg(not(clippy)) #[path = "x"] include!("x")"#;
    let bytes = b"clippy";
    let raw_bytes = br"clippy";
    let c_string = c"clippy";
    let data = include_bytes!("data.bin");
    let quote = '"';
    let escaped = '\'';
    let byte = b'\'';
    let unicode = '\u{1F600}';
    let label: &'static str = "clippy";
    'outer: loop { break 'outer; }
    let clippy_config = 1; // a different identifier
    let not_clippy = clippy_config;
    let included = include_str!("data.txt");
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
        ] {
            assert!(findings(source).is_err(), "{source:?}");
        }
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

        /// A member at `crates/<name>` whose library starts from `root_file`, a path relative to the member's folder as it would be written in `[lib]`.
        fn member(&self, name: &str, root_file: &str) -> Package {
            let folder = self.0.join("crates").join(name);
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
        let line = check(&scratch.0, &packages).unwrap_or_else(|problem| panic!("{problem}"));
        assert!(line.contains("none of the 4 `.rs` files"), "{line}");

        scratch.write(
            &["crates", "a", "src", "target", "hidden.rs"],
            "\n#[cfg(not(clippy))]\npub fn sine(x: f64) -> f64 { x.sin() }\n",
        );
        let problem = check(&scratch.0, &packages).unwrap_err();
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
            "//! A.\ninclude!(\"sine.in\");\n#[path = \"../../../shared/sine.rs\"]\nmod sine;\n#[cfg(clippy)]\nfn twin() {}\n",
        );
        let mut moved = scratch.member("b", "../../shared/units.rs");
        moved.targets.push(Target {
            kinds: vec!["bin".to_owned()],
            name: "tool".to_owned(),
            root_file: moved.folder.join("src").join("main.in"),
        });
        scratch.write(&["crates", "b", "src", "main.in"], "fn main() {}\n");
        let problem = check(&scratch.0, &[scratch.member("a", "src/lib.rs"), moved]).unwrap_err();
        let lib = relative(&["crates", "a", "src", "lib.rs"]);
        for expected in [
            format!("`include!` (the identifier `include`) is used here:\n  {lib}:2\n"),
            format!("the attribute `#[path = \"…\"]` is used here:\n  {lib}:3\n"),
            format!(
                "the bare identifier `clippy` (not followed by `::`) is used here:\n  {lib}:5\n"
            ),
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
        let problem = check(&scratch.0, &[package]).unwrap_err();
        assert!(
            problem.contains("contains the build folder target/"),
            "{problem}"
        );
        assert!(!problem.contains("out.rs"), "{problem}");
    }

    /// Creates a symbolic link, or returns false where the system does not allow it (Windows without developer mode).
    fn symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        made.is_ok()
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
        let problem = check(&scratch.0, &[scratch.member("a", "src/lib.rs")]).unwrap_err();
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

    #[cfg(unix)]
    #[test]
    fn rejects_entries_that_are_neither_files_nor_folders() {
        let scratch = Scratch::new("socket");
        scratch.write(&["crates", "a", "src", "lib.rs"], "//! A.\n");
        let _socket =
            std::os::unix::net::UnixListener::bind(scratch.0.join("crates").join("a").join("s"))
                .unwrap();
        let problem = check(&scratch.0, &[scratch.member("a", "src/lib.rs")]).unwrap_err();
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
        let result = check(&scratch.0, &[scratch.member("a", "src/lib.rs")]);
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
        check(&root, &packages).unwrap_or_else(|problem| panic!("{problem}"));
    }
}
