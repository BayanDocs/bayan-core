//! The harness of the engine's four targets (work package CORE-010): protocol messages (`engine_messages`), the request of `bayan_render_tile` (`engine_tile`), the configuration (`engine_config`) and recordings that `diag.replay` reads (`engine_replay`), all untrusted input of the engine boundary (engine protocol §3, §10, §13; ADR-0006).
//!
//! Every engine here is created with its test switches off ([`config`]), so `diag.panic` is refused. libfuzzer-sys's panic hook aborts on every panic before anything can catch it, so a panic that the engine would catch and report (spec §12) still fails the run; bayan-ffi's silent hook is not linked into these targets. Each harness also checks the engine's answers itself, and fails on the error `panic` and on `engine.error` with `recoverable: false`, so that a caught panic fails the run even where no such hook runs (the tests of this crate show both).
//!
//! **Work.** The engine's limits allow requests that take long by design: a tile of 4,096 × 4,096 pixels takes about a third of a second natively and longer under AddressSanitizer, and one replay may render 2³⁰ pixels, about 20 seconds natively (spec §13). A short input could ask for many of them, and libFuzzer's timeout would then report work that the limits allow rather than work that grows faster than its input. So each harness renders at most [`PIXEL_BUDGET`] pixels per input, its own tiles included: it reads each message (and each entry of a recording) before the engine does, counts the pixels it asks for, and leaves out a message, or the whole input, that would exceed the budget. It reads them as the engine does, only more leniently, so that it never counts less than the engine renders: with typed structures that skip every field they do not need with serde's `IgnoredAny`, which serde_json skips at any depth, as the engine skips the fields it does not know (a `serde_json::Value` refuses nesting deeper than 128 levels); with invalid UTF-8 replaced, because the engine skips such fields without checking them; with both the size and the message of a recording's entry counted, whatever its kind; and with a recording, or an entry, written as a JSON array, which serde's derived readers also accept, by position, counted as over the budget ([`is_array`]). Whatever else it cannot read, the engine refuses too. A tile of up to 4,096 pixels on one side still fits, as long as the other side is short.

use std::sync::{Arc, OnceLock};

use bayan_engine::blobs::{BlobId, BlobStore};
use bayan_engine::config::Config;
use bayan_engine::limits::{MAX_MESSAGE_BYTES, MAX_TILE_SIDE};
use bayan_engine::protocol::{ArgValue, ErrorCode, ErrorInfo, ReplayReport};
use bayan_engine::{Engine, TileError, digest, recording, schema};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::schema_validator::Validator;

/// The most pixels one input may make the engine render, in tiles and replays together, the harness's own tiles included (the last tile of `engine_messages`, and the replay of a recorded call and the reference tile of `engine_tile`): 2²², four times the pixels of a full page at the resolution the seeds use, and under a second under AddressSanitizer.
pub const PIXEL_BUDGET: u64 = 1 << 22;

/// What the harness counts for JSON whose pixels it does not read: more than any budget, so that the input that holds it is left out.
const OVER_BUDGET: u64 = u64::MAX;

/// The pixels of the tile of 8 × 10 that `engine_messages` renders after the input, to show that the engine still works ([`last_tile`]); a test checks that this is what the engine renders for it.
const LAST_TILE_PIXELS: u64 = 8 * 10;

/// The request of the tile that `engine_messages` renders after the input, of the whole first page of document `doc_id`: [`LAST_TILE_PIXELS`] pixels.
fn last_tile(doc_id: u64) -> String {
    format!(
        r#"{{"v":0,"id":3,"type":"render.tile","payload":{{"doc_id":{doc_id},"page":0,"rect":{{"x":0,"y":0,"width":15544800,"height":20116800}},"width":8,"height":10}}}}"#
    )
}

/// The most messages of one input that `engine_messages` hands to the engine; the rest is ignored. Each message's work is small or counted against [`PIXEL_BUDGET`], but the largest answer, the accessibility tree of the whole mock document (about 180 KB), takes about 30 ms under AddressSanitizer to answer and check, so 64 of them stay well within the timeout of 10 seconds.
pub const MAX_MESSAGES: usize = 64;

