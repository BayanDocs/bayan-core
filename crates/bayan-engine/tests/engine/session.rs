//! The v0 messages behave as the engine protocol specification says: handshake, documents, view, tiles, input on the mock document's editable line, accessibility, the UI manifest stub, errors and limits.

use crate::support::{
    engine, error_code, events, notice, ok_payload, open_document, reply_to, request, send, view,
    whole_page,
};
use bayan_engine::limits::{MAX_BLOBS, MAX_DOCUMENTS, MAX_EDITABLE_TEXT_UNITS, MAX_MESSAGE_BYTES};
use serde_json::{Value, json};

#[test]
fn the_handshake_comes_first_and_agrees_on_version_0() {
    let mut engine = engine();
    assert_eq!(
        error_code(&send(&mut engine, &request(1, "ui.manifest", json!({}))), 1),
        "handshake_required"
    );
    let refused = send(
        &mut engine,
        &request(2, "hello", json!({ "protocol_versions": [1, 2] })),
    );
    assert_eq!(error_code(&refused, 2), "unsupported_protocol_version");
    assert_eq!(
        reply_to(&refused, 2)["error"]["args"]["supported"],
        json!([0])
    );
    let welcome = send(
        &mut engine,
        &request(3, "hello", json!({ "protocol_versions": [0, 1] })),
    );
    let reply = reply_to(&welcome, 3);
    assert_eq!(reply["type"], json!("welcome"));
    assert_eq!(reply["payload"]["protocol_version"], json!(0));
    assert_eq!(reply["payload"]["layout_epoch"], json!(0));
    assert_eq!(
        reply["payload"]["features"],
        json!(["mock_document", "record_replay"])
    );
    assert_eq!(
        reply["payload"]["engine_version"],
        json!(bayan_engine::ENGINE_VERSION)
    );
}

#[test]
fn a_new_hello_closes_every_document() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(
        &mut engine,
        &request(3, "hello", json!({ "protocol_versions": [0] })),
    );
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(4, "doc.close", json!({ "doc_id": doc_id }))
            ),
            4
        ),
        "not_found"
    );
}

#[test]
fn opens_the_mock_document_from_any_blob_and_closes_it() {
    let mut engine = engine();
    send(
        &mut engine,
        &request(1, "hello", json!({ "protocol_versions": [0] })),
    );
    assert_eq!(
        error_code(
            &send(&mut engine, &request(2, "doc.open", json!({ "blob": 9 }))),
            2
        ),
        "not_found"
    );
    let blob = engine
        .blobs()
        .put_shell(b"PK\x03\x04 not really a document")
        .unwrap();
    let opened = send(
        &mut engine,
        &request(
            3,
            "doc.open",
            json!({ "blob": blob, "file_name_hint": "letter.docx" }),
        ),
    );
    let reply = reply_to(&opened, 3);
    assert_eq!(reply["type"], json!("doc.opened"));
    assert_eq!(reply["payload"]["page_count_estimate"], json!(3));
    assert_eq!(reply["payload"]["warnings"], json!([]));
    assert_eq!(
        reply["payload"]["fonts"],
        json!({ "missing": [], "machine_dependent": false })
    );
    let doc_id = reply["payload"]["doc_id"].as_u64().unwrap();
    // The file name hint is never repeated anywhere.
    let all = send(
        &mut engine,
        &request(4, "query.a11y", json!({ "doc_id": doc_id })),
    );
    assert!(!serde_json::to_string(&all).unwrap().contains("letter.docx"));
    assert!(
        ok_payload(
            &send(
                &mut engine,
                &request(5, "doc.close", json!({ "doc_id": doc_id }))
            ),
            5
        )
        .is_object()
    );
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(6, "doc.close", json!({ "doc_id": doc_id }))
            ),
            6
        ),
        "not_found"
    );
}

