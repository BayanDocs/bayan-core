//! A panic inside a message handler produces `engine.error`, and the engine stays usable (CORE-007 AC-3, engine protocol §12, ADR-0006 §4).
//!
//! The tests that make the engine panic, and the one that starts the engine thread, run on the host only. The gate also runs the tests in WebAssembly (wasm32-wasip1), where a panic aborts the whole test program instead of unwinding, so the engine cannot catch it, and no thread can be started. The web worker host recovers from such a panic by replacing the WebAssembly instance instead (engine protocol §3.2); the tests that need no panic run in WebAssembly too.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use crate::support::{engine_with, error_code, events, ok_payload, open_document, request, send};
#[cfg(not(target_arch = "wasm32"))]
use bayan_engine::blobs::BlobStore;
use bayan_engine::config::Config;
#[cfg(not(target_arch = "wasm32"))]
use bayan_engine::recording::replay;
#[cfg(not(target_arch = "wasm32"))]
use bayan_engine::thread::EngineThread;
use serde_json::{Value, json};

fn panicking() -> Config {
    Config { allow_panic: true }
}

// Host only: it makes the engine panic (see the module documentation).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_forced_panic_becomes_an_error_and_a_fresh_session() {
    let mut engine = engine_with(panicking());
    let doc_id = open_document(&mut engine);
    let answers = send(&mut engine, &request(3, "diag.panic", json!({})));
    // The request gets its answer, and the shell learns that the session is gone.
    assert_eq!(error_code(&answers, 3), "panic");
    let errors = events(&answers, "engine.error");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["payload"]["code"], json!("panic"));
    assert_eq!(
        errors[0]["payload"]["message_id"],
        json!("engine-error-panic")
    );
    assert_eq!(errors[0]["payload"]["recoverable"], json!(false));
    // The old session's document is gone; a new handshake starts a working session.
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(4, "query.a11y", json!({ "doc_id": doc_id }))
            ),
            4
        ),
        "handshake_required"
    );
    let reopened = open_document(&mut engine);
    let tree = send(
        &mut engine,
        &request(5, "query.a11y", json!({ "doc_id": reopened })),
    );
    assert_eq!(ok_payload(&tree, 5)["root"]["role"], json!("document"));
}

// Host only: it makes the engine panic (see the module documentation).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_panic_without_a_request_identifier_is_still_reported() {
    let mut engine = engine_with(panicking());
    open_document(&mut engine);
    let answers = send(&mut engine, &json!({ "v": 0, "type": "diag.panic" }));
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0]["type"], json!("engine.error"));
    assert_eq!(answers[0]["payload"]["recoverable"], json!(false));
}

#[test]
fn a_new_engine_reports_the_panic_that_stopped_its_predecessor() {
    // The web worker host replaces a WebAssembly instance that panicked and lets the new engine report it (engine protocol §3.2).
    let mut engine = engine_with(panicking());
    let answers: Vec<Value> = engine
        .report_panic(Some(7))
        .iter()
        .map(|json| serde_json::from_str(json).unwrap())
        .collect();
    assert_eq!(answers.len(), 2);
    assert_eq!(error_code(&answers, 7), "panic");
    let errors = events(&answers, "engine.error");
    assert_eq!(errors[0]["payload"]["recoverable"], json!(false));
    // Event numbers start again at 1 in the new instance (§4).
    assert_eq!(errors[0]["seq"], json!(1));
    // Without an identifier, or with one that is not a request identifier, only the event remains.
    for id in [None, Some(0), Some(1 << 53)] {
        let answers = engine.report_panic(id);
        assert_eq!(answers.len(), 1);
        assert!(answers[0].contains(r#""type":"engine.error""#));
    }
    // The new engine works.
    open_document(&mut engine);
}

#[test]
fn without_the_test_switch_diag_panic_is_refused() {
    let mut engine = engine_with(Config::default());
    open_document(&mut engine);
    let answers = send(&mut engine, &request(3, "diag.panic", json!({})));
    assert_eq!(error_code(&answers, 3), "not_allowed");
    let reason = &answers[0]["error"]["args"]["reason"];
    assert_eq!(reason, &json!("test_switch_off"));
    // Nothing was lost.
    let manifest = send(&mut engine, &request(4, "ui.manifest", json!({})));
    assert!(ok_payload(&manifest, 4)["commands"].is_array());
}

// Host only: it makes the engine panic (see the module documentation).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn event_numbers_keep_increasing_across_a_panic() {
    let mut engine = engine_with(panicking());
    let doc_id = open_document(&mut engine);
    let before = send(
        &mut engine,
        &json!({ "v": 0, "type": "input.text", "payload": { "doc_id": doc_id, "text": "a" } }),
    );
    let last_before = before
        .iter()
        .filter_map(|message| message["seq"].as_u64())
        .max()
        .unwrap();
    let panic = send(&mut engine, &json!({ "v": 0, "type": "diag.panic" }));
    assert_eq!(panic[0]["seq"].as_u64(), Some(last_before + 1));
}

// Host only: it makes the engine panic (see the module documentation).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_recording_reproduces_a_panic() {
    let mut engine = engine_with(panicking());
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    send(
        &mut engine,
        &request(4, "input.text", json!({ "doc_id": doc_id, "text": "x" })),
    );
    send(&mut engine, &request(5, "diag.panic", json!({})));
    // The recording kept running across the panic.
    let answers = send(
        &mut engine,
        &request(1, "hello", json!({ "protocol_versions": [0] })),
    );
    assert_eq!(ok_payload(&answers, 1)["protocol_version"], json!(0));
    let stopped = send(&mut engine, &request(6, "diag.record.stop", json!({})));
    let stopped = ok_payload(&stopped, 6);
    assert_eq!(stopped["entries"], json!(3));
    let recording = engine
        .blobs()
        .take(stopped["blob"].as_u64().unwrap())
        .unwrap();
    // Replayed with the test switch on, the panic happens again, identically.
    assert!(replay(panicking(), &recording).unwrap().identical);
    // Replayed without it, the engine refuses diag.panic, so the replay differs at that entry.
    assert_eq!(
        replay(Config::default(), &recording)
            .unwrap()
            .first_difference,
        Some(1)
    );
}

// Host only: it starts the engine thread and makes the engine panic (see the module documentation).
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_engine_thread_survives_a_panic() {
    let blobs = Arc::new(BlobStore::new());
    let blob = blobs.put_shell(b"").unwrap();
    let thread = EngineThread::spawn(panicking(), blobs).unwrap();
    let (sender, receiver) = mpsc::channel::<String>();
    let sender = Mutex::new(sender);
    thread
        .set_sink(Arc::new(move |message: &str| {
            let _ignored = sender.lock().unwrap().send(message.to_owned());
        }))
        .unwrap();
    let post = |message: Value| {
        thread
            .post(serde_json::to_vec(&message).unwrap(), 0)
            .unwrap()
    };
    let next = || -> Value {
        serde_json::from_str(&receiver.recv_timeout(Duration::from_secs(10)).unwrap()).unwrap()
    };
    post(request(1, "hello", json!({ "protocol_versions": [0] })));
    assert_eq!(next()["re"], json!(1));
    post(request(2, "diag.panic", json!({})));
    assert_eq!(next()["error"]["code"], json!("panic"));
    assert_eq!(next()["payload"]["recoverable"], json!(false));
    post(request(3, "hello", json!({ "protocol_versions": [0] })));
    assert_eq!(next()["ok"], json!(true));
    post(request(4, "doc.open", json!({ "blob": blob })));
    assert_eq!(next()["type"], json!("doc.opened"));
    thread.stop();
}