/// The shell's blobs that `engine_messages` puts before each input, with odd identifiers as the shell's blobs get them: blob 1, empty, for `doc.open`, and blob 3, the recording that the engine's tests commit as `session.json`, for `diag.replay`.
pub const EMPTY_BLOB: BlobId = 1;
pub const RECORDING_BLOB: BlobId = 3;

/// The recording of [`RECORDING_BLOB`]: a session recorded by the engine's tests (`crates/bayan-engine/tests/engine/record_replay.rs`), committed and kept current by them.
pub const SESSION_RECORDING: &[u8] =
    include_bytes!("../../crates/bayan-engine/tests/fixtures/recordings/session.json");

/// The configuration of every engine target: the defaults, whose test switches are off, so that `diag.panic` is refused.
pub fn config() -> Config {
    let config = Config::default();
    assert!(
        !config.allow_panic,
        "the engine targets run with the test switches off"
    );
    config
}

/// The validator of the protocol's JSON Schema, which the engine's `schema` module generates; the validator is the engine tests' own (`schema_validator.rs`).
fn validator() -> &'static Validator<'static> {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    static VALIDATOR: OnceLock<Validator<'static>> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        Validator::new(SCHEMA.get_or_init(|| schema::json_schema().expect("the protocol's schema")))
    })
}

/// Checks one message the engine sent: at most the message limit, JSON, a valid protocol message by the schema, and no report of a panic; returns it parsed. Panics, which fails the run, otherwise.
pub fn check_answer(answer: &str) -> Value {
    assert!(
        answer.len() <= MAX_MESSAGE_BYTES,
        "an answer of {} bytes",
        answer.len()
    );
    let value: Value = serde_json::from_str(answer).expect("every answer is JSON");
    if let Err(error) = validator().validate(&value) {
        panic!("an answer that is not a valid protocol message: {error}");
    }
    let code = value
        .pointer("/error/code")
        .or_else(|| value.pointer("/payload/code"));
    assert_ne!(code, Some(&json!("panic")), "the engine caught a panic");
    assert_ne!(
        value.pointer("/payload/recoverable"),
        Some(&json!(false)),
        "the engine discarded its session"
    );
    value
}

/// A message, read as far as its pixels go: its type, and its payload as the JSON text it was written as, as the engine's envelope reads them (`crates/bayan-engine/src/envelope.rs`). serde skips every other field with `IgnoredAny`, which serde_json skips at any depth without storing it, as the engine skips the fields it does not know; a `serde_json::Value` refuses nesting deeper than 128 levels, and would then count nothing for a message that the engine renders.
#[derive(Deserialize)]
struct MessageHead<'a> {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(borrow)]
    payload: Option<&'a RawValue>,
}

/// The size of a `render.tile`'s tile, read from its payload; every other field is skipped as [`MessageHead`] skips it.
#[derive(Deserialize)]
struct TileSize {
    width: Option<u64>,
    height: Option<u64>,
}

/// A recording, read as far as its pixels go: its entries, each as the JSON text it was written as; every other field is skipped as [`MessageHead`] skips it.
#[derive(Deserialize)]
struct RecordingHead<'a> {
    #[serde(borrow)]
    entries: Option<Vec<&'a RawValue>>,
}

/// An entry of a recording, read as far as its pixels go: its message or tile request, and its tile's size. Each field has the type of the engine's `Entry` (`crates/bayan-engine/src/recording.rs`) or a wider one, and every other field, the entry's kind among them, is skipped as [`MessageHead`] skips it.
#[derive(Deserialize)]
struct EntryHead {
    text: Option<String>,
    base64: Option<String>,
    width: Option<u64>,
    height: Option<u64>,
}

/// What a message asks the engine to render.
enum Asks {
    /// A tile of so many pixels: none for a size the engine refuses.
    Tile(u64),
    /// A replay of one of the harness's blobs.
    Replay,
    /// Nothing.
    Nothing,
}