#[test]
fn view_set_reports_pages_invalidation_progress_and_the_caret() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let answers = send(&mut engine, &request(3, "view.set", view(doc_id, 1.5)));
    assert_eq!(ok_payload(&answers, 3), &json!({}));
    let kinds: Vec<&str> = answers
        .iter()
        .skip(1)
        .map(|message| message["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "view.pages",
            "render.invalidate",
            "view.layout.progress",
            "overlay.update"
        ]
    );
    let pages = &answers[1]["payload"]["pages"];
    assert_eq!(pages.as_array().unwrap().len(), 3);
    assert_eq!(
        pages[0],
        json!({ "index": 0, "width": 15_544_800, "height": 20_116_800, "section": 0, "x": 0, "y": 0 })
    );
    assert_eq!(pages[2]["y"], json!(2 * (20_116_800 + 457_200)));
    assert_eq!(answers[3]["payload"]["complete"], json!(true));
    let caret = &answers[4]["payload"]["caret"];
    assert_eq!(caret["page"], json!(0));
    assert_eq!(caret["visible"], json!(true));
    // Events carry increasing sequence numbers.
    let sequence: Vec<u64> = answers
        .iter()
        .filter_map(|message| message["seq"].as_u64())
        .collect();
    assert_eq!(sequence, [1, 2, 3, 4]);
    for bad in [0.0, 100.0, -1.0] {
        let refused = send(&mut engine, &request(4, "view.set", view(doc_id, bad)));
        assert_eq!(error_code(&refused, 4), "invalid_request", "zoom {bad}");
    }
}

#[test]
fn tiles_are_premultiplied_rgba_and_transparent_outside_the_page() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    // A tile that starts 100 BLU-wide pixels left of the page: its first column is outside it.
    let request_json = json!({ "doc_id": doc_id, "page": 0, "rect": { "x": -1_000, "y": 0, "width": 4_000, "height": 4_000 } });
    let outcome = engine.render_tile(request_json.to_string().as_bytes(), 4, 4, 0);
    let pixels = outcome.pixels.unwrap();
    assert_eq!(pixels.len(), 4 * 4 * 4);
    assert_eq!(&pixels[0..4], &[0, 0, 0, 0]);
    assert_eq!(&pixels[4..8], &[255, 255, 255, 255]);
    // Every pixel is premultiplied: no colour channel exceeds its alpha.
    let whole = engine.render_tile(
        whole_page(doc_id, 0, 85, 110).to_string().as_bytes(),
        85,
        110,
        0,
    );
    for pixel in whole.pixels.unwrap().chunks(4) {
        assert!(pixel[0] <= pixel[3] && pixel[1] <= pixel[3] && pixel[2] <= pixel[3]);
    }
}

#[test]
fn the_direct_tile_path_checks_its_request() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let page = whole_page(doc_id, 0, 10, 10).to_string();
    let render = |engine: &mut bayan_engine::Engine, request: &str, width, height| {
        engine
            .render_tile(request.as_bytes(), width, height, 0)
            .pixels
            .map(|pixels| pixels.len())
    };
    assert_eq!(render(&mut engine, &page, 10, 10), Ok(400));
    // The payload's own size, when present, must equal the parameters.
    assert_eq!(
        render(&mut engine, &page, 11, 10),
        Err(bayan_engine::TileError::InvalidArgument)
    );
    assert_eq!(
        render(&mut engine, "{", 10, 10),
        Err(bayan_engine::TileError::InvalidArgument)
    );
    assert_eq!(
        render(&mut engine, &whole_page(doc_id, 3, 1, 1).to_string(), 1, 1),
        Err(bayan_engine::TileError::NotFound)
    );
    assert_eq!(
        render(&mut engine, &whole_page(77, 0, 1, 1).to_string(), 1, 1),
        Err(bayan_engine::TileError::NotFound)
    );
    assert_eq!(
        render(
            &mut engine,
            &whole_page(doc_id, 0, 4097, 1).to_string(),
            4097,
            1
        ),
        Err(bayan_engine::TileError::InvalidArgument)
    );
    let zero =
        json!({ "doc_id": doc_id, "page": 0, "rect": { "x": 0, "y": 0, "width": 0, "height": 5 } })
            .to_string();
    assert_eq!(
        render(&mut engine, &zero, 1, 1),
        Err(bayan_engine::TileError::InvalidArgument)
    );
}

#[test]
fn the_message_tile_path_returns_a_blob_with_its_hash() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let answers = send(
        &mut engine,
        &request(3, "render.tile", whole_page(doc_id, 1, 20, 26)),
    );
    let reply = ok_payload(&answers, 3);
    assert_eq!(
        (
            reply["width"].as_u64(),
            reply["height"].as_u64(),
            reply["stride"].as_u64()
        ),
        (Some(20), Some(26), Some(80))
    );
    let blob = reply["blob"].as_u64().unwrap();
    // Blobs the engine creates have even identifiers (engine protocol §3.1).
    assert_eq!(blob % 2, 0);
    let pixels = engine.blobs().take(blob).unwrap();
    assert_eq!(pixels.len(), 20 * 26 * 4);
    assert_eq!(
        reply["hash"].as_str().unwrap(),
        bayan_engine::digest::of_bytes(&pixels)
    );
    // Without a size, the message form is refused.
    let mut sizeless = whole_page(doc_id, 0, 1, 1);
    sizeless.as_object_mut().unwrap().remove("width");
    assert_eq!(
        error_code(&send(&mut engine, &request(4, "render.tile", sizeless)), 4),
        "invalid_request"
    );
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(5, "render.tile", whole_page(doc_id, 0, 0, 1))
            ),
            5
        ),
        "limit_exceeded"
    );
}

