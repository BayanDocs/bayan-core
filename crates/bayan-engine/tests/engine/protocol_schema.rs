//! The JSON Schema and the TypeScript declarations match the engine's Rust types (CORE-007 AC-4).
//!
//! - The committed copies in `crates/bayan-engine/protocol/` are exactly what the generators produce from the current types. After changing a protocol type, regenerate them with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-engine --test engine` and review the difference like any other change.
//! - Sample messages, both those a shell sends and every message the engine sent during a session that uses every v0 message, validate against the schema; messages that break it are refused by the schema and by the engine alike.

use std::path::PathBuf;

use crate::support::schema_validator::Validator;
use crate::support::{
    engine, error_code, events, notice, ok_payload, open_document, reply_to, request, send, view,
    whole_page,
};
use bayan_engine::protocol::{Direction, MESSAGES};
use bayan_engine::{schema, typescript};
use serde_json::{Value, json};

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("protocol")
}

/// Compares a committed generated file with what the generator produces now, or rewrites it when `BAYAN_UPDATE_GENERATED` is set.
fn check_generated(file: &str, generated: &str) {
    let path = generated_dir().join(file);
    if std::env::var_os("BAYAN_UPDATE_GENERATED").is_some() {
        std::fs::create_dir_all(generated_dir()).unwrap();
        std::fs::write(&path, generated).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        committed == generated,
        "{} is out of date with the protocol's Rust types. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-engine --test engine` and review the difference.",
        path.display()
    );
}

#[test]
fn the_committed_schema_is_current() {
    check_generated(schema::SCHEMA_FILE, &schema::json_schema_file().unwrap());
}

#[test]
fn the_committed_typescript_declarations_are_current() {
    check_generated(
        typescript::DECLARATIONS_FILE,
        &typescript::declarations_file().unwrap(),
    );
}

#[test]
fn the_schema_lists_every_v0_message() {
    let schema = schema::json_schema().unwrap();
    let listed: Vec<&str> = schema["x-bayan-messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["type"].as_str().unwrap())
        .collect();
    let expected: Vec<&str> = MESSAGES.iter().map(|message| message.name).collect();
    assert_eq!(listed, expected);
}

/// The reply payload definition for a request type, as `x-bayan-messages` lists it.
fn reply_definition(schema: &Value, kind: &str) -> String {
    let entry = schema["x-bayan-messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["type"] == json!(kind))
        .unwrap();
    entry["reply"]
        .as_str()
        .unwrap()
        .trim_start_matches("#/$defs/")
        .to_owned()
}

/// Every request a shell sends in the session below, with the type of each.
fn session_requests(doc_id: u64, blob: u64) -> Vec<Value> {
    vec![
        request(10, "view.set", view(doc_id, 1.0)),
        request(11, "render.tile", whole_page(doc_id, 0, 51, 66)),
        request(12, "input.text", json!({ "doc_id": doc_id, "text": "Hi" })),
        request(
            13,
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "start" }),
        ),
        request(
            14,
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "update", "text": "にほ", "selection": { "start": 1, "end": 2 } }),
        ),
        request(
            15,
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "end" }),
        ),
        request(
            16,
            "input.key",
            json!({ "doc_id": doc_id, "key": "Backspace", "code": "Backspace" }),
        ),
        request(
            17,
            "input.pointer",
            json!({ "doc_id": doc_id, "kind": "down", "x": 120.5, "y": 210, "button": 0, "clicks": 1, "pointer_type": "mouse", "modifiers": { "shift": true } }),
        ),
        request(18, "query.a11y", json!({ "doc_id": doc_id })),
        request(19, "ui.manifest", json!({ "locale": "en-US" })),
        request(20, "diag.record.start", json!({})),
        request(21, "input.text", json!({ "doc_id": doc_id, "text": "!" })),
        request(22, "diag.record.stop", json!({})),
        request(23, "doc.open", json!({ "blob": blob })),
        request(24, "doc.close", json!({ "doc_id": doc_id + 1 })),
        request(25, "diag.panic", json!({})),
        notice("input.text", json!({ "doc_id": 999, "text": "x" })),
    ]
}

