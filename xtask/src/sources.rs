//! The guardrail against code that hides from Clippy.
//!
//! When Clippy compiles a crate, it sets the condition `clippy`; a normal build does not. Code under `#[cfg(not(clippy))]` is therefore compiled by the tests and in every release, but Clippy never sees it, so none of the bans of `clippy.toml` apply to it: `#[cfg(not(clippy))] fn sine(x: f64) -> f64 { x.sin() }`, next to a `#[cfg(clippy)]` twin that Clippy checks instead, passes every step of the gate. The same works through `cfg_attr`, `cfg!` and macros that build the condition, such as `m!(clippy)`.
//!
//! Every lint path starts with `clippy::`, so the bare identifier `clippy` (not followed by `::`) has no legitimate use in this repository's code. This check reads every Rust file of every workspace member and rejects that identifier anywhere outside comments, strings and character literals.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The identifier that Clippy sets as a condition.
const IDENTIFIER: &str = "clippy";

/// Folders that are never searched: build output and Git's data.
const NOT_SEARCHED: [&str; 2] = ["target", ".git"];

/// A token of Rust source, as far as this check needs to tell them apart.
#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    /// An identifier, with `r#` removed from a raw identifier, and the line it is on.
    Ident(&'a str, usize),
    /// The path separator `::`.
    PathSeparator,
    /// Any other punctuation, number, or literal.
    Other,
}

/// The lines (counting from 1) on which `source` uses the bare identifier `clippy`, that is, not followed by `::`. Comments, strings (also raw, byte and C strings), character literals and lifetimes are skipped. Source the reader cannot follow, such as a comment or string that is never closed, is an error, so nothing can hide behind a misreading.
pub fn bare_identifier_lines(source: &str) -> Result<Vec<usize>, String> {
    let tokens = tokenize(source)?;
    let mut lines = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        if let Token::Ident(name, line) = token
            && *name == IDENTIFIER
            && tokens.get(index + 1) != Some(&Token::PathSeparator)
        {
            lines.push(*line);
        }
    }
    Ok(lines)
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
            tokens.push(Token::Other);
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

/// Checks every Rust file below the given folders (the workspace members' folders) for the bare identifier `clippy`. Returns one line describing what was checked, or every place it was found.
pub fn check(root: &Path, folders: &[PathBuf]) -> Result<String, String> {
    let mut files = BTreeSet::new();
    for folder in folders {
        rust_files(folder, &mut files)?;
    }
    let mut problems = Vec::new();
    for file in &files {
        let source = std::fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        let shown = file
            .strip_prefix(root)
            .unwrap_or(file)
            .display()
            .to_string();
        let lines = bare_identifier_lines(&source).map_err(|error| {
            format!("{shown}: {error}. The gate must be able to read every Rust file to check it for `cfg(clippy)`.")
        })?;
        for line in lines {
            problems.push(format!("  {shown}:{line}"));
        }
    }
    if problems.is_empty() {
        return Ok(format!(
            "no code hides from Clippy: the bare identifier `clippy` appears in none of the {} Rust files of the workspace",
            files.len()
        ));
    }
    Err(format!(
        "the bare identifier `clippy` (not followed by `::`) is used here:\n{}\nClippy sets the condition `clippy` when it compiles a crate, and a normal build does not, so code under `#[cfg(not(clippy))]` is compiled and shipped but never checked by Clippy: the bans of clippy.toml (ADR-0005) do not apply to it. The gate therefore rejects the identifier anywhere outside comments and strings, also inside `cfg_attr`, `cfg!` and macros. Lint paths such as `clippy::unwrap_used` are fine. Remove the condition, or rename an item or variable that happens to be called `clippy`.",
        problems.join("\n")
    ))
}

/// Adds every `.rs` file below `folder` to `files`, skipping `target` and `.git` folders. Symbolic links are not followed.
fn rust_files(folder: &Path, files: &mut BTreeSet<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(folder)
        .map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot read {}: {error}", folder.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if file_type.is_dir() {
            if !NOT_SEARCHED
                .iter()
                .any(|skip| entry.file_name() == OsStr::new(skip))
            {
                rust_files(&path, files)?;
            }
        } else if file_type.is_file() && path.extension() == Some(OsStr::new("rs")) {
            files.insert(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: &str) -> Vec<usize> {
        bare_identifier_lines(source).unwrap_or_else(|problem| panic!("{problem}"))
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
            assert_eq!(found(&format!("\n{source}\n")), [2], "{source}");
        }
    }

    #[test]
    fn allows_lint_paths_comments_and_strings() {
        let source = r##"
#![expect(clippy::print_stdout, reason = "the clippy step prints")]
#[expect(clippy :: unwrap_used, reason = "spacing around :: is still a path")]
// A comment may say cfg(not(clippy)).
/* A block comment /* nested */ may say clippy too. */
/// So may documentation: `#[cfg(clippy)]`.
fn run() {
    let command = "cargo clippy -- -D warnings";
    let raw = r#"cfg(not(clippy))"#;
    let bytes = b"clippy";
    let raw_bytes = br"clippy";
    let c_string = c"clippy";
    let quote = '"';
    let escaped = '\'';
    let byte = b'\'';
    let unicode = '\u{1F600}';
    let label: &'static str = "clippy";
    'outer: loop { break 'outer; }
    let clippy_config = 1; // a different identifier
    let not_clippy = clippy_config;
}
"##;
        assert_eq!(found(source), Vec::<usize>::new());
    }

    #[test]
    fn counts_lines_across_multi_line_comments_and_strings() {
        let source = "/* one\ntwo */ let s = \"a\nb\";\nr#\"x\ny\"#;\nclippy";
        assert_eq!(found(source), [6]);
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
            assert!(bare_identifier_lines(source).is_err(), "{source:?}");
        }
    }

    #[test]
    fn checks_every_rust_file_below_the_members() {
        let root = std::env::temp_dir().join(format!("bayan-xtask-{}-sources", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |relative: &str, text: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write("crates/a/src/lib.rs", "//! A.\n");
        write("crates/a/tests/t.rs", "#[test]\nfn t() {}\n");
        write(
            "crates/a/target/debug/build/out.rs",
            "#[cfg(clippy)] fn skipped() {}\n",
        );
        write("crates/a/notes.txt", "cfg(not(clippy))\n");
        // Built from separate parts, so the expected paths match on Windows too.
        let member = root.join("crates").join("a");
        let folders = [member.clone(), member];
        let line = check(&root, &folders).unwrap_or_else(|problem| panic!("{problem}"));
        assert!(line.contains("none of the 2 Rust files"), "{line}");

        write(
            "crates/a/src/hidden.rs",
            "\n#[cfg(not(clippy))]\npub fn sine(x: f64) -> f64 { x.sin() }\n",
        );
        let problem = check(&root, &folders).unwrap_err();
        let hidden = Path::new("crates").join("a").join("src").join("hidden.rs");
        let shown = format!("  {}:2", hidden.display());
        assert!(problem.contains(&shown), "{problem}");
        assert!(problem.contains("never checked by Clippy"), "{problem}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_real_workspace_does_not_hide_from_clippy() {
        let root = crate::workspace_root();
        check(&root, &[root.join("crates"), root.join("xtask")])
            .unwrap_or_else(|problem| panic!("{problem}"));
    }
}