/// Reads what a message asks the engine to render, as [`MessageHead`] and [`TileSize`] read it, with invalid UTF-8 replaced, because the engine skips the fields it does not know without checking them. A message it cannot read, the engine refuses too, and a `render.tile` whose payload it cannot read, the engine refuses as `invalid_request`.
fn asks(message: &[u8]) -> Asks {
    let text = String::from_utf8_lossy(message);
    let Ok(head) = serde_json::from_str::<MessageHead<'_>>(&text) else {
        return Asks::Nothing;
    };
    match head.kind.as_deref() {
        Some("render.tile") => Asks::Tile(
            head.payload
                .and_then(|payload| serde_json::from_str::<TileSize>(payload.get()).ok())
                .map_or(0, |size| tile_pixels(size.width, size.height)),
        ),
        Some("diag.replay") => Asks::Replay,
        _ => Asks::Nothing,
    }
}

/// Whether JSON text is an array. serde's derived `Deserialize` reads a struct from an array too, by position, and so does the engine read a recording and its entries; the harness does not mirror that, and counts such JSON as [`OVER_BUDGET`].
fn is_array(json: &str) -> bool {
    json.trim_start().starts_with('[')
}

/// The pixels of a tile of `width` × `height` that the engine would render: none for a size it refuses.
fn tile_pixels(width: Option<u64>, height: Option<u64>) -> u64 {
    match (width, height) {
        (Some(width), Some(height))
            if (1..=u64::from(MAX_TILE_SIDE)).contains(&width)
                && (1..=u64::from(MAX_TILE_SIDE)).contains(&height) =>
        {
            width * height
        }
        _ => 0,
    }
}

/// The pixels a message may make the engine render: a `render.tile`'s tile, and for `diag.replay` the most that any blob of the harness's can make it render (only [`RECORDING_BLOB`] is a recording, because the harness releases every blob the engine creates).
fn message_pixels(message: &[u8]) -> u64 {
    match asks(message) {
        Asks::Tile(pixels) => pixels,
        Asks::Replay => recording_pixels(SESSION_RECORDING),
        Asks::Nothing => 0,
    }
}

/// Decodes base64 more leniently than the engine (`crates/bayan-engine/src/base64.rs`, which also refuses missing padding and non-zero padding bits), so that it never decodes less than the engine.
fn lenient_base64(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0_u32;
    let mut count = 0;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for symbol in text.bytes().take_while(|&symbol| symbol != b'=') {
        let value = match symbol {
            b'A'..=b'Z' => symbol - b'A',
            b'a'..=b'z' => symbol - b'a' + 26,
            b'0'..=b'9' => symbol - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        bits = bits << 6 | u32::from(value);
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push(u8::try_from(bits >> count & 0xFF).unwrap_or(0));
        }
    }
    Some(out)
}

/// The most pixels that replaying `recording` may render: the tiles of its tile entries and of the `render.tile` messages among its message entries. It reads the recording as the engine does (`RecordingFile` in `crates/bayan-engine/src/recording.rs`), as leniently as [`asks`] reads a message: a recording that it cannot read, or with an entry that it cannot read, the engine refuses before it renders anything, and one written as an array, or with an entry written as an array, counts as [`OVER_BUDGET`] ([`is_array`]).
pub fn recording_pixels(recording: &[u8]) -> u64 {
    let text = String::from_utf8_lossy(recording);
    if is_array(&text) {
        return OVER_BUDGET;
    }
    let Ok(RecordingHead {
        entries: Some(entries),
    }) = serde_json::from_str(&text)
    else {
        return 0;
    };
    let mut pixels = 0_u64;
    for entry in entries {
        if is_array(entry.get()) {
            return OVER_BUDGET;
        }
        let Ok(entry) = serde_json::from_str::<EntryHead>(entry.get()) else {
            return 0;
        };
        // A tile entry renders its size, a message entry its message (a replaying engine refuses diag.replay itself); counting both, whatever the entry's kind, never counts less.
        let message = [
            entry.text.map(String::into_bytes),
            entry.base64.as_deref().and_then(lenient_base64),
        ]
        .iter()
        .flatten()
        .map(|bytes| match asks(bytes) {
            Asks::Tile(pixels) => pixels,
            Asks::Replay | Asks::Nothing => 0,
        })
        .max()
        .unwrap_or(0);
        pixels = pixels.saturating_add(tile_pixels(entry.width, entry.height).max(message));
    }
    pixels
}