#[test]
fn requests_without_an_identifier_leave_no_blob_behind() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    // `open_document` put the shell's blob; it is the only one.
    assert_eq!(engine.blobs().len(), 1);
    // Nobody could learn the identifier of a tile or recording sent without a reply, so the engine keeps none.
    let answers = send(
        &mut engine,
        &notice("render.tile", whole_page(doc_id, 0, 4, 4)),
    );
    assert!(answers.is_empty(), "{answers:?}");
    assert!(
        send(&mut engine, &request(3, "diag.record.start", json!({})))
            .iter()
            .all(|message| message["ok"] == json!(true))
    );
    assert!(send(&mut engine, &notice("diag.record.stop", json!({}))).is_empty());
    assert_eq!(engine.blobs().len(), 1);
    // The recording stopped all the same.
    assert_eq!(
        error_code(
            &send(&mut engine, &request(4, "diag.record.stop", json!({}))),
            4
        ),
        "not_recording"
    );
}

#[test]
fn blob_0_stands_for_bytes_that_could_not_be_stored() {
    let mut engine = engine();
    open_document(&mut engine);
    // `bayan_blob_put` returns 0 when a limit refuses the bytes, and the web worker host passes 0 for an ArrayBuffer it cannot store (engine protocol §3).
    let answers = send(&mut engine, &request(3, "doc.open", json!({ "blob": 0 })));
    assert_eq!(error_code(&answers, 3), "limit_exceeded");
    assert_eq!(
        reply_to(&answers, 3)["error"]["args"]["limit"],
        json!("blob")
    );
    // Any other unknown blob is simply not found.
    let answers = send(&mut engine, &request(4, "doc.open", json!({ "blob": 9 })));
    assert_eq!(error_code(&answers, 4), "not_found");
}

#[test]
fn a_host_that_cannot_deliver_a_reply_answers_with_internal() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let answers: Vec<Value> = engine
        .report_host_failure(Some(3))
        .iter()
        .map(|json| serde_json::from_str(json).unwrap())
        .collect();
    assert_eq!(answers.len(), 1);
    assert_eq!(error_code(&answers, 3), "internal");
    // A message without an identifier, or with one that is not a request identifier, has nobody to answer.
    for id in [None, Some(0), Some(1 << 53)] {
        assert!(engine.report_host_failure(id).is_empty());
    }
    // The session is untouched.
    let answers = send(
        &mut engine,
        &request(4, "query.a11y", json!({ "doc_id": doc_id })),
    );
    assert_eq!(ok_payload(&answers, 4)["root"]["role"], json!("document"));
}

