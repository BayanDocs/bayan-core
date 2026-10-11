//! The JSON Schema of the corpus manifest, generated from the Rust types of [`crate::manifest`] so that the schema cannot drift from the code.
//!
//! The committed copy is `lab/corpus/manifest.schema.json`; a test fails when it differs from what this module generates. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab --test lab` and review the difference.

use schemars::generate::SchemaSettings;
use serde_json::Value;

use crate::manifest::Manifest;

/// The schema's identifier.
pub const SCHEMA_ID: &str = "urn:bayandocs:lab:corpus-manifest:v1";

/// The JSON Schema (draft 2020-12) of a manifest, as the tools read it.
#[must_use]
pub fn json_schema() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator();
    let mut schema = generator.into_root_schema_for::<Manifest>().to_value();
    if let Value::Object(object) = &mut schema {
        object.insert("$id".to_owned(), Value::String(SCHEMA_ID.to_owned()));
        object.insert(
            "title".to_owned(),
            Value::String("BayanDocs Fidelity Lab corpus manifest".to_owned()),
        );
    }
    schema
}

/// The schema as the text of the committed file: pretty-printed, with a final line feed.
///
/// # Errors
///
/// Never in practice: the schema is plain JSON.
pub fn json_schema_file() -> Result<String, String> {
    let mut text =
        serde_json::to_string_pretty(&json_schema()).map_err(|error| error.to_string())?;
    text.push('\n');
    Ok(text)
}
