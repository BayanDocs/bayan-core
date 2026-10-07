//! The TypeScript declarations of engine protocol v0, for the web shell ([engine protocol specification][spec] §9, ADR-0012 §1).
//!
//! The declarations are written from the JSON Schema ([`crate::schema`]), not from the Rust types directly, so the two cannot disagree. They describe the messages as the web shell sees them through the worker host (spec §3.2): a schema definition marked `x-bayan-web-type` becomes that browser type, so blob fields are `ArrayBuffer`s and the reply to `render.tile` carries an `ImageBitmap`. The generator understands exactly the parts of JSON Schema that schemars produces for our types and fails on anything else, so a new kind of type cannot be silently mistranslated.
//!
//! [spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md

use std::fmt::Write as _;

use serde_json::{Map, Value};

/// The file name under which the declarations are published (spec §9).
pub const DECLARATIONS_FILE: &str = "engine-protocol.d.ts";

/// The envelope definitions of the schema, which the declarations write by hand from the message list instead.
const ENVELOPES: [&str; 5] = ["Request", "Reply", "ReplyOk", "ReplyError", "Event"];

/// Keywords that describe a value without constraining its type.
const ANNOTATIONS: [&str; 6] = [
    "description",
    "default",
    "title",
    "format",
    "minimum",
    "maximum",
];

fn unsupported(what: &str, schema: &Value) -> String {
    format!("the TypeScript generator does not understand {what}: {schema}")
}

/// A JSDoc comment, indented.
fn comment(out: &mut String, indent: &str, text: Option<&str>) {
    let Some(text) = text else {
        return;
    };
    let text = text.replace("*/", "*\\/");
    let _ignored = writeln!(out, "{indent}/** {text} */");
}

/// The name a reference points to, such as `Hello` for `#/$defs/Hello`.
fn referenced(reference: &str) -> Result<&str, String> {
    reference
        .strip_prefix("#/$defs/")
        .ok_or_else(|| format!("the reference {reference} does not point into $defs"))
}

/// A TypeScript literal for a JSON value used as a constant.
fn literal(value: &Value) -> Result<String, String> {
    match value {
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {
            serde_json::to_string(value).map_err(|error| error.to_string())
        }
        Value::Array(_) | Value::Object(_) => {
            Err(unsupported("a constant that is not a scalar", value))
        }
    }
}

/// Wraps a union in parentheses, for use inside an array type.
fn grouped(text: String) -> String {
    if text.contains(" | ") {
        format!("({text})")
    } else {
        text
    }
}

/// The TypeScript type of a schema.
fn type_of(schema: &Value) -> Result<String, String> {
    if schema == &Value::Bool(true) {
        return Ok("unknown".to_owned());
    }
    let Some(object) = schema.as_object() else {
        return Err(unsupported("this schema", schema));
    };
    for key in object.keys() {
        let known = ANNOTATIONS.contains(&key.as_str())
            || matches!(
                key.as_str(),
                "$ref"
                    | "type"
                    | "const"
                    | "enum"
                    | "oneOf"
                    | "anyOf"
                    | "items"
                    | "properties"
                    | "required"
                    | "additionalProperties"
            );
        if !known && !key.starts_with("x-") {
            return Err(unsupported(&format!("the keyword {key}"), schema));
        }
    }
    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return Ok(referenced(reference)?.to_owned());
    }
    if let Some(constant) = object.get("const") {
        return literal(constant);
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        let literals: Result<Vec<String>, String> = values.iter().map(literal).collect();
        return Ok(literals?.join(" | "));
    }
    for union in ["oneOf", "anyOf"] {
        if let Some(members) = object.get(union).and_then(Value::as_array) {
            let types: Result<Vec<String>, String> = members.iter().map(type_of).collect();
            let mut types = types?;
            types.dedup();
            return Ok(types.join(" | "));
        }
    }
    match object.get("type") {
        Some(Value::String(kind)) => type_of_kind(kind, object),
        Some(Value::Array(kinds)) => {
            let types: Result<Vec<String>, String> = kinds
                .iter()
                .map(|kind| {
                    kind.as_str()
                        .ok_or_else(|| unsupported("a type that is not a string", schema))
                        .and_then(|kind| type_of_kind(kind, object))
                })
                .collect();
            Ok(types?.join(" | "))
        }
        _ => Err(unsupported("a schema without a type", schema)),
    }
}