#[test]
fn typing_and_composing_change_the_editable_line() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let answers = send(
        &mut engine,
        &notice("input.text", json!({ "doc_id": doc_id, "text": "Hi" })),
    );
    let kinds: Vec<&str> = answers
        .iter()
        .map(|message| message["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        ["render.invalidate", "overlay.update", "a11y.update"]
    );
    let node = &answers[2]["payload"]["changes"][0]["node"];
    assert_eq!(node["text"], json!("Hi"));
    assert_eq!(node["editable"], json!(true));
    assert_eq!(node["name"], json!("Editable line"));
    assert_eq!(answers[2]["payload"]["caret"]["offset"], json!(2));
    let first_caret = answers[1]["payload"]["caret"]["rect"]["x"]
        .as_i64()
        .unwrap();

    let composing = send(
        &mut engine,
        &notice(
            "input.composition",
            json!({ "doc_id": doc_id, "phase": "update", "text": "😀ab", "selection": { "start": 2, "end": 3 } }),
        ),
    );
    let node = &events(&composing, "a11y.update")[0]["payload"];
    assert_eq!(node["changes"][0]["node"]["text"], json!("Hi😀ab"));
    // The caret sits after the composition's selection: "Hi" (2) + "😀a" (3 UTF-16 code units).
    assert_eq!(node["caret"]["offset"], json!(5));
    let composing_caret = events(&composing, "overlay.update")[0]["payload"]["caret"]["rect"]["x"]
        .as_i64()
        .unwrap();
    assert!(composing_caret > first_caret);

    // Keys belong to the input method while it composes.
    assert!(
        send(
            &mut engine,
            &notice("input.key", json!({ "doc_id": doc_id, "key": "Backspace" }))
        )
        .is_empty()
    );
    let committed = send(
        &mut engine,
        &notice("input.text", json!({ "doc_id": doc_id, "text": "!" })),
    );
    assert_eq!(
        events(&committed, "a11y.update")[0]["payload"]["changes"][0]["node"]["text"],
        json!("Hi!")
    );
    let deleted = send(
        &mut engine,
        &notice("input.key", json!({ "doc_id": doc_id, "key": "Backspace" })),
    );
    assert_eq!(
        events(&deleted, "a11y.update")[0]["payload"]["changes"][0]["node"]["text"],
        json!("Hi")
    );
    // A key the line does not use changes nothing and sends nothing.
    assert!(
        send(
            &mut engine,
            &notice(
                "input.key",
                json!({ "doc_id": doc_id, "key": "a", "code": "KeyA" })
            )
        )
        .is_empty()
    );
}

#[test]
fn a_click_moves_the_caret_inside_the_editable_line() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(
        &mut engine,
        &notice("input.text", json!({ "doc_id": doc_id, "text": "abcdef" })),
    );
    // Before view.set the engine cannot tell where a click lands.
    let early = send(
        &mut engine,
        &notice(
            "input.pointer",
            json!({ "doc_id": doc_id, "kind": "down", "x": 100, "y": 200 }),
        ),
    );
    assert!(early.is_empty());
    let set = send(&mut engine, &request(3, "view.set", view(doc_id, 1.0)));
    let caret = &events(&set, "overlay.update")[0]["payload"]["caret"]["rect"];
    // Click on the caret's line, at the left margin of the text: the caret goes to the start.
    let x = (caret["x"].as_f64().unwrap() - 6.0 * 137_160.0) / 19_050.0;
    let y = (caret["y"].as_f64().unwrap() + 100_000.0) / 19_050.0;
    let moved = send(
        &mut engine,
        &notice(
            "input.pointer",
            json!({ "doc_id": doc_id, "kind": "down", "x": x, "y": y, "button": 0 }),
        ),
    );
    assert_eq!(
        events(&moved, "a11y.update")[0]["payload"]["caret"]["offset"],
        json!(0)
    );
    // The secondary button, and other kinds of pointer events, do nothing.
    for (kind, button) in [("down", 2), ("move", 0), ("up", 0)] {
        let ignored = send(
            &mut engine,
            &notice(
                "input.pointer",
                json!({ "doc_id": doc_id, "kind": kind, "x": x + 30.0, "y": y, "button": button }),
            ),
        );
        assert!(ignored.is_empty(), "{kind} {button}");
    }
}

#[test]
fn the_accessibility_tree_has_the_structure_of_the_document() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let answers = send(
        &mut engine,
        &request(3, "query.a11y", json!({ "doc_id": doc_id })),
    );
    let tree = ok_payload(&answers, 3);
    let root = &tree["root"];
    assert_eq!(root["role"], json!("document"));
    let children = root["children"].as_array().unwrap();
    let roles: Vec<&str> = children
        .iter()
        .map(|child| child["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles[0], "heading");
    assert_eq!(children[0]["level"], json!(1));
    assert_eq!(children[0]["text"], json!("BayanDocs mock document"));
    for role in ["paragraph", "list", "table"] {
        assert!(roles.contains(&role), "{role}");
    }
    let table = children
        .iter()
        .find(|child| child["role"] == json!("table"))
        .unwrap();
    let rows = table["children"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["children"][0]["role"], json!("column_header"));
    assert_eq!(rows[1]["children"][1]["text"], json!("Qt Quick"));
    let list = children
        .iter()
        .find(|child| child["role"] == json!("list"))
        .unwrap();
    assert_eq!(list["children"].as_array().unwrap().len(), 3);
    // Every text node has one box per character.
    let heading_chars = children[0]["chars"].as_array().unwrap();
    assert_eq!(heading_chars.len(), "BayanDocs mock document".len());
    // A subtree can be asked for by node.
    let table_id = table["id"].as_u64().unwrap();
    let subtree = send(
        &mut engine,
        &request(
            4,
            "query.a11y",
            json!({ "doc_id": doc_id, "node": table_id }),
        ),
    );
    assert_eq!(ok_payload(&subtree, 4)["root"]["role"], json!("table"));
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(
                    5,
                    "query.a11y",
                    json!({ "doc_id": doc_id, "node": 123_456 })
                )
            ),
            5
        ),
        "not_found"
    );
}