/// An engine with the given configuration and the harness's blobs, after a successful handshake.
fn engine_after_hello(config: Config) -> Engine {
    let blobs = Arc::new(BlobStore::new());
    assert_eq!(blobs.put_shell(b""), Ok(EMPTY_BLOB));
    assert_eq!(blobs.put_shell(SESSION_RECORDING), Ok(RECORDING_BLOB));
    let mut engine = Engine::new(config, blobs);
    let welcome = send(
        &mut engine,
        br#"{"v":0,"id":1,"type":"hello","payload":{"protocol_versions":[0]}}"#,
    );
    assert_eq!(welcome[0]["type"], json!("welcome"));
    engine
}

/// Hands one message that arrived at `received_ms` to the engine, checks every answer, releases the blobs the engine created for it, as a shell would, and returns the answers.
fn send_at(engine: &mut Engine, message: &[u8], received_ms: u64) -> Vec<Value> {
    let answers: Vec<Value> = engine
        .handle(message, received_ms)
        .iter()
        .map(|answer| check_answer(answer))
        .collect();
    for answer in &answers {
        // Engine blobs have even identifiers; only those are released.
        if let Some(blob) = answer.pointer("/payload/blob").and_then(Value::as_u64)
            && blob % 2 == 0
        {
            engine.blobs().release(blob);
        }
    }
    answers
}

fn send(engine: &mut Engine, message: &[u8]) -> Vec<Value> {
    send_at(engine, message, 0)
}

/// `engine_messages`: the input split into messages at each line feed (which JSON writes only as whitespace between tokens), handed one by one to an engine with the test switches off after a valid handshake (see [`messages_with`]).
pub fn messages(data: &[u8]) {
    messages_with(config(), data);
}

