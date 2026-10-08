//! Record and replay reproduce identical tile hashes (CORE-007 AC-2, engine protocol §10).

use crate::support::{
    engine, engine_with, error_code, ok_payload, open_document, request, send, view, whole_page,
};
use bayan_engine::config::Config;
use bayan_engine::digest;
use bayan_engine::recording::replay;
use serde_json::{Value, json};

/// A session that types, composes, clicks and renders tiles through both paths, recorded from `start` on. Returns the recording and the hashes of the tiles rendered while recording, in order.
fn recorded_session() -> (Vec<u8>, Vec<String>) {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(3, "view.set", view(doc_id, 1.0)));
    // Typed before the recording starts, so the recording must start from this state.
    send(
        &mut engine,
        &request(
            4,
            "input.text",
            json!({ "doc_id": doc_id, "text": "Before " }),
        ),
    );
    assert_eq!(
        ok_payload(
            &send(&mut engine, &request(5, "diag.record.start", json!({}))),
            5
        ),
        &json!({})
    );

    let mut hashes = Vec::new();
    let tile = |engine: &mut bayan_engine::Engine, id: u64, hashes: &mut Vec<String>| {
        let answers = send(
            engine,
            &request(id, "render.tile", whole_page(doc_id, 0, 102, 132)),
        );
        let reply = ok_payload(&answers, id);
        let blob = reply["blob"].as_u64().unwrap();
        let pixels = engine.blobs().take(blob).unwrap();
        // The hash in the reply is the digest of exactly the pixels in the blob.
        assert_eq!(reply["hash"].as_str().unwrap(), digest::of_bytes(&pixels));
        hashes.push(reply["hash"].as_str().unwrap().to_owned());
    };
    tile(&mut engine, 10, &mut hashes);
    send(
        &mut engine,
        &request(
            11,
            "input.text",
            json!({ "doc_id": doc_id, "text": "Hello, world" }),
        ),
    );
    tile(&mut engine, 12, &mut hashes);
    send(
        &mut engine,
        &request(
            13,
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "update", "text": "日本語", "selection": { "start": 0, "end": 2 } }),
        ),
    );
    // The C interface's direct path, bayan_render_tile.
    let direct = engine.render_tile(
        serde_json::to_string(&whole_page(doc_id, 0, 51, 66))
            .unwrap()
            .as_bytes(),
        51,
        66,
        40,
    );
    hashes.push(digest::of_bytes(&direct.pixels.unwrap()));
    send(
        &mut engine,
        &request(
            14,
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "end" }),
        ),
    );
    send(
        &mut engine,
        &request(
            15,
            "input.pointer",
            json!({ "doc_id": doc_id, "kind": "down", "x": 110, "y": 230, "button": 0 }),
        ),
    );
    send(
        &mut engine,
        &request(
            16,
            "input.key",
            json!({ "doc_id": doc_id, "key": "Backspace" }),
        ),
    );
    send(
        &mut engine,
        &request(17, "query.a11y", json!({ "doc_id": doc_id })),
    );
    tile(&mut engine, 18, &mut hashes);
    // A failed request is recorded too, and must fail again identically.
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(19, "render.tile", whole_page(doc_id, 7, 1, 1))
            ),
            19
        ),
        "not_found"
    );

    let stopped = send(&mut engine, &request(20, "diag.record.stop", json!({})));
    let stopped = ok_payload(&stopped, 20);
    assert_eq!(stopped["truncated"], json!(false));
    assert_eq!(stopped["entries"], json!(11));
    let recording = engine
        .blobs()
        .take(stopped["blob"].as_u64().unwrap())
        .unwrap()
        .to_vec();
    (recording, hashes)
}

#[test]
fn replay_reproduces_identical_tile_hashes() {
    let (recording, hashes) = recorded_session();
    let report = replay(Config::default(), &recording).unwrap();
    assert!(report.identical, "{report:?}");
    assert_eq!(report.entries, 11);
    let replayed: Vec<&str> = report.tiles.iter().map(|tile| tile.hash.as_str()).collect();
    assert_eq!(replayed, hashes);
    // The tiles differ from one another, so equal hashes mean something.
    assert_ne!(hashes[0], hashes[1]);
    assert_ne!(hashes[1], hashes[2]);
}