#[test]
fn the_ui_manifest_stub_names_commands_and_a_ribbon() {
    let mut engine = engine();
    open_document(&mut engine);
    let answers = send(
        &mut engine,
        &request(3, "ui.manifest", json!({ "locale": "ar" })),
    );
    let manifest = ok_payload(&answers, 3);
    assert_eq!(manifest["manifest_version"], json!(0));
    assert_eq!(manifest["locale"], json!("en-US"));
    assert!(
        manifest["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["id"] == json!("format.bold.toggle"))
    );
    assert_eq!(manifest["ribbon"]["tabs"][1]["label"], json!("Home"));
}

#[test]
fn unknown_and_unimplemented_messages_are_refused_without_echoing_content() {
    let mut engine = engine();
    open_document(&mut engine);
    for kind in ["cmd.exec", "doc.save", "no.such.thing"] {
        let answers = send(
            &mut engine,
            &request(3, kind, json!({ "secret": "document text" })),
        );
        assert_eq!(error_code(&answers, 3), "unsupported_message");
        let text = serde_json::to_string(&answers).unwrap();
        assert!(!text.contains("document text"), "{text}");
    }
    // The type is repeated only if it looks like a message type (spec §12); anything else is left out, not cut.
    let answers = send(&mut engine, &request(4, "no.such.thing", json!({})));
    assert_eq!(
        reply_to(&answers, 4)["error"]["args"]["type"],
        json!("no.such.thing")
    );
    for kind in ["x".repeat(65), "Document Text".to_owned()] {
        let answers = send(&mut engine, &request(5, &kind, json!({})));
        assert_eq!(error_code(&answers, 5), "unsupported_message");
        assert_eq!(reply_to(&answers, 5)["error"]["args"], json!({}), "{kind}");
    }
    // Without an id, failures arrive as recoverable engine.error events.
    let answers = send(&mut engine, &notice("no.such.thing", json!({})));
    assert_eq!(answers[0]["type"], json!("engine.error"));
    assert_eq!(answers[0]["payload"]["recoverable"], json!(true));
}

#[test]
fn the_limits_are_enforced() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    // Documents.
    for id in 0..MAX_DOCUMENTS - 1 {
        let blob = engine.blobs().put_shell(b"").unwrap();
        let opened = send(
            &mut engine,
            &request(
                100 + u64::try_from(id).unwrap(),
                "doc.open",
                json!({ "blob": blob }),
            ),
        );
        assert!(opened[0]["ok"] == json!(true));
    }
    let blob = engine.blobs().put_shell(b"").unwrap();
    assert_eq!(
        error_code(
            &send(
                &mut engine,
                &request(3, "doc.open", json!({ "blob": blob }))
            ),
            3
        ),
        "limit_exceeded"
    );
    // The editable line's text.
    let long = "a".repeat(MAX_EDITABLE_TEXT_UNITS + 1);
    let answers = send(
        &mut engine,
        &request(4, "input.text", json!({ "doc_id": doc_id, "text": long })),
    );
    assert_eq!(error_code(&answers, 4), "limit_exceeded");
    assert_eq!(
        reply_to(&answers, 4)["error"]["args"]["limit"],
        json!("text_length")
    );
    // Messages.
    let huge = vec![b' '; MAX_MESSAGE_BYTES + 1];
    let answers = engine.handle(&huge, 0);
    let answer: Value = serde_json::from_str(&answers[0]).unwrap();
    assert_eq!(answer["payload"]["code"], json!("limit_exceeded"));
    // Blobs created by the engine count against the blob limit too.
    while engine.blobs().len() < MAX_BLOBS {
        engine.blobs().put_shell(b"").unwrap();
    }
    let full = send(
        &mut engine,
        &request(5, "render.tile", whole_page(doc_id, 0, 1, 1)),
    );
    assert_eq!(reply_to(&full, 5)["error"]["args"]["limit"], json!("blobs"));
}