/// The TypeScript type of one JSON type of a schema.
fn type_of_kind(kind: &str, object: &Map<String, Value>) -> Result<String, String> {
    match kind {
        "string" => Ok("string".to_owned()),
        "integer" | "number" => Ok("number".to_owned()),
        "boolean" => Ok("boolean".to_owned()),
        "null" => Ok("null".to_owned()),
        "array" => {
            let items = object
                .get("items")
                .map_or(Ok("unknown".to_owned()), type_of)?;
            Ok(format!("{}[]", grouped(items)))
        }
        "object" => {
            if object.contains_key("properties") {
                let mut out = String::from("{\n");
                properties(&mut out, "  ", object)?;
                out.push('}');
                return Ok(out);
            }
            match object.get("additionalProperties") {
                Some(values) if values.is_object() => {
                    Ok(format!("{{ [key: string]: {} }}", type_of(values)?))
                }
                _ => Ok("Record<string, never>".to_owned()),
            }
        }
        _ => Err(format!(
            "the TypeScript generator does not understand the JSON type {kind}"
        )),
    }
}

/// The properties of an object schema, one per line, optional ones marked with `?`.
fn properties(out: &mut String, indent: &str, object: &Map<String, Value>) -> Result<(), String> {
    let required: Vec<&str> = object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(Value::Object(properties)) = object.get("properties") else {
        return Ok(());
    };
    for (name, schema) in properties {
        comment(
            out,
            indent,
            schema.get("description").and_then(Value::as_str),
        );
        let optional = if required.contains(&name.as_str()) {
            ""
        } else {
            "?"
        };
        let _ignored = writeln!(out, "{indent}{name}{optional}: {};", type_of(schema)?);
    }
    Ok(())
}

/// One definition of the schema as a TypeScript declaration.
fn definition(out: &mut String, name: &str, schema: &Value) -> Result<(), String> {
    comment(out, "", schema.get("description").and_then(Value::as_str));
    if let Some(web_type) = schema.get("x-bayan-web-type").and_then(Value::as_str) {
        let _ignored = writeln!(out, "export type {name} = {web_type};");
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or_else(|| unsupported("this definition", schema))?;
    if object.get("type").and_then(Value::as_str) == Some("object")
        && object.contains_key("properties")
    {
        let _ignored = writeln!(out, "export interface {name} {{");
        properties(out, "  ", object)?;
        out.push_str("}\n");
    } else {
        let _ignored = writeln!(out, "export type {name} = {};", type_of(schema)?);
    }
    Ok(())
}

/// One entry of `x-bayan-messages`.
struct Message<'a> {
    name: &'a str,
    to_engine: bool,
    payload: &'a str,
    reply: Option<&'a str>,
}

fn messages(schema: &Value) -> Result<Vec<Message<'_>>, String> {
    let list = schema
        .get("x-bayan-messages")
        .and_then(Value::as_array)
        .ok_or("the schema has no x-bayan-messages")?;
    list.iter()
        .map(|entry| {
            let text = |key: &str| entry.get(key).and_then(Value::as_str);
            let name = text("type").ok_or("a message without a type")?;
            let payload = referenced(text("payload").ok_or("a message without a payload")?)?;
            // The web worker host delivers some replies differently (spec §3.2).
            let reply = text("web_reply")
                .or_else(|| text("reply"))
                .map(referenced)
                .transpose()?;
            Ok(Message {
                name,
                to_engine: text("direction") == Some("to_engine"),
                payload,
                reply,
            })
        })
        .collect()
}

