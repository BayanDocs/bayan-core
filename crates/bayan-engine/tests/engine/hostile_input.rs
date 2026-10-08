//! Hostile input (ADR-0006, engine protocol §13): random bytes and damaged versions of valid messages and recordings never make a handler panic, every answer is a valid protocol message, and the engine stays usable.
//!
//! This is a deterministic, bounded stand-in for fuzzing until the fuzzing workspace exists (CORE-005, CORE-006): it runs the same damaged inputs on every run and every platform.

use std::cell::Cell;
use std::sync::Once;

use bayan_engine::config::Config;
use bayan_engine::recording::replay;
use bayan_engine::schema;
use serde_json::{Value, json};

use crate::support::schema_validator::Validator;
use crate::support::{engine, ok_payload, open_document, request, send, view, whole_page};

thread_local! {
    /// How many panics happened on this thread, including those the engine caught.
    static PANICS: Cell<u64> = const { Cell::new(0) };
}

/// The number of panics on the current thread so far. The first call installs a panic hook that counts each panic on the thread where it happens, then hands it on to the previous hook. The engine catches the panics of its handlers, also while it replays a recording, so only such a count shows them all; other tests' panics happen on other threads and do not count here.
fn panics_so_far() -> u64 {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ignored = PANICS.try_with(|count| count.set(count.get() + 1));
            previous(info);
        }));
    });
    PANICS.with(Cell::get)
}

/// Numbers that limits and conversions must survive: negative, fractional, beyond `u32`, `i64`, `u64` and 2⁵³, too large for a double, subnormal, zero, and values that are not numbers at all.
const EXTREMES: [&str; 14] = [
    "-1",
    "0",
    "-0",
    "0.5",
    "4097",
    "4294967296",
    "9007199254740992",
    "-9223372036854775809",
    "18446744073709551616",
    "1e309",
    "1e-320",
    "null",
    "\"\"",
    "[]",
];

/// Where the numbers of a JSON text are, outside its strings: the byte ranges of every number token.
fn number_spans(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    let mut in_string = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if in_string {
            match byte {
                b'\\' => index += 1,
                b'"' => in_string = false,
                _ => {}
            }
            index += 1;
        } else if byte == b'"' {
            in_string = true;
            index += 1;
        } else if byte == b'-' || byte.is_ascii_digit() {
            let start = index;
            while index < bytes.len() && b"-+.eE0123456789".contains(&bytes[index]) {
                index += 1;
            }
            spans.push((start, index));
        } else {
            index += 1;
        }
    }
    spans
}

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
            // Replace a randomly chosen number, anywhere in the message, with an extreme one.
            let text = String::from_utf8_lossy(&out).into_owned();
            let spans = number_spans(&text);
            if !spans.is_empty() {
                let (start, end) = spans[random.below(spans.len())];
                let extreme = EXTREMES[random.below(EXTREMES.len())];
                out = format!("{}{extreme}{}", &text[..start], &text[end..]).into_bytes();
            }
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
fn numbers_are_found_outside_strings_only() {
    let text = r#"{"a":-12.5e3,"b":"x1","c":[0,7],"d":"\"9"}"#;
    let numbers: Vec<&str> = number_spans(text)
        .iter()
        .map(|&(start, end)| &text[start..end])
        .collect();
    assert_eq!(numbers, ["-12.5e3", "0", "7"]);
}

// Host only: in WebAssembly (wasm32-wasip1) a panic aborts the whole test program instead of unwinding, so the engine cannot catch it.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn the_panic_count_sees_panics_the_engine_catches() {
    let mut engine = crate::support::engine_with(Config { allow_panic: true });
    open_document(&mut engine);
    let before = panics_so_far();
    send(&mut engine, &request(5, "diag.panic", json!({})));
    assert_eq!(panics_so_far(), before + 1);
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
        let panics = panics_so_far();
        let answers = engine.handle(&message, 0);
        assert_eq!(
            panics_so_far(),
            panics,
            "handling a damaged message panicked: {}",
            String::from_utf8_lossy(&message)
        );
        for answer in answers {
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
        // A damaged recording either fails to load, or replays and reports what differs; it never panics, not even in a handler, where the replaying engine would catch the panic and only a digest would show it.
        let panics = panics_so_far();
        let outcome = replay(Config::default(), &damaged);
        assert_eq!(
            panics_so_far(),
            panics,
            "replaying a damaged recording panicked: {}",
            String::from_utf8_lossy(&damaged)
        );
        if outcome.is_err() {
            refused += 1;
        }
    }
    assert!(
        refused > 100,
        "only {refused} damaged recordings were refused"
    );
}
