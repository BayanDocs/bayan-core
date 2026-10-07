//! Acceptance criterion 3 of CORE-002: the public interface exposes no floating-point types except the one explicitly named lossy conversion, `Blu::to_f64_lossy` in `src/lossy.rs`.
//!
//! The test checks something stronger and easier to check reliably: no source file of the crate except `src/lossy.rs` names `f32` or `f64` at all, in code or in comments. A public item can only take or return a floating-point value by naming its type somewhere in the crate, so the public interface is covered. It reads the source files, so it runs on the host only; in WebAssembly, the tests have no access to files, and the sources are the same anyway.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::{Path, PathBuf};

/// Every `.rs` file below `folder`.
fn rust_files(folder: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(folder).expect("readable source folder") {
        let path = entry.expect("readable folder entry").path();
        if path.is_dir() {
            rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// Whether `line` contains `f32` or `f64` as a word of its own.
fn names_a_float_type(line: &str) -> bool {
    let part_of_a_word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    ["f32", "f64"].iter().any(|name| {
        line.match_indices(name).any(|(start, _)| {
            !part_of_a_word(line[..start].chars().next_back())
                && !part_of_a_word(line[start + name.len()..].chars().next())
        })
    })
}

#[test]
fn recognizes_the_float_type_names_as_words() {
    assert!(names_a_float_type("pub fn to_f64_lossy(self) -> f64 {"));
    assert!(names_a_float_type("let x: f32 = 1.0;"));
    assert!(names_a_float_type("(f64)"));
    assert!(!names_a_float_type("let buf64 = 0; // a hex value 0xff64"));
    assert!(!names_a_float_type("fn to_f64_lossy(self)"));
}

#[test]
fn only_the_lossy_conversion_names_a_floating_point_type() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&source, &mut files);
    assert!(
        files.iter().any(|file| file.ends_with("lossy.rs")),
        "the walk must find src/lossy.rs"
    );
    assert!(files.len() >= 10, "the walk found only {files:?}");
    let mut found = Vec::new();
    for file in files.iter().filter(|file| !file.ends_with("lossy.rs")) {
        let text = fs::read_to_string(file).expect("readable source file");
        for (number, line) in text.lines().enumerate() {
            if names_a_float_type(line) {
                found.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        found.is_empty(),
        "bayan-units exposes no floating point except Blu::to_f64_lossy (CORE-002, AC-3; ADR-0005 §3), but these lines name f32 or f64:\n{}",
        found.join("\n")
    );
}