/// A map from message types to payload types.
fn map(
    out: &mut String,
    name: &str,
    description: &str,
    entries: impl Iterator<Item = (String, String)>,
) {
    comment(out, "", Some(description));
    let _ignored = writeln!(out, "export interface {name} {{");
    for (message, payload) in entries {
        let _ignored = writeln!(out, "  \"{message}\": {payload};");
    }
    out.push_str("}\n\n");
}

const PREAMBLE: &str = "\
// TypeScript declarations of the BayanDocs engine protocol, version 0 (docs repository: specs/engine-protocol.md).
// Generated by bayan-engine from the JSON Schema of the engine's Rust types (engine-protocol.schema.json); do not edit.
// They describe the messages as the web shell exchanges them with the worker host: blob fields hold ArrayBuffers, and the
// reply to render.tile carries an ImageBitmap (specification §3.2). Version 0 is unstable and may change between core releases.

/** The protocol version these declarations describe. */
export type ProtocolVersion = 0;

";

const ENVELOPE_TYPES: &str = "\
/** The types of the messages the shell sends. */
export type RequestType = keyof Requests;

/** The types of the events the engine sends. */
export type EventType = keyof Events;

/** A message from the shell to the engine (spec §4). Without `id`, the engine sends no reply. */
export type EngineRequest<T extends RequestType = RequestType> = {
  [K in T]: { v: ProtocolVersion; id?: number; type: K; payload: Requests[K] };
}[T];

/** A successful reply to a request of type `T` (spec §4). */
export interface EngineReplyOk<T extends RequestType = RequestType> {
  v: ProtocolVersion;
  re: number;
  ok: true;
  /** The reply's name, where the specification gives one (`welcome`, `doc.opened`). */
  type?: string;
  payload: Replies[T];
}

/** A failed reply (spec §4). */
export interface EngineReplyError {
  v: ProtocolVersion;
  re: number;
  ok: false;
  error: ErrorInfo;
}

/** The reply to a request of type `T`. */
export type EngineReply<T extends RequestType = RequestType> = EngineReplyOk<T> | EngineReplyError;

/** An event from the engine (spec §4). */
export type EngineEvent<T extends EventType = EventType> = {
  [K in T]: { v: ProtocolVersion; type: K; seq: number; payload: Events[K] };
}[T];

/** Any message from the engine. */
export type EngineMessage = EngineReply | EngineEvent;

/** The engine configuration (spec §3.3). */
export interface EngineConfig {
  test?: {
    /** Lets `diag.panic` make the engine panic, for tests of the error path. */
    allow_panic?: boolean;
  };
}

/** The worker host's first message from the main thread, optional (spec §3.2). */
export interface WorkerInit {
  worker: \"init\";
  config?: EngineConfig;
  /** The address of bayan_wasm_bg.wasm, resolved against the worker script's address. */
  wasm_url?: string;
}

/** The worker host is ready. */
export interface WorkerReady {
  worker: \"ready\";
  engine_version: string;
}

/**
 * The worker host stopped working, and ignores every later message: the configuration is invalid (`invalid_config`), the
 * engine module could not be loaded (`load_failed`), or the engine could not be replaced after a panic (`restart_failed`).
 */
export interface WorkerFailed {
  worker: \"failed\";
  reason: \"invalid_config\" | \"load_failed\" | \"restart_failed\";
}

/** A control message from the worker host. */
export type WorkerStatus = WorkerReady | WorkerFailed;
";