/// `engine_messages` with another configuration, for the tests of this harness. Properties: no panic, also none that the engine catches; every answer is a valid protocol message (the engine's `schema` module, as `hostile_input.rs` checks); and valid requests after the hostile ones still succeed: a new handshake, opening the mock document and rendering a tile of it.
pub fn messages_with(config: Config, data: &[u8]) {
    let mut engine = engine_after_hello(config);
    let mut pixels = LAST_TILE_PIXELS;
    for (index, message) in data
        .split(|&byte| byte == b'\n')
        .take(MAX_MESSAGES)
        .enumerate()
    {
        let cost = message_pixels(message);
        if pixels.saturating_add(cost) > PIXEL_BUDGET {
            continue;
        }
        pixels += cost;
        send_at(&mut engine, message, u64::try_from(index).unwrap_or(0) * 10);
    }
    // The engine still works.
    let id = bayan_engine::limits::MAX_ID;
    let welcome = send(
        &mut engine,
        format!(r#"{{"v":0,"id":{id},"type":"hello","payload":{{"protocol_versions":[0]}}}}"#)
            .as_bytes(),
    );
    assert_eq!(
        (welcome.len(), &welcome[0]["re"], &welcome[0]["type"]),
        (1, &json!(id), &json!("welcome")),
        "a new handshake after the input"
    );
    let opened = send(
        &mut engine,
        format!(r#"{{"v":0,"id":2,"type":"doc.open","payload":{{"blob":{EMPTY_BLOB}}}}}"#)
            .as_bytes(),
    );
    let doc_id = opened[0]
        .pointer("/payload/doc_id")
        .and_then(Value::as_u64)
        .expect("the mock document opens after the input");
    let tile = send(&mut engine, last_tile(doc_id).as_bytes());
    assert_eq!(
        tile[0].pointer("/payload/stride"),
        Some(&json!(32)),
        "a tile after the input"
    );
}

/// The request of the reference tile that `engine_tile` renders after each input, and its size.
const REFERENCE_TILE: &[u8] =
    br#"{"doc_id":1,"page":0,"rect":{"x":0,"y":0,"width":15544800,"height":20116800}}"#;
const REFERENCE_SIZE: (u32, u32) = (32, 41);

/// The pixels that `engine_tile` renders for a tile of `width` × `height`: the tile (none for a size the engine refuses), again in the replay when the call is recorded, and the reference tile afterwards.
fn tile_cost(width: u32, height: u32, record: bool) -> u64 {
    tile_pixels(Some(u64::from(width)), Some(u64::from(height))) * (1 + u64::from(record))
        + u64::from(REFERENCE_SIZE.0) * u64::from(REFERENCE_SIZE.1)
}

/// An engine after the handshake with the mock document open as document 1, as `engine_tile` starts.
fn engine_with_document(config: Config) -> Engine {
    let mut engine = engine_after_hello(config);
    let opened = send(
        &mut engine,
        format!(r#"{{"v":0,"id":2,"type":"doc.open","payload":{{"blob":{EMPTY_BLOB}}}}}"#)
            .as_bytes(),
    );
    assert_eq!(opened[0].pointer("/payload/doc_id"), Some(&json!(1)));
    engine
}

/// The digest of the reference tile, rendered once by a fresh engine.
fn reference_digest() -> &'static str {
    static DIGEST: OnceLock<String> = OnceLock::new();
    DIGEST.get_or_init(|| {
        let mut engine = engine_with_document(config());
        let pixels = engine
            .render_tile(REFERENCE_TILE, REFERENCE_SIZE.0, REFERENCE_SIZE.1, 0)
            .pixels
            .expect("the reference tile");
        digest::of_bytes(&pixels)
    })
}

/// `engine_tile`: [`Engine::render_tile`], the request behind `bayan_render_tile`, on an engine with the mock document open. The input is a flags byte, the tile's width and height in pixels as two little-endian 16-bit numbers, and the request; bit 0 of the flags records the call. Properties: no panic; a rendered tile has exactly its size and no messages; a refused request is refused as `invalid_argument` or `not_found`, never `internal`, and with no messages; a recorded call replays identically; and the engine is still usable afterwards: the reference tile renders exactly as on a fresh engine. A request whose tiles would exceed [`PIXEL_BUDGET`], the replay of a recorded call and the reference tile counted ([`tile_cost`]), is not made, so that the fuzzer does not spend its time filling memory; the size limits themselves (1 to 4,096 pixels a side) are cheap to reach.
pub fn tile(data: &[u8]) {
    let Some((&[flags, w0, w1, h0, h1], request)) = data.split_first_chunk::<5>() else {
        return;
    };
    let (width, height) = (
        u32::from(u16::from_le_bytes([w0, w1])),
        u32::from(u16::from_le_bytes([h0, h1])),
    );
    let record = flags & 1 == 1;
    if tile_cost(width, height, record) > PIXEL_BUDGET {
        return;
    }
    let mut engine = engine_with_document(config());
    if record {
        send(&mut engine, br#"{"v":0,"id":3,"type":"diag.record.start"}"#);
    }
    let outcome = engine.render_tile(request, width, height, 7);
    assert!(
        outcome.messages.is_empty(),
        "rendering a tile sent {} messages",
        outcome.messages.len()
    );
    match &outcome.pixels {
        Ok(pixels) => {
            assert_eq!(
                pixels.len(),
                usize::try_from(u64::from(width) * u64::from(height) * 4).unwrap_or(usize::MAX)
            );
            assert_eq!(outcome.digest, record.then(|| digest::of_bytes(pixels)));
        }
        Err(TileError::InvalidArgument | TileError::NotFound) => assert_eq!(outcome.digest, None),
        Err(error) => panic!("a tile request refused as {}", error.as_str()),
    }
    if record {
        // Not through `send`, which would release the recording's blob.
        let answers = engine.handle(br#"{"v":0,"id":4,"type":"diag.record.stop"}"#, 9);
        let stopped = check_answer(&answers[0]);
        let blob = stopped
            .pointer("/payload/blob")
            .and_then(Value::as_u64)
            .expect("the recording's blob");
        let recording = engine.blobs().take(blob).expect("the recording");
        let report = recording::replay(config(), &recording).expect("the recording replays");
        assert!(
            report.identical && report.entries == 1,
            "the recorded call replays identically: {report:?}"
        );
    }
    let reference = engine
        .render_tile(REFERENCE_TILE, REFERENCE_SIZE.0, REFERENCE_SIZE.1, 0)
        .pixels
        .expect("the reference tile after the input");
    assert_eq!(
        digest::of_bytes(&reference),
        reference_digest(),
        "the reference tile changed after the input"
    );
}

/// `engine_config`: [`Config::from_json`]. Properties: no panic; the same input gives the same configuration; only an empty input or a JSON object is accepted; and an accepted configuration starts an engine that completes a handshake, and, with the test switch off, refuses `diag.panic`.
pub fn config_json(data: &[u8]) {
    let parsed = Config::from_json(data);
    assert_eq!(
        Config::from_json(data),
        parsed,
        "the same configuration twice"
    );
    let Ok(config) = parsed else {
        assert!(
            !data.is_empty(),
            "an empty configuration means the defaults"
        );
        return;
    };
    assert!(
        data.is_empty()
            || serde_json::from_slice::<Value>(data).is_ok_and(|value| value.is_object()),
        "only an object is a configuration"
    );
    let mut engine = engine_after_hello(config);
    if !config.allow_panic {
        let refused = send(&mut engine, br#"{"v":0,"id":2,"type":"diag.panic"}"#);
        assert_eq!(
            refused[0].pointer("/error/code"),
            Some(&json!("not_allowed"))
        );
    }
}

/// `engine_replay`: [`recording::replay`] with a fixed configuration, the test switches off (see [`replay_with`]).
pub fn replay(data: &[u8]) {
    replay_with(config(), data);
}

/// `engine_replay` with another configuration, for the tests of this harness. Properties: no panic, also none that the replaying engine catches (a replay that met one would also differ from its recording, which the harness cannot tell from a damaged digest, so only the hook catches it); the recording is refused with a documented error, which the schema accepts, or replayed into a consistent report, which the schema accepts; and replaying it again gives the same result. A recording whose tiles would take more than [`PIXEL_BUDGET`] pixels, both replays together, is not replayed.
pub fn replay_with(config: Config, data: &[u8]) {
    if recording_pixels(data).saturating_mul(2) > PIXEL_BUDGET {
        return;
    }
    let result = recording::replay(config, data);
    assert_eq!(
        recording::replay(config, data),
        result,
        "replaying the same recording twice"
    );
    match &result {
        Ok(report) => check_report(report),
        Err(error) => check_refusal(error),
    }
}

fn check_report(report: &ReplayReport) {
    let value = serde_json::to_value(report).expect("a report is JSON");
    if let Err(error) = validator().validate_definition("ReplayReport", &value) {
        panic!("a replay report that the schema refuses: {error}");
    }
    assert_eq!(report.identical, report.first_difference.is_none());
    assert!(
        report
            .first_difference
            .is_none_or(|entry| entry < report.entries)
    );
    let mut previous = 0;
    for tile in &report.tiles {
        assert!(
            tile.entry < report.entries && tile.entry >= previous,
            "{tile:?}"
        );
        previous = tile.entry;
        assert!(
            (1..=MAX_TILE_SIDE).contains(&tile.width) && (1..=MAX_TILE_SIDE).contains(&tile.height),
            "{tile:?}"
        );
        assert!(
            tile.hash.strip_prefix("fnv1a64:").is_some_and(
                |hex| hex.len() == 16 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
            ),
            "{tile:?}"
        );
    }
}

/// A replay is refused only with the errors that `recording::replay` documents: `invalid_recording`, `recording_mismatch` with the field that differs, and `limit_exceeded` for the pixels of a replay (spec §10).
fn check_refusal(error: &ErrorInfo) {
    let value = serde_json::to_value(error).expect("an error is JSON");
    if let Err(problem) = validator().validate_definition("ErrorInfo", &value) {
        panic!("a refusal that the schema refuses: {problem}");
    }
    let text = |name: &str| match error.args.get(name) {
        Some(ArgValue::Text(text)) => Some(text.as_str()),
        _ => None,
    };
    match error.code {
        ErrorCode::InvalidRecording => assert!(error.args.is_empty(), "{error:?}"),
        ErrorCode::RecordingMismatch => assert!(
            matches!(
                text("field"),
                Some("engine_version" | "protocol_version" | "layout_epoch")
            ) && error.args.len() == 1,
            "{error:?}"
        ),
        ErrorCode::LimitExceeded => assert!(
            text("limit") == Some("replay_pixels") && error.args.len() == 1,
            "{error:?}"
        ),
        _ => panic!("a replay refused with an undocumented error: {error:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_pixels_the_engine_would_render() {
        assert_eq!(message_pixels(br#"{"v":0,"id":4,"type":"render.tile","payload":{"doc_id":1,"page":0,"rect":{},"width":8,"height":3}}"#), 24);
        // Invalid UTF-8 in a field the engine skips hides nothing.
        assert_eq!(message_pixels(b"{\"v\":0,\"x\":\"\xff\",\"type\":\"render.tile\",\"payload\":{\"width\":4096,\"height\":4096}}"), 1 << 24);
        // Escapes in the type are read as the engine reads them.
        assert_eq!(
            message_pixels(br#"{"v":0,"type":"render.tile","payload":{"width":2,"height":2}}"#),
            4
        );
        // Sizes the engine refuses cost nothing.
        assert_eq!(
            message_pixels(br#"{"v":0,"type":"render.tile","payload":{"width":4097,"height":2}}"#),
            0
        );
        assert_eq!(
            message_pixels(br#"{"v":0,"type":"render.tile","payload":{"width":0,"height":2}}"#),
            0
        );
        assert_eq!(
            message_pixels(br#"{"v":0,"type":"diag.replay","payload":{"blob":3}}"#),
            recording_pixels(SESSION_RECORDING)
        );
        assert!(recording_pixels(SESSION_RECORDING) > 0);
        assert_eq!(message_pixels(b"not json"), 0);
    }

    #[test]
    fn sets_aside_what_the_engine_renders_for_the_last_tile() {
        // The budget of `engine_messages` keeps LAST_TILE_PIXELS for the tile after the input (CORE-010's review, round 2): the harness counts exactly that for the request it sends, and the engine renders exactly that many pixels for it, so that a change to either the request or the constant fails here.
        assert_eq!(message_pixels(last_tile(1).as_bytes()), LAST_TILE_PIXELS);
        let mut engine = engine_after_hello(config());
        let opened = send(
            &mut engine,
            format!(r#"{{"v":0,"id":2,"type":"doc.open","payload":{{"blob":{EMPTY_BLOB}}}}}"#)
                .as_bytes(),
        );
        let doc_id = opened[0]
            .pointer("/payload/doc_id")
            .and_then(Value::as_u64)
            .expect("the mock document opens");
        let tile = send(&mut engine, last_tile(doc_id).as_bytes());
        let size = |field: &str| {
            tile[0]
                .pointer(&format!("/payload/{field}"))
                .and_then(Value::as_u64)
                .expect("a rendered tile")
        };
        assert_eq!(size("width") * size("height"), LAST_TILE_PIXELS);
        assert_eq!(size("stride"), 4 * size("width"));
    }

    /// JSON nested 200 levels deep, deeper than a `serde_json::Value` reads.
    fn nested() -> String {
        let nested = format!("{}{}", "[".repeat(200), "]".repeat(200));
        assert!(serde_json::from_str::<Value>(&nested).is_err());
        nested
    }

    #[test]
    fn counts_a_message_with_fields_nested_deeper_than_a_value_reads() {
        let nested = nested();
        let message = format!(
            r#"{{"v":0,"id":4,"skipped":{nested},"type":"render.tile","payload":{{"skipped":{nested},"doc_id":1,"page":0,"rect":{{"skipped":{nested},"x":0,"y":0,"width":15544800,"height":20116800}},"width":8,"height":3}}}}"#
        );
        assert_eq!(message_pixels(message.as_bytes()), 24);
        // The engine skips those fields and renders the tile: 8 pixels of 4 bytes a row.
        let mut engine = engine_with_document(config());
        let rendered = send(&mut engine, message.as_bytes());
        assert_eq!(
            rendered[0].pointer("/payload/stride"),
            Some(&json!(32)),
            "{rendered:?}"
        );
    }

    #[test]
    fn counts_a_recording_with_fields_nested_deeper_than_a_value_reads() {
        let nested = nested();
        let placeholder = "nested-200-levels";
        let mut recording: Value = serde_json::from_slice(SESSION_RECORDING).unwrap();
        recording["skipped"] = json!(placeholder);
        for entry in recording["entries"].as_array_mut().unwrap() {
            entry["skipped"] = json!(placeholder);
            // Every message and tile request is an object; the field goes in first.
            let text = entry["text"].as_str().unwrap();
            assert!(text.starts_with('{'), "{text}");
            entry["text"] = json!(text.replacen('{', &format!(r#"{{"skipped":{nested},"#), 1));
        }
        let recording = serde_json::to_string(&recording)
            .unwrap()
            .replace(&format!("\"{placeholder}\""), &nested);
        assert_eq!(
            recording_pixels(recording.as_bytes()),
            recording_pixels(SESSION_RECORDING)
        );
        // The engine skips those fields and replays every entry as it was recorded, tiles included.
        let report = recording::replay(config(), recording.as_bytes()).expect("a replay");
        assert!(report.identical && !report.tiles.is_empty(), "{report:?}");
        assert_eq!(Ok(report), recording::replay(config(), SESSION_RECORDING));
    }

    #[test]
    fn counts_a_recording_or_an_entry_written_as_an_array_as_over_the_budget() {
        assert_eq!(recording_pixels(b" [1, 2]"), OVER_BUDGET);
        assert_eq!(
            recording_pixels(br#"{"entries":[[0,"tile",null,null,8,3,"fnv1a64:0"]]}"#),
            OVER_BUDGET
        );
        // An entry's size and its message both count, whatever its kind, which serde also reads from a map.
        assert_eq!(
            recording_pixels(br#"{"entries":[{"kind":{"tile":null},"width":8,"height":3}]}"#),
            24
        );
        assert_eq!(
            recording_pixels(
                br#"{"entries":[{"kind":"tile","width":2,"height":2,"text":"{\"type\":\"render.tile\",\"payload\":{\"width\":8,\"height\":3}}"}]}"#
            ),
            24
        );
    }

    #[test]
    fn engine_tile_counts_the_replay_of_a_recorded_call_and_the_reference_tile() {
        let reference = 32 * 41;
        assert_eq!(tile_cost(2048, 1024, false), (1 << 21) + reference);
        assert_eq!(tile_cost(2048, 1024, true), (1 << 22) + reference);
        assert!(tile_cost(2048, 1024, true) > PIXEL_BUDGET);
        assert_eq!(tile_cost(4097, 1, true), reference);
        assert_eq!(tile_cost(0, 1, false), reference);
    }

    #[test]
    fn decodes_base64_at_least_as_far_as_the_engine() {
        assert_eq!(lenient_base64("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(lenient_base64("aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(lenient_base64("aGVsbG9="), Some(b"hello".to_vec()));
        assert_eq!(lenient_base64("a GVs"), None);
    }

    #[test]
    fn the_seeds_pass() {
        messages(b"{\"v\":0,\"id\":2,\"type\":\"doc.open\",\"payload\":{\"blob\":1}}\n{\"v\":0,\"id\":3,\"type\":\"diag.replay\",\"payload\":{\"blob\":3}}\n{\"v\":0,\"id\":4,\"type\":\"diag.panic\"}");
        let mut input = vec![1, 64, 0, 48, 0];
        input.extend_from_slice(REFERENCE_TILE);
        tile(&input);
        config_json(b"");
        config_json(br#"{"test":{"allow_panic":true}}"#);
        replay(SESSION_RECORDING);
    }
}
