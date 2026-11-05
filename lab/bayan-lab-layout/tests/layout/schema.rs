//! AC-3: the layout JSON's schema is documented and validated against sample files from both sides.
//!
//! The samples in `lab/schema/samples/` are the two sides of a comparison: the PDF extractor's output for PDFs of LibreOffice and Chromium (Word's side, as the lab will extract it from Word's PDFs), a hand-written file of the engine's side with BLU, and a hand-written skeleton from Word's object model with sampled lines and no glyphs. Each must pass the JSON Schema, checked by an independent validator against the schema generated from the Rust types (which `the_committed_schema_is_current` keeps equal to the committed `lab/schema/layout.schema.json`), and the rules that only `Layout::check` can check; broken files must fail.

use bayan_lab_layout::layout::{Layout, Source};
use bayan_lab_layout::schema;
use serde_json::{Value, json};

use crate::support::schema_validator::Validator;

const SAMPLES: [(&str, &str); 4] = [
    (
        "pdf-libreoffice.json",
        include_str!("../../../schema/samples/pdf-libreoffice.json"),
    ),
    (
        "pdf-chromium.json",
        include_str!("../../../schema/samples/pdf-chromium.json"),
    ),
    (
        "engine.json",
        include_str!("../../../schema/samples/engine.json"),
    ),
    (
        "word.json",
        include_str!("../../../schema/samples/word.json"),
    ),
];

// Reads the committed schema, and rewrites it when asked, so it runs on the host only: the tests in WebAssembly have no files.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_committed_schema_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../schema")
        .join(schema::SCHEMA_FILE);
    let generated = schema::json_schema_file().unwrap();
    if std::env::var_os("BAYAN_UPDATE_GENERATED").is_some() {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        committed == generated,
        "{} is out of date with the layout JSON's Rust types. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab-layout --test layout` and review the difference.",
        path.display()
    );
}