/// The TypeScript declarations for a protocol schema.
///
/// # Errors
///
/// Fails if the schema contains a construct the generator does not understand.
pub fn declarations(schema: &Value) -> Result<String, String> {
    let definitions = schema
        .get("$defs")
        .and_then(Value::as_object)
        .ok_or("the schema has no $defs")?;
    let messages = messages(schema)?;
    let mut out = String::from(PREAMBLE);
    map(
        &mut out,
        "Requests",
        "The payloads of the messages the shell sends, by message type.",
        messages
            .iter()
            .filter(|message| message.to_engine)
            .map(|message| (message.name.to_owned(), message.payload.to_owned())),
    );
    map(
        &mut out,
        "Replies",
        "The payloads of successful replies, by the type of the request they answer.",
        messages
            .iter()
            .filter_map(|message| Some((message.name.to_owned(), message.reply?.to_owned()))),
    );
    map(
        &mut out,
        "Events",
        "The payloads of the events the engine sends, by event type.",
        messages
            .iter()
            .filter(|message| !message.to_engine)
            .map(|message| (message.name.to_owned(), message.payload.to_owned())),
    );
    out.push_str(ENVELOPE_TYPES);
    for (name, schema) in definitions {
        if ENVELOPES.contains(&name.as_str()) {
            continue;
        }
        out.push('\n');
        definition(&mut out, name, schema)?;
    }
    Ok(out)
}

/// The declarations of this engine's protocol, as the published file.
///
/// # Errors
///
/// As [`crate::schema::json_schema`] and [`declarations`].
pub fn declarations_file() -> Result<String, String> {
    declarations(&crate::schema::json_schema()?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn translates_the_constructs_schemars_produces() {
        let cases = [
            (json!({"type": "string"}), "string"),
            (
                json!({"type": ["integer", "null"], "format": "uint32", "minimum": 0}),
                "number | null",
            ),
            (
                json!({"type": "array", "items": {"$ref": "#/$defs/Rect"}}),
                "Rect[]",
            ),
            (
                json!({"type": ["array", "null"], "items": {"$ref": "#/$defs/Rect"}}),
                "Rect[] | null",
            ),
            (
                json!({"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "null"}]}}),
                "(string | null)[]",
            ),
            (json!({"type": "array", "items": true}), "unknown[]"),
            (
                json!({"anyOf": [{"$ref": "#/$defs/Caret"}, {"type": "null"}]}),
                "Caret | null",
            ),
            (
                json!({"oneOf": [{"const": "a", "type": "string"}, {"const": "b", "type": "string"}]}),
                "\"a\" | \"b\"",
            ),
            (
                json!({"type": "object", "additionalProperties": {"$ref": "#/$defs/ArgValue"}}),
                "{ [key: string]: ArgValue }",
            ),
            (json!({"type": "object"}), "Record<string, never>"),
        ];
        for (schema, expected) in cases {
            assert_eq!(type_of(&schema).unwrap(), expected, "{schema}");
        }
    }

    #[test]
    fn refuses_what_it_does_not_understand() {
        for schema in [
            json!({"type": "string", "pattern": "^a"}),
            json!({"allOf": [{"type": "string"}]}),
            json!({"$ref": "https://example.org/other"}),
            json!(false),
            json!({"type": "tuple"}),
        ] {
            assert!(type_of(&schema).is_err(), "{schema}");
        }
    }

    #[test]
    fn writes_interfaces_and_web_types() {
        let mut out = String::new();
        let schema = json!({
            "description": "A thing.",
            "type": "object",
            "required": ["a"],
            "properties": {"a": {"type": "integer", "description": "First."}, "b": {"type": "boolean"}},
        });
        definition(&mut out, "Thing", &schema).unwrap();
        assert_eq!(
            out,
            "/** A thing. */\nexport interface Thing {\n  /** First. */\n  a: number;\n  b?: boolean;\n}\n"
        );
        let mut out = String::new();
        let blob = json!({"type": "integer", "x-bayan-web-type": "ArrayBuffer"});
        definition(&mut out, "BlobRef", &blob).unwrap();
        assert_eq!(out, "export type BlobRef = ArrayBuffer;\n");
    }
}