#[test]
fn replay_through_the_protocol_reports_the_same_tiles() {
    let (recording, hashes) = recorded_session();
    let mut engine = engine();
    open_document(&mut engine);
    let blob = engine.blobs().put_shell(&recording).unwrap();
    let answers = send(
        &mut engine,
        &request(50, "diag.replay", json!({ "blob": blob })),
    );
    let report = ok_payload(&answers, 50);
    assert_eq!(report["identical"], json!(true));
    assert_eq!(report["first_difference"], Value::Null);
    let replayed: Vec<&str> = report["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tile| tile["hash"].as_str().unwrap())
        .collect();
    assert_eq!(replayed, hashes);
    // Replaying does not touch the shell's own session: its document is still open.
    let still_open = send(
        &mut engine,
        &request(51, "query.a11y", json!({ "doc_id": 1 })),
    );
    assert_eq!(ok_payload(&still_open, 51)["doc_id"], json!(1));
}

#[test]
fn replaying_twice_gives_the_same_report() {
    let (recording, _) = recorded_session();
    assert_eq!(
        replay(Config::default(), &recording),
        replay(Config::default(), &recording)
    );
}

#[test]
fn a_replay_that_renders_differently_is_caught() {
    let (recording, _) = recorded_session();
    let mut file: Value = serde_json::from_slice(&recording).unwrap();
    // Change the text typed during the recording: every later tile differs.
    let entries = file["entries"].as_array_mut().unwrap();
    let typed = entries
        .iter_mut()
        .find(|entry| {
            entry["text"]
                .as_str()
                .is_some_and(|text| text.contains("Hello, world"))
        })
        .unwrap();
    typed["text"] = json!(
        typed["text"]
            .as_str()
            .unwrap()
            .replace("Hello, world", "Hello, World")
    );
    let changed = serde_json::to_vec(&file).unwrap();
    let report = replay(Config::default(), &changed).unwrap();
    assert!(!report.identical);
    // The typing itself produces the same kinds of events, but their content differs.
    assert_eq!(report.first_difference, Some(1));
}

#[test]
fn recording_is_refused_twice_and_replay_while_recording() {
    let mut engine = engine();
    open_document(&mut engine);
    assert!(
        ok_payload(
            &send(&mut engine, &request(3, "diag.record.start", json!({}))),
            3
        )
        .is_object()
    );
    assert_eq!(
        error_code(
            &send(&mut engine, &request(4, "diag.record.start", json!({}))),
            4
        ),
        "already_recording"
    );
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(5, "diag.replay", json!({ "blob": 1 }))
            ),
            5
        ),
        "not_allowed"
    );
    assert!(
        ok_payload(
            &send(&mut engine, &request(6, "diag.record.stop", json!({}))),
            6
        )["blob"]
            .is_u64()
    );
    assert_eq!(
        error_code(
            &send(&mut engine, &request(7, "diag.record.stop", json!({}))),
            7
        ),
        "not_recording"
    );
}

#[test]
fn a_recording_keeps_the_blobs_it_used() {
    let mut engine = engine_with(Config::default());
    send(
        &mut engine,
        &request(1, "hello", json!({ "protocol_versions": [0] })),
    );
    send(&mut engine, &request(2, "diag.record.start", json!({})));
    let blob = engine.blobs().put_shell(b"document bytes").unwrap();
    send(
        &mut engine,
        &request(3, "doc.open", json!({ "blob": blob })),
    );
    // The shell releases the blob at once; the recording already holds a copy.
    assert!(engine.blobs().release(blob));
    let stopped = send(&mut engine, &request(4, "diag.record.stop", json!({})));
    let recording = engine
        .blobs()
        .take(ok_payload(&stopped, 4)["blob"].as_u64().unwrap())
        .unwrap();
    let file: Value = serde_json::from_slice(&recording).unwrap();
    assert_eq!(file["blobs"][0]["id"], json!(blob));
    assert_eq!(file["blobs"][0]["base64"], json!("ZG9jdW1lbnQgYnl0ZXM="));
    assert!(replay(Config::default(), &recording).unwrap().identical);
}

/// Records `count` tiles through the message path, each `tile(doc_id)` and released by the shell as soon as it arrives, as the web worker host does; returns the recording.
fn recording_of_released_tiles(count: u64, tile: impl Fn(u64) -> Value) -> Vec<u8> {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    for id in 10..10 + count {
        let answers = send(&mut engine, &request(id, "render.tile", tile(doc_id)));
        let blob = ok_payload(&answers, id)["blob"].as_u64().unwrap();
        assert!(engine.blobs().release(blob));
    }
    let stopped = send(&mut engine, &request(2, "diag.record.stop", json!({})));
    let stopped = ok_payload(&stopped, 2);
    assert_eq!(stopped["truncated"], json!(false));
    engine
        .blobs()
        .take(stopped["blob"].as_u64().unwrap())
        .unwrap()
        .to_vec()
}

#[test]
fn a_replay_of_more_tiles_than_blobs_may_exist_at_once_is_identical() {
    // 1,100 tiles: more than the 1,024 blobs that may exist at once (spec §13). The shell released each one while recording, so the replay must too.
    let recording = recording_of_released_tiles(1100, |doc_id| whole_page(doc_id, 0, 1, 1));
    let report = replay(Config::default(), &recording).unwrap();
    assert_eq!(report.first_difference, None);
    assert!(report.identical);
    assert_eq!(report.tiles.len(), 1100);
}