// Reads both copies of the validator, so it runs on the host only: the tests in WebAssembly have no files.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_validator_is_bayan_engines() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let code = |path: &str| {
        // The module documentation says which copy a file is; the code below it must be the same.
        std::fs::read_to_string(root.join(path))
            .unwrap()
            .replace("\r\n", "\n")
            .lines()
            .filter(|line| !line.starts_with("//!"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let engine = code("crates/bayan-engine/tests/engine/support/schema_validator.rs");
    let ours = code("lab/bayan-lab-layout/tests/layout/support/schema_validator.rs");
    // The one function only the engine's tests use, which this copy leaves out.
    let validate_definition = "    }\n\n    /// Validates a value against the definition `name` of the document.\n    pub fn validate_definition(&self, name: &str, value: &Value) -> Result<(), String> {\n        let schema = self.definition(name)?;\n        self.check(schema, value, \"$\")\n";
    assert_eq!(
        engine.matches(validate_definition).count(),
        1,
        "bayan-engine's validator no longer has the `validate_definition` this test expects"
    );
    assert!(
        ours == engine.replacen(validate_definition, "", 1),
        "tests/layout/support/schema_validator.rs differs from bayan-engine's copy: change them together"
    );
}

/// Checks a file both ways: against the JSON Schema with the independent validator, and with the Rust types and their rules.
fn validate(text: &str) -> (Result<(), String>, Result<Layout, String>) {
    let schema = schema::json_schema();
    let value: Value = serde_json::from_str(text).unwrap();
    (
        Validator::new(&schema).validate(&value),
        Layout::from_json(text),
    )
}

#[test]
fn samples_from_both_sides_follow_the_schema_and_its_rules() {
    for (name, text) in SAMPLES {
        let (by_schema, by_rules) = validate(text);
        assert_eq!(by_schema, Ok(()), "{name} against the JSON Schema");
        let layout = by_rules.unwrap_or_else(|error| panic!("{name} against the rules: {error}"));
        // Written again, a sample is the same text: the format has one spelling.
        assert_eq!(
            serde_json::to_string_pretty(&layout).unwrap() + "\n",
            text.replace("\r\n", "\n"),
            "{name} round trip"
        );
    }
    let engine = Layout::from_json(SAMPLES[2].1).unwrap();
    assert_eq!(engine.source, Source::Engine);
    assert!(
        engine.pages[0].lines[0]
            .glyphs
            .as_deref()
            .unwrap()
            .iter()
            .all(|glyph| glyph.x_blu.is_some())
    );
    let word = Layout::from_json(SAMPLES[3].1).unwrap();
    assert_eq!(word.source, Source::Word);
    assert!(
        word.pages
            .iter()
            .flat_map(|page| &page.lines)
            .all(|line| line.glyphs.is_none() && line.text.is_none())
    );
}

/// A copy of the engine sample with one change.
fn engine_with(change: impl FnOnce(&mut Value)) -> String {
    let mut value: Value = serde_json::from_str(SAMPLES[2].1).unwrap();
    change(&mut value);
    value.to_string()
}

#[test]
fn the_schema_refuses_files_of_the_wrong_shape() {
    let cases: [(&str, String); 6] = [
        (
            "an unknown field",
            engine_with(|value| value["comment"] = json!("extra")),
        ),
        (
            "another version",
            engine_with(|value| value["schema"] = json!("urn:bayandocs:lab:layout:v2")),
        ),
        (
            "an unknown producer kind",
            engine_with(|value| value["source"] = json!("typst")),
        ),
        (
            "a missing page size",
            engine_with(|value| {
                value["pages"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("width")
                    .map(|_| ())
                    .unwrap()
            }),
        ),
        (
            "a glyph with a negative offset",
            engine_with(|value| value["pages"][0]["lines"][0]["glyphs"][0]["offset"] = json!(-1)),
        ),
        (
            "a length as a string",
            engine_with(|value| value["pages"][0]["height"] = json!("792")),
        ),
    ];
    for (what, text) in cases {
        let (by_schema, by_rules) = validate(&text);
        assert!(by_schema.is_err(), "the schema accepted {what}");
        assert!(by_rules.is_err(), "the types accepted {what}");
    }
}

#[test]
fn the_rules_refuse_what_the_schema_cannot_see() {
    let cases: [(&str, String, &str); 9] = [
        (
            "a page count that does not match",
            engine_with(|value| value["page_count"] = json!(2)),
            "page_count is 2 but there are 1 pages",
        ),
        (
            "a glyph beyond its line's text",
            engine_with(|value| value["pages"][0]["lines"][0]["glyphs"][0]["offset"] = json!(12)),
            "beyond the line's 12 characters",
        ),
        (
            "a font that does not exist",
            engine_with(|value| value["pages"][0]["lines"][0]["glyphs"][0]["font"] = json!(2)),
            "font 2 is not in fonts",
        ),
        (
            "a BLU value that does not match its points",
            engine_with(|value| {
                value["pages"][0]["lines"][0]["glyphs"][0]["x_blu"] = json!(1_828_826)
            }),
            "but its BLU value",
        ),
        (
            "a length with four decimals",
            engine_with(|value| value["pages"][0]["lines"][0]["glyphs"][0]["x"] = json!(72.0001)),
            "more than three decimals",
        ),
        (
            "a negative width",
            engine_with(|value| value["pages"][0]["width"] = json!(-612.0)),
            "width and height must be positive",
        ),
        (
            "a control character in a line",
            engine_with(|value| value["pages"][0]["lines"][0]["text"] = json!("Hello,\nworld")),
            "control or noncharacter U+000A",
        ),
        (
            "overlapping glyphs",
            engine_with(|value| value["pages"][0]["lines"][0]["glyphs"][1]["length"] = json!(2)),
            "overlapping",
        ),
        (
            "lines that go back in the document",
            engine_with(|value| {
                value["pages"][0]["lines"][0]["start"] = json!(20);
                value["pages"][0]["lines"][0]["end"] = json!(32);
            }),
            "before the start of the line",
        ),
    ];
    for (what, text, message) in cases {
        let (by_schema, by_rules) = validate(&text);
        if message != "more than three decimals" {
            assert_eq!(
                by_schema,
                Ok(()),
                "{what}: the schema alone cannot see this"
            );
        }
        let error = by_rules.expect_err(what);
        assert!(error.contains(message), "{what}: {error}");
    }
}

#[test]
fn errors_say_where_and_never_quote_a_value() {
    // A layout JSON holds a document's text: an error may name a position, never a value of the file.
    let secret = "Zanzibar";
    let cases = [
        (
            "a string where a length belongs",
            engine_with(|value| value["pages"][0]["width"] = json!(secret)),
        ),
        (
            "a text where a direction belongs",
            engine_with(|value| value["pages"][0]["lines"][0]["direction"] = json!(secret)),
        ),
        (
            "an unknown field",
            engine_with(|value| value[secret] = json!(1)),
        ),
        (
            "a number where a line's text belongs",
            engine_with(|value| value["pages"][0]["lines"][0]["text"] = json!(12)),
        ),
        ("text that is not JSON", format!("{{\"{secret}\": tru")),
        (
            "text that ends too early",
            format!("{{\"producer\": \"{secret}"),
        ),
    ];
    for (what, text) in cases {
        let error = Layout::from_json(&text).unwrap_err();
        assert!(!error.contains(secret), "{what}: {error}");
        assert!(
            error.starts_with("line ") && error.contains(", column "),
            "{what}: {error}"
        );
    }
    // The checks of lengths quote nothing, and keep their own messages.
    let precise = engine_with(|value| value["pages"][0]["width"] = json!(612.0001));
    let error = Layout::from_json(&precise).unwrap_err();
    assert!(
        error.starts_with("line ") && error.ends_with("a length has more than three decimals"),
        "{error}"
    );
}

#[test]
fn files_from_a_pdf_count_offsets_through_their_lines() {
    let mut value: Value = serde_json::from_str(SAMPLES[0].1).unwrap();
    // Each line starts one character after the previous one ends: the line feed between them.
    value["pages"][0]["lines"][1]["start"] =
        json!(value["pages"][0]["lines"][1]["start"].as_u64().unwrap() + 1);
    let error = Layout::from_json(&value.to_string()).unwrap_err();
    assert!(error.contains("but the line starts at offset"), "{error}");
    let mut value: Value = serde_json::from_str(SAMPLES[0].1).unwrap();
    value["pages"][0]["lines"][0]
        .as_object_mut()
        .unwrap()
        .remove("direction");
    let error = Layout::from_json(&value.to_string()).unwrap_err();
    assert!(error.contains("gives every line's direction"), "{error}");
    let mut value: Value = serde_json::from_str(SAMPLES[3].1).unwrap();
    value["pages"][0]["lines"][0]["glyphs"] = json!([]);
    let error = Layout::from_json(&value.to_string()).unwrap_err();
    assert!(error.contains("needs its text"), "{error}");
}
