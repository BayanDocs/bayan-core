//! The JSON Schema of engine protocol v0, generated from the Rust types ([engine protocol specification][spec] §9, ADR-0012 §1).
//!
//! schemars derives the schema of every payload from the same definitions serde uses ([`crate::protocol`]): payloads the shell sends are described as the engine reads them (optional fields may be left out), payloads the engine sends as it writes them. This module adds the envelopes of spec §4 and the list of messages, `x-bayan-messages`, from which [`crate::typescript`] writes the TypeScript declarations. The schema follows JSON Schema draft 2020-12; keywords that start with `x-bayan-` are annotations for our generators, which validators ignore.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use schemars::generate::SchemaSettings;
use serde_json::{Map, Value, json};

use crate::limits::MAX_ID;
use crate::protocol::{Direction, ErrorInfo, MESSAGES, PROTOCOL_VERSION, schema_of};

/// The schema's identifier.
pub const SCHEMA_ID: &str = "urn:bayandocs:engine-protocol:v0";

/// The file name under which the schema is published (spec §9).
pub const SCHEMA_FILE: &str = "engine-protocol.schema.json";

/// A reference to a definition of the schema, as text.
fn reference(name: &str) -> Value {
    json!({ "$ref": format!("#/$defs/{name}") })
}

/// The reference that a schemars subschema holds, such as `#/$defs/Hello`.
fn reference_of(schema: &schemars::Schema) -> Result<String, String> {
    schema
        .as_object()
        .and_then(|object| object.get("$ref"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "a payload type was inlined instead of defined by name".to_owned())
}

/// Whether a definition has required properties.
fn has_required(definitions: &Map<String, Value>, reference: &str) -> bool {
    reference
        .strip_prefix("#/$defs/")
        .and_then(|name| definitions.get(name))
        .and_then(|definition| definition.get("required"))
        .and_then(Value::as_array)
        .is_some_and(|required| !required.is_empty())
}

/// The JSON Schema of the protocol.
///
/// # Errors
///
/// Fails if two payload types share a name but differ between the directions, which the protocol's types must avoid.
pub fn json_schema() -> Result<Value, String> {
    // Payloads the shell sends, as the engine reads them; payloads the engine sends, as it writes them.
    let mut reading = SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator();
    let mut writing = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let error_reference = reference_of(&schema_of::<ErrorInfo>(&mut writing))?;

    struct Request {
        name: &'static str,
        payload: String,
    }
    let mut requests = Vec::new();
    let mut events = Vec::new();
    let mut listed = Vec::new();
    for spec in &MESSAGES {
        match spec.direction {
            Direction::ToEngine => {
                let payload = reference_of(&(spec.payload)(&mut reading))?;
                let mut entry = json!({
                    "type": spec.name,
                    "direction": "to_engine",
                    "payload": payload,
                });
                if let Some(reply) = spec.reply {
                    entry["reply"] = Value::String(reference_of(&reply(&mut writing))?);
                }
                if let Some(reply_type) = spec.reply_type {
                    entry["reply_type"] = Value::String(reply_type.to_owned());
                }
                if let Some(web_reply) = spec.web_reply {
                    entry["web_reply"] = Value::String(reference_of(&web_reply(&mut writing))?);
                }
                listed.push(entry);
                requests.push(Request {
                    name: spec.name,
                    payload,
                });
            }
            Direction::ToShell => {
                let payload = reference_of(&(spec.payload)(&mut writing))?;
                listed.push(json!({
                    "type": spec.name,
                    "direction": "to_shell",
                    "payload": payload,
                }));
                events.push(json!({
                    "properties": {
                        "type": { "const": spec.name },
                        "payload": { "$ref": payload },
                    },
                }));
            }
        }
    }

    let mut definitions = reading.take_definitions(true);
    for (name, definition) in writing.take_definitions(true) {
        match definitions.get(&name) {
            Some(existing) if *existing != definition => {
                return Err(format!(
                    "the type {name} is described differently when the shell sends it and when the engine sends it; give one of them another name"
                ));
            }
            Some(_) => {}
            None => {
                definitions.insert(name, definition);
            }
        }
    }

    let variants: Vec<Value> = requests
        .iter()
        .map(|request| {
            let mut variant = json!({
                "properties": {
                    "type": { "const": request.name },
                    "payload": { "$ref": request.payload },
                },
            });
            if has_required(&definitions, &request.payload) {
                variant["required"] = json!(["payload"]);
            }
            variant
        })
        .collect();
    let version = json!({ "const": PROTOCOL_VERSION, "description": "The protocol version." });
    let identifier = json!({
        "type": "integer",
        "minimum": 1,
        "maximum": MAX_ID,
        "description": "A request identifier, below 2^53.",
    });
    let envelopes = [
        (
            "Request",
            json!({
                "description": "A message from the shell to the engine (spec §4). Without `id`, the engine sends no reply.",
                "type": "object",
                "required": ["v", "type"],
                "properties": {
                    "v": version,
                    "id": identifier,
                    "type": { "type": "string" },
                    "payload": { "type": "object" },
                },
                "oneOf": variants,
            }),
        ),
        (
            "Reply",
            json!({
                "description": "The engine's reply to a request with an `id` (spec §4).",
                "oneOf": [reference("ReplyOk"), reference("ReplyError")],
            }),
        ),
        (
            "ReplyOk",
            json!({
                "description": "A successful reply. Its payload has the reply type that `x-bayan-messages` lists for the request.",
                "type": "object",
                "required": ["v", "re", "ok", "payload"],
                "properties": {
                    "v": version,
                    "re": identifier,
                    "ok": { "const": true },
                    "type": { "type": "string", "description": "The reply's name, where the specification gives one (`welcome`, `doc.opened`)." },
                    "payload": { "type": "object" },
                },
            }),
        ),
        (
            "ReplyError",
            json!({
                "description": "A failed reply.",
                "type": "object",
                "required": ["v", "re", "ok", "error"],
                "properties": {
                    "v": version,
                    "re": identifier,
                    "ok": { "const": false },
                    "error": { "$ref": error_reference },
                },
            }),
        ),
        (
            "Event",
            json!({
                "description": "A message the engine sends without being asked (spec §4).",
                "type": "object",
                "required": ["v", "type", "seq", "payload"],
                "properties": {
                    "v": version,
                    "type": { "type": "string" },
                    "seq": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_ID,
                        "description": "Increases by one with every event an engine instance sends.",
                    },
                    "payload": { "type": "object" },
                },
                "oneOf": events,
            }),
        ),
    ];
    for (name, definition) in envelopes {
        if definitions.insert(name.to_owned(), definition).is_some() {
            return Err(format!("a payload type is named {name}, like an envelope"));
        }
    }

    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": SCHEMA_ID,
        "title": "BayanDocs engine protocol, version 0",
        "description": "Every message of engine protocol v0 (BayanDocs docs repository, specs/engine-protocol.md), generated from the engine's Rust types. A message is a request, a reply or an event.",
        "x-bayan-protocol-version": PROTOCOL_VERSION,
        "x-bayan-messages": listed,
        "oneOf": [reference("Request"), reference("Reply"), reference("Event")],
        "$defs": definitions,
    }))
}

/// The schema as the published file: pretty-printed JSON with a final line feed.
///
/// # Errors
///
/// As [`json_schema`].
pub fn json_schema_file() -> Result<String, String> {
    let schema = json_schema()?;
    let mut text = serde_json::to_string_pretty(&schema).map_err(|error| error.to_string())?;
    text.push('\n');
    Ok(text)
}