#[test]
fn a_replay_of_more_pixels_than_blobs_may_hold_at_once_is_identical() {
    // 300 tiles of 1 MiB each: more than the 256 MiB that all blobs may hold together (spec §13). They show an empty area beside the page, which is quick to draw (every pixel stays transparent) but just as large.
    let recording = recording_of_released_tiles(300, |doc_id| {
        json!({
            "doc_id": doc_id,
            "page": 0,
            "rect": { "x": -40_000_000, "y": 0, "width": 15_544_800, "height": 15_544_800 },
            "width": 512,
            "height": 512,
        })
    });
    let report = replay(Config::default(), &recording).unwrap();
    assert_eq!(report.first_difference, None);
    assert!(report.identical);
    assert_eq!(report.tiles.len(), 300);
}

#[test]
fn a_blob_the_engine_created_and_a_later_message_read_survives_the_replay() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    let answers = send(
        &mut engine,
        &request(4, "render.tile", whole_page(doc_id, 0, 2, 2)),
    );
    let tile = ok_payload(&answers, 4)["blob"].as_u64().unwrap();
    // The shell keeps the tile's blob and opens it as a document, so the recording carries it.
    let opened = send(
        &mut engine,
        &request(5, "doc.open", json!({ "blob": tile })),
    );
    assert!(ok_payload(&opened, 5)["doc_id"].is_u64());
    let stopped = send(&mut engine, &request(6, "diag.record.stop", json!({})));
    let recording = engine
        .blobs()
        .take(ok_payload(&stopped, 6)["blob"].as_u64().unwrap())
        .unwrap();
    let report = replay(Config::default(), &recording).unwrap();
    assert_eq!(report.first_difference, None);
}

#[test]
fn a_message_the_host_refused_as_oversized_truncates_a_recording_only_without_an_identifier() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let parse = |answers: Vec<String>| -> Vec<Value> {
        answers
            .iter()
            .map(|json| serde_json::from_str(json).unwrap())
            .collect()
    };
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    // With an identifier, the host's refusal is a reply, which a replay never compares: the recording goes on.
    let answers = parse(engine.refuse_oversized(Some(4)));
    assert_eq!(error_code(&answers, 4), "limit_exceeded");
    assert_eq!(answers[0]["error"]["args"]["limit"], json!("message_size"));
    send(
        &mut engine,
        &request(5, "query.a11y", json!({ "doc_id": doc_id })),
    );
    // Without one, the refusal is an event, which counts towards `seq`; the recording cannot hold it and stops there.
    let answers = parse(engine.refuse_oversized(None));
    assert_eq!(answers[0]["type"], json!("engine.error"));
    assert_eq!(answers[0]["payload"]["recoverable"], json!(true));
    send(
        &mut engine,
        &request(6, "query.a11y", json!({ "doc_id": doc_id })),
    );
    let stopped = send(&mut engine, &request(7, "diag.record.stop", json!({})));
    let stopped = ok_payload(&stopped, 7);
    assert_eq!(stopped["truncated"], json!(true));
    // Only the message before the refusal was kept, and it replays identically.
    assert_eq!(stopped["entries"], json!(1));
    let blob = stopped["blob"].as_u64().unwrap();
    let recording = engine.blobs().take(blob).unwrap();
    let report = replay(Config::default(), &recording).unwrap();
    assert!(report.identical);
}

#[test]
fn a_message_the_host_could_not_serialize_is_answered_like_an_oversized_one() {
    let mut engine = engine();
    open_document(&mut engine);
    let parse = |answers: Vec<String>| -> Vec<Value> {
        answers
            .iter()
            .map(|json| serde_json::from_str(json).unwrap())
            .collect()
    };
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    let answers = parse(engine.refuse_unreadable(Some(4)));
    assert_eq!(error_code(&answers, 4), "invalid_message");
    let answers = parse(engine.refuse_unreadable(None));
    assert_eq!(answers[0]["type"], json!("engine.error"));
    assert_eq!(answers[0]["payload"]["code"], json!("invalid_message"));
    let stopped = send(&mut engine, &request(5, "diag.record.stop", json!({})));
    assert_eq!(ok_payload(&stopped, 5)["truncated"], json!(true));
}

#[test]
fn a_full_blob_store_does_not_lose_the_recording() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(&mut engine, &request(3, "diag.record.start", json!({})));
    send(
        &mut engine,
        &request(4, "query.a11y", json!({ "doc_id": doc_id })),
    );
    // The shell holds as many blobs as may exist at once.
    let mut held = Vec::new();
    while let Ok(blob) = engine.blobs().put_shell(b"") {
        held.push(blob);
    }
    let refused = send(&mut engine, &request(5, "diag.record.stop", json!({})));
    assert_eq!(error_code(&refused, 5), "limit_exceeded");
    // The recording keeps running; once the shell releases a blob, stopping works.
    assert!(engine.blobs().release(held.pop().unwrap()));
    let stopped = send(&mut engine, &request(6, "diag.record.stop", json!({})));
    let stopped = ok_payload(&stopped, 6);
    // The query, and the stop that could not be stored, which a replay answers with not_recording instead.
    assert_eq!(stopped["entries"], json!(2));
    let recording = engine
        .blobs()
        .take(stopped["blob"].as_u64().unwrap())
        .unwrap();
    let report = replay(Config::default(), &recording).unwrap();
    assert_eq!(report.first_difference, Some(1));
}
