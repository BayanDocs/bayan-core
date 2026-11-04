//! The JSON Schema of the layout JSON, generated from the Rust types of [`crate::layout`] so that the schema cannot drift from the code.
//!
//! The committed copy is `lab/schema/layout.schema.json`, for producers that do not use these types, such as the Word harness of LAB-002; a test fails when it differs from what this module generates. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab-layout --test layout` and review the difference. The schema checks the shape of a file; [`crate::layout::Layout::check`] checks the rules between its fields as well.

use schemars::generate::SchemaSettings;
use serde_json::Value;

use crate::layout::{Layout, SCHEMA_ID};

/// The file name of the committed schema, in `lab/schema/`.
pub const SCHEMA_FILE: &str = "layout.schema.json";

/// The JSON Schema (draft 2020-12) of a layout JSON file, as the tools read it.
#[must_use]
pub fn json_schema() -> Value {
    let generator = SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator();
    let mut schema = generator.into_root_schema_for::<Layout>().to_value();
    if let Value::Object(object) = &mut schema {
        object.insert("$id".to_owned(), Value::String(SCHEMA_ID.to_owned()));
        object.insert(
            "title".to_owned(),
            Value::String("BayanDocs Fidelity Lab layout JSON, version 1".to_owned()),
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
