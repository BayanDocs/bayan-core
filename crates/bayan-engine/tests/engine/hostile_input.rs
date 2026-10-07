//! Hostile input (ADR-0006, engine protocol §13): random bytes and damaged versions of valid messages and recordings never make a handler panic, every answer is a valid protocol message, and the engine stays usable.
//!
//! This is a deterministic, bounded stand-in for fuzzing until the fuzzing workspace exists (CORE-005, CORE-006): it runs the same damaged inputs on every run and every platform.

use bayan_engine::config::Config;
use bayan_engine::recording::replay;
use bayan_engine::schema;
use serde_json::{Value, json};

use crate::support::schema_validator::Validator;
use crate::support::{engine, ok_payload, open_document, request, send, view, whole_page};

/// A small deterministic pseudo-random generator (xorshift64*), so every run tests the same inputs.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap()).unwrap()
    }
}

/// Damages bytes in one of several ways.
fn damage(bytes: &[u8], random: &mut Random) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let position = random.below(out.len() + 1);
    match random.below(6) {
        0 if !out.is_empty() => {
            let index = position.min(out.len() - 1);
            out[index] = u8::try_from(random.next() & 0xff).unwrap();
        }
        1 => out.insert(position, b"{}[]\":,0-9e\\\x00\xff"[random.below(14)]),
        2 if !out.is_empty() => {
            out.remove(position.min(out.len() - 1));
        }
        3 => out.truncate(position),
        4 => {
            let end = (position + random.below(16)).min(out.len());
            let slice = out[position..end].to_vec();
            out.splice(position..position, slice);
        }
        _ => {
            // Replace a number with an extreme one.
            let text = String::from_utf8_lossy(&out).into_owned();
            let extreme = [
                "-1",
                "1e309",
                "18446744073709551616",
                "-9223372036854775809",
                "0.5",
                "null",
                "\"\"",
                "[]",
            ][random.below(8)];
            out = text.replacen(char::is_numeric, extreme, 1).into_bytes();
        }
    }
    out
}

/// Valid messages of every kind, to damage.
fn seeds(doc_id: u64, blob: u64) -> Vec<Vec<u8>> {
    [
        request(1, "hello", json!({ "protocol_versions": [0], "shell": { "name": "x", "version": "1", "platform": "p" } })),
        request(2, "doc.open", json!({ "blob": blob })),
        request(3, "view.set", view(doc_id, 1.25)),
        request(4, "render.tile", whole_page(doc_id, 0, 8, 8)),
        request(5, "input.text", json!({ "doc_id": doc_id, "text": "Grüße 😀" })),
        request(6, "input.composition", json!({ "doc_id": doc_id, "phase": "update", "text": "かな", "selection": { "start": 0, "end": 1 } })),
        request(7, "input.key", json!({ "doc_id": doc_id, "key": "ArrowLeft", "modifiers": { "shift": true } })),
        request(8, "input.pointer", json!({ "doc_id": doc_id, "kind": "down", "x": 150.25, "y": 222, "button": 0, "clicks": 2 })),
        request(9, "query.a11y", json!({ "doc_id": doc_id, "node": 5 })),
        request(10, "ui.manifest", json!({ "locale": "fr" })),
        request(11, "doc.close", json!({ "doc_id": doc_id + 5 })),
        request(12, "diag.replay", json!({ "blob": blob })),
    ]
    .iter()
    .map(|message| serde_json::to_vec(message).unwrap())
    .collect()
}

#[test]
fn damaged_messages_are_answered_with_valid_messages_and_never_panic_a_handler() {
    let schema = schema::json_schema().unwrap();
    let validator = Validator::new(&schema);
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(2, "view.set", view(doc_id, 1.0)));
    let blob = engine.blobs().put_shell(b"bytes").unwrap();
    let seeds = seeds(doc_id, blob);
    let mut random = Random(0x0123_4567_89ab_cdef);
    let mut answered = 0;
    for round in 0..3_000 {
        let mut message = seeds[random.below(seeds.len())].clone();
        for _ in 0..=random.below(3) {
            message = damage(&message, &mut random);
        }
        if round % 10 == 0 {
            // Now and then, pure noise.
            message = (0..random.below(64))
                .map(|_| u8::try_from(random.next() & 0xff).unwrap())
                .collect();
        }
        for answer in engine.handle(&message, 0) {
            let answer: Value = serde_json::from_str(&answer).unwrap();
            validator
                .validate(&answer)
                .unwrap_or_else(|error| panic!("{error}\n{answer}"));
            let code = answer
                .pointer("/error/code")
                .or_else(|| answer.pointer("/payload/code"));
            assert_ne!(
                code,
                Some(&json!("panic")),
                "a handler panicked on {}",
                String::from_utf8_lossy(&message)
            );
            // Blobs that tiles and recordings create are released, as a shell would.
            if let Some(id) = answer.pointer("/payload/blob").and_then(Value::as_u64) {
                engine.blobs().release(id);
            }
            answered += 1;
        }
    }
    assert!(answered > 1_000, "only {answered} answers");
    // The engine still works.
    let doc_id = open_document(&mut engine);
    let tree = send(
        &mut engine,
        &request(99, "query.a11y", json!({ "doc_id": doc_id })),
    );
    assert_eq!(ok_payload(&tree, 99)["root"]["role"], json!("document"));
}

#[test]
fn damaged_recordings_are_refused_or_replayed_but_never_panic() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    for message in seeds(doc_id, 1).iter().skip(2).take(6) {
        engine.handle(message, 7);
    }
    let stopped = send(&mut engine, &request(4, "diag.record.stop", json!({})));
    let recording = engine
        .blobs()
        .take(ok_payload(&stopped, 4)["blob"].as_u64().unwrap())
        .unwrap()
        .to_vec();
    assert!(replay(Config::default(), &recording).unwrap().identical);
    let mut random = Random(0xfeed_beef_dead_cafe);
    let mut refused = 0;
    for _ in 0..300 {
        let mut damaged = recording.clone();
        for _ in 0..=random.below(4) {
            damaged = damage(&damaged, &mut random);
        }
        // A damaged recording either fails to load, or replays and reports what differs; it never panics.
        if replay(Config::default(), &damaged).is_err() {
            refused += 1;
        }
    }
    assert!(
        refused > 100,
        "only {refused} damaged recordings were refused"
    );
}
