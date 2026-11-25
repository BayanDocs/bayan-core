//! JSON nested as deeply as the engine reads it, [`MAX_JSON_DEPTH`] levels, is read on the stacks the engine runs on; one level more is refused before anything deeper is read; and values nested far more deeply, which the engine skips, are skipped without recursion (the architecture review of 2026-11-20, finding L14).
//!
//! Natively, the test harness runs each test on a thread of 2 MiB, the size of the engine thread's stack (`ENGINE_THREAD_STACK_BYTES`); the gate's WebAssembly tests run the same tests in Node.js, where WebAssembly runs on the JavaScript engine's stack as it does in a browser, whose size the module cannot set. A stack overflow ends the whole test program in either, so passing is the evidence. Only a configuration is read into a `serde_json::Value`, which recurses as deeply as its input nests; messages and recordings go into types of a fixed depth, and the values those skip are read without recursion, however deeply they nest.

use bayan_engine::config::Config;
use bayan_engine::limits::{MAX_JSON_DEPTH, MAX_MESSAGE_BYTES};
use bayan_engine::recording::replay;
use serde_json::{Value, json};

use crate::support::{engine, ok_payload, open_document, request, send};

/// Far deeper than serde_json would ever recurse: a million levels, two million bytes of brackets, well within the size of a message.
const FAR_DEEPER: usize = 1_000_000;

/// `levels` arrays, one inside the other, around `inner`.
fn nested(levels: usize, inner: &str) -> String {
    format!("{}{inner}{}", "[".repeat(levels), "]".repeat(levels))
}

#[test]
fn serde_json_reads_exactly_max_json_depth_levels() {
    assert!(serde_json::from_str::<Value>(&nested(MAX_JSON_DEPTH, "1")).is_ok());
    let error = serde_json::from_str::<Value>(&nested(MAX_JSON_DEPTH + 1, "1")).unwrap_err();
    assert!(
        error.to_string().starts_with("recursion limit exceeded"),
        "{error}"
    );
}

#[test]
fn json_as_deep_as_the_engine_reads_it_fits_its_stacks() {
    // A configuration, read into a serde_json::Value: an object with a field the engine does not know, as deep as allowed in all, is read; one level deeper, or a million, is refused at the level beyond the limit.
    let config = |depth: usize| format!(r#"{{"unknown":{}}}"#, nested(depth - 1, "1"));
    assert!(Config::from_json(config(MAX_JSON_DEPTH).as_bytes()).is_ok());
    assert!(Config::from_json(config(MAX_JSON_DEPTH + 1).as_bytes()).is_err());
    assert!(Config::from_json(config(FAR_DEEPER).as_bytes()).is_err());

    // A message whose payload has a field that the payload's type does not have, nested as deeply as allowed or a million levels deep: the engine skips the field without recursion and answers as for any other unknown field.
    assert!(config(FAR_DEEPER).len() < MAX_MESSAGE_BYTES);
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    for depth in [MAX_JSON_DEPTH, FAR_DEEPER] {
        let message = format!(
            r#"{{"v":1,"id":7,"type":"query.a11y","payload":{{"doc_id":{doc_id},"unknown":{}}}}}"#,
            nested(depth - 2, "1")
        );
        let answers: Vec<Value> = engine
            .handle(message.as_bytes(), 0)
            .iter()
            .map(|answer| serde_json::from_str(answer).unwrap())
            .collect();
        assert_eq!(ok_payload(&answers, 7)["root"]["role"], json!("document"));
    }

    // A recording with such a field: skipped the same way, and the recording refused for lacking what a recording holds.
    for depth in [MAX_JSON_DEPTH, FAR_DEEPER] {
        let recording = format!(r#"{{"unknown":{}}}"#, nested(depth - 1, "1"));
        assert!(replay(Config::default(), recording.as_bytes()).is_err());
    }

    // The engine still works.
    let tree = send(
        &mut engine,
        &request(99, "query.a11y", json!({ "doc_id": doc_id })),
    );
    assert_eq!(ok_payload(&tree, 99)["root"]["role"], json!("document"));
}
