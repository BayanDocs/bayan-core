//! Test support for bayan-engine's integration tests: a JSON Schema validator for the part of draft 2020-12 that the protocol's schema uses, and helpers that talk to an engine like a shell.

pub mod schema_validator;

use std::sync::Arc;

use bayan_engine::Engine;
use bayan_engine::blobs::BlobStore;
use bayan_engine::config::Config;
use serde_json::{Value, json};

/// An engine with the given configuration and a fresh blob store.
pub fn engine_with(config: Config) -> Engine {
    Engine::new(config, Arc::new(BlobStore::new()))
}

/// An engine with the default configuration.
pub fn engine() -> Engine {
    engine_with(Config::default())
}

/// Sends a message and returns the engine's messages, parsed.
pub fn send(engine: &mut Engine, message: &Value) -> Vec<Value> {
    let text = serde_json::to_vec(message).unwrap();
    engine
        .handle(&text, 0)
        .iter()
        .map(|json| serde_json::from_str(json).unwrap())
        .collect()
}

/// A request envelope.
pub fn request(id: u64, kind: &str, payload: Value) -> Value {
    json!({ "v": 0, "id": id, "type": kind, "payload": payload })
}

/// A fire-and-forget envelope.
pub fn notice(kind: &str, payload: Value) -> Value {
    json!({ "v": 0, "type": kind, "payload": payload })
}

/// The reply among `messages` to request `id`.
pub fn reply_to(messages: &[Value], id: u64) -> &Value {
    messages
        .iter()
        .find(|message| message["re"] == json!(id))
        .unwrap_or_else(|| panic!("no reply to {id} in {messages:?}"))
}

/// The payload of the successful reply to request `id`.
pub fn ok_payload(messages: &[Value], id: u64) -> &Value {
    let reply = reply_to(messages, id);
    assert_eq!(reply["ok"], json!(true), "{reply}");
    &reply["payload"]
}

/// The error code of the failed reply to request `id`.
pub fn error_code(messages: &[Value], id: u64) -> String {
    let reply = reply_to(messages, id);
    assert_eq!(reply["ok"], json!(false), "{reply}");
    reply["error"]["code"].as_str().unwrap().to_owned()
}

/// The events of a type among `messages`.
pub fn events<'a>(messages: &'a [Value], kind: &str) -> Vec<&'a Value> {
    messages
        .iter()
        .filter(|message| message.get("re").is_none() && message["type"] == json!(kind))
        .collect()
}

/// Says hello, puts an empty blob and opens the mock document; returns its document identifier.
pub fn open_document(engine: &mut Engine) -> u64 {
    let hello = send(
        engine,
        &request(1, "hello", json!({ "protocol_versions": [0] })),
    );
    assert_eq!(ok_payload(&hello, 1)["protocol_version"], json!(0));
    let blob = engine.blobs().put_shell(b"").unwrap();
    let opened = send(engine, &request(2, "doc.open", json!({ "blob": blob })));
    ok_payload(&opened, 2)["doc_id"].as_u64().unwrap()
}

/// A `view.set` payload at a zoom factor.
pub fn view(doc_id: u64, zoom: f64) -> Value {
    json!({
        "doc_id": doc_id,
        "viewport": { "x": 0, "y": 0, "width": 1024, "height": 768 },
        "zoom": zoom,
        "device_scale": 2,
        "mode": "print",
    })
}

/// The whole of page `page` of the mock document, as a `render.tile` payload of `width` × `height` pixels.
pub fn whole_page(doc_id: u64, page: u32, width: u32, height: u32) -> Value {
    json!({
        "doc_id": doc_id,
        "page": page,
        "rect": { "x": 0, "y": 0, "width": 15_544_800, "height": 20_116_800 },
        "width": width,
        "height": height,
        "zoom": 1,
        "device_scale": 1,
        "mode": "interactive",
    })
}