#[test]
fn every_message_of_a_session_validates_against_the_schema() {
    let schema = schema::json_schema().unwrap();
    let validator = Validator::new(&schema);
    let mut engine = engine();
    let hello = request(
        1,
        "hello",
        json!({
            "protocol_versions": [0],
            "shell": { "name": "test", "version": "1", "platform": "linux" },
            "capabilities": { "clipboard_formats": [], "ime": true, "accessibility": true },
            "locale": "en-US",
            "theme": "light",
        }),
    );
    validator.validate(&hello).unwrap();
    let welcome = send(&mut engine, &hello);
    let blob = engine.blobs().put_shell(b"").unwrap();
    let open = request(
        2,
        "doc.open",
        json!({ "blob": blob, "format_hint": "docx" }),
    );
    validator.validate(&open).unwrap();
    let opened = send(&mut engine, &open);
    let doc_id = ok_payload(&opened, 2)["doc_id"].as_u64().unwrap();

    let mut seen_requests = vec!["hello".to_owned(), "doc.open".to_owned()];
    let mut seen_events = Vec::new();
    let mut checked = 0;
    let mut check_answers = |kind: &str, answers: &[Value], seen_events: &mut Vec<String>| {
        for message in answers {
            validator
                .validate(message)
                .unwrap_or_else(|error| panic!("{error}\n{message}"));
            if message.get("re").is_some() && message["ok"] == json!(true) {
                let definition = reply_definition(&schema, kind);
                validator
                    .validate_definition(&definition, &message["payload"])
                    .unwrap_or_else(|error| panic!("{error}\n{message}"));
            } else if message.get("seq").is_some() {
                seen_events.push(message["type"].as_str().unwrap().to_owned());
            }
            checked += 1;
        }
    };
    check_answers("hello", &welcome, &mut seen_events);
    check_answers("doc.open", &opened, &mut seen_events);
    let mut recording_blob = None;
    for message in session_requests(doc_id, blob) {
        validator
            .validate(&message)
            .unwrap_or_else(|error| panic!("{error}\n{message}"));
        let kind = message["type"].as_str().unwrap().to_owned();
        let answers = send(&mut engine, &message);
        if kind == "diag.record.stop" {
            recording_blob = ok_payload(&answers, 22)["blob"].as_u64();
        }
        check_answers(&kind, &answers, &mut seen_events);
        seen_requests.push(kind);
    }
    // The replay request needs the blob the recording came back in.
    let replay = request(
        26,
        "diag.replay",
        json!({ "blob": recording_blob.unwrap() }),
    );
    validator.validate(&replay).unwrap();
    let answers = send(&mut engine, &replay);
    assert_eq!(ok_payload(&answers, 26)["identical"], json!(true));
    check_answers("diag.replay", &answers, &mut seen_events);
    seen_requests.push("diag.replay".to_owned());
    assert!(checked > 30, "only {checked} messages were checked");

    // The session used every message type of the catalog.
    for message in &MESSAGES {
        let seen = match message.direction {
            Direction::ToEngine => seen_requests.iter().any(|kind| kind == message.name),
            Direction::ToShell => seen_events.iter().any(|kind| kind == message.name),
        };
        assert!(seen, "the session never used {}", message.name);
    }
}

#[test]
fn every_catalog_request_is_handled() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    for (index, message) in MESSAGES
        .iter()
        .filter(|message| message.direction == Direction::ToEngine)
        .enumerate()
    {
        let id = 100 + u64::try_from(index).unwrap();
        let answers = send(
            &mut engine,
            &request(id, message.name, json!({ "doc_id": doc_id })),
        );
        let reply = reply_to(&answers, id);
        if reply["ok"] == json!(false) {
            assert_ne!(
                reply["error"]["code"],
                json!("unsupported_message"),
                "{}",
                message.name
            );
        }
    }
}

#[test]
fn messages_that_break_the_schema_are_refused_by_the_engine_too() {
    let schema = schema::json_schema().unwrap();
    let validator = Validator::new(&schema);
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let broken = [
        request(30, "view.set", json!({ "doc_id": doc_id, "zoom": 1 })),
        request(
            31,
            "view.set",
            json!({ "doc_id": doc_id, "viewport": { "x": 0, "y": 0, "width": 1, "height": 1 }, "zoom": 1, "device_scale": 1, "mode": "sideways" }),
        ),
        request(
            32,
            "render.tile",
            json!({ "doc_id": doc_id, "page": -1, "rect": { "x": 0, "y": 0, "width": 1, "height": 1 }, "width": 1, "height": 1 }),
        ),
        request(
            33,
            "render.tile",
            json!({ "doc_id": doc_id, "page": 0, "rect": { "x": 0.5, "y": 0, "width": 1, "height": 1 }, "width": 1, "height": 1 }),
        ),
        request(34, "input.text", json!({ "doc_id": doc_id, "text": 5 })),
        request(
            35,
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "middle" }),
        ),
        request(
            36,
            "input.pointer",
            json!({ "doc_id": doc_id, "kind": "down", "x": "left", "y": 0 }),
        ),
        request(37, "doc.open", json!({ "blob": "seven" })),
        request(38, "hello", json!({ "protocol_versions": "0" })),
    ];
    for message in &broken {
        assert!(
            validator.validate(message).is_err(),
            "the schema accepts {message}"
        );
        let id = message["id"].as_u64().unwrap();
        let answers = send(&mut engine, message);
        assert_eq!(error_code(&answers, id), "invalid_request", "{message}");
    }
    // Envelopes that break the schema are refused too.
    for message in [
        json!({ "v": 0, "id": 40 }),
        json!({ "v": 0, "id": 0, "type": "ui.manifest" }),
        json!({ "v": 0, "id": 41, "type": "ui.manifest", "payload": [] }),
        json!({ "v": 1, "id": 42, "type": "ui.manifest" }),
    ] {
        assert!(
            validator.validate(&message).is_err(),
            "the schema accepts {message}"
        );
        let answers = send(&mut engine, &message);
        let error = answers[0]
            .get("error")
            .map_or(&answers[0]["payload"], |error| error);
        assert!(error["code"].is_string(), "{answers:?}");
    }
    assert!(
        events(
            &send(&mut engine, &json!({ "v": 0, "type": 5 })),
            "engine.error"
        )
        .len()
            == 1
    );
}

#[test]
fn the_validator_refuses_keywords_it_does_not_know() {
    let schema = json!({ "type": "string", "pattern": "^a" });
    assert!(Validator::new(&schema).validate(&json!("a")).is_err());
    let schema = json!({ "type": "integer", "format": "uint8" });
    assert!(Validator::new(&schema).validate(&json!(256)).is_err());
    assert!(Validator::new(&schema).validate(&json!(1.0)).is_err());
    assert!(Validator::new(&schema).validate(&json!(255)).is_ok());
}
