//! Protocol v1's text positions, the clipboard and the accessibility tree's content, through the protocol as a shell sends them (spec §14.4, §14.5, §14.7 item 2; CORE-014).

use bayan_engine::Engine;
use serde_json::{Value, json};

use crate::support::{
    engine, error_code, events, notice, ok_payload, open_document, request, send, view,
};

/// The mock document's editable line, and its first heading, which is not editable.
const EDITABLE: u64 = 5;
const HEADING: u64 = 2;
/// The paragraph in Arabic on the last page.
const ARABIC: u64 = 35;

/// An engine with the mock document open and `text` typed into its editable line.
fn typed(text: &str) -> (Engine, u64) {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    send(
        &mut engine,
        &request(3, "input.text", json!({ "doc_id": doc_id, "text": text })),
    );
    (engine, doc_id)
}

/// The editable line's text, the accessibility caret's offset and the selection, from `query.a11y`.
fn line_state(engine: &mut Engine, doc_id: u64) -> (String, u64, Value) {
    let answers = send(
        engine,
        &request(
            90,
            "query.a11y",
            json!({ "doc_id": doc_id, "node": EDITABLE }),
        ),
    );
    let tree = ok_payload(&answers, 90);
    (
        tree["root"]["text"].as_str().unwrap().to_owned(),
        tree["caret"]["offset"].as_u64().unwrap(),
        tree["selection"].clone(),
    )
}

/// A text position in the editable line.
fn at(offset: u32) -> Value {
    json!({ "node": EDITABLE, "offset": offset })
}

fn replace(doc_id: u64, text: &str, node: u64, start: u32, end: u32) -> Value {
    json!({ "doc_id": doc_id, "text": text, "replace": { "node": node, "start": start, "end": end } })
}

/// The `engine.error` that answers a message without an identifier, with its code; it is recoverable.
fn recoverable_error(answers: &[Value]) -> String {
    let errors = events(answers, "engine.error");
    assert_eq!(errors.len(), 1, "{answers:?}");
    assert_eq!(errors[0]["payload"]["recoverable"], json!(true));
    errors[0]["payload"]["code"].as_str().unwrap().to_owned()
}

#[test]
fn input_text_with_replace_replaces_exactly_the_range_and_puts_the_caret_after_it() {
    // "a😀b": the emoji is the UTF-16 code units 1 and 2.
    let (mut engine, doc_id) = typed("a😀b");
    let answers = send(
        &mut engine,
        &request(4, "input.text", replace(doc_id, "xy", EDITABLE, 1, 3)),
    );
    ok_payload(&answers, 4);
    assert_eq!(events(&answers, "render.invalidate").len(), 1);
    let update = events(&answers, "a11y.update")[0];
    assert_eq!(
        update["payload"]["changes"][0]["node"]["text"],
        json!("axyb")
    );
    assert_eq!(
        update["payload"]["caret"],
        json!({ "node": EDITABLE, "offset": 3 })
    );
    assert_eq!(update["payload"]["selection"], json!([]));
    // Without an identifier it works the same; an insertion is a range with equal ends.
    let answers = send(
        &mut engine,
        &notice("input.text", replace(doc_id, "😀", EDITABLE, 0, 0)),
    );
    assert!(events(&answers, "engine.error").is_empty());
    assert_eq!(line_state(&mut engine, doc_id).0, "😀axyb");
    assert_eq!(line_state(&mut engine, doc_id).1, 2);
}

#[test]
fn input_text_with_replace_refuses_what_spec_14_4_refuses_with_and_without_an_identifier() {
    let (mut engine, doc_id) = typed("a😀b");
    let cases = [
        // A node that does not exist.
        (replace(doc_id, "x", 999_999, 0, 0), "not_found"),
        // A node that is not editable.
        (replace(doc_id, "x", HEADING, 0, 1), "invalid_request"),
        // A range outside the node's text, and one backwards.
        (replace(doc_id, "x", EDITABLE, 0, 5), "invalid_request"),
        (replace(doc_id, "x", EDITABLE, 3, 1), "invalid_request"),
        // An end inside the surrogate pair.
        (replace(doc_id, "x", EDITABLE, 2, 3), "invalid_request"),
        (replace(doc_id, "x", EDITABLE, 0, 2), "invalid_request"),
        // A replacement that would make the text longer than its limit.
        (
            replace(doc_id, &"z".repeat(1025), EDITABLE, 0, 4),
            "limit_exceeded",
        ),
    ];
    for (payload, code) in cases {
        let answers = send(&mut engine, &request(5, "input.text", payload.clone()));
        assert_eq!(error_code(&answers, 5), code, "{payload}");
        let answers = send(&mut engine, &notice("input.text", payload.clone()));
        assert_eq!(recoverable_error(&answers), code, "{payload}");
    }
    // Nothing changed.
    assert_eq!(line_state(&mut engine, doc_id).0, "a😀b");
    // The node's arguments name only what is missing, never content.
    let answers = send(
        &mut engine,
        &request(6, "input.text", replace(doc_id, "secret", 999_999, 0, 0)),
    );
    assert_eq!(
        answers[0]["error"]["args"],
        json!({ "what": "node" }),
        "{answers:?}"
    );
}

#[test]
fn input_select_sets_a_selection_and_answers_with_the_overlay_and_the_tree() {
    let (mut engine, doc_id) = typed("hello");
    let select =
        |anchor, focus| json!({ "doc_id": doc_id, "anchor": at(anchor), "focus": at(focus) });
    let answers = send(&mut engine, &request(4, "input.select", select(4, 1)));
    ok_payload(&answers, 4);
    // A selection changes no tile.
    assert!(events(&answers, "render.invalidate").is_empty());
    let overlay = events(&answers, "overlay.update")[0];
    assert_eq!(overlay["payload"]["caret"]["visible"], json!(false));
    assert_eq!(
        overlay["payload"]["selection"][0]["rects"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let update = events(&answers, "a11y.update")[0];
    assert_eq!(
        update["payload"]["selection"],
        json!([{ "anchor": at(4), "focus": at(1) }])
    );
    assert_eq!(update["payload"]["caret"]["offset"], json!(1));
    // The same selection again changes nothing, and the engine answers with nothing more, as for keys.
    let answers = send(&mut engine, &request(5, "input.select", select(4, 1)));
    assert_eq!(answers.len(), 1);
    // A caret: anchor and focus equal; the caret shows again and nothing is selected.
    let answers = send(&mut engine, &request(6, "input.select", select(2, 2)));
    let overlay = events(&answers, "overlay.update")[0];
    assert_eq!(overlay["payload"]["caret"]["visible"], json!(true));
    assert_eq!(overlay["payload"]["selection"], json!([]));
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("hello".to_owned(), 2, json!([]))
    );
}

#[test]
fn input_select_refuses_what_input_text_refuses_with_and_without_an_identifier() {
    let (mut engine, doc_id) = typed("a😀b");
    let cases = [
        (json!({ "node": 999_999, "offset": 0 }), at(0), "not_found"),
        (at(0), json!({ "node": 999_999, "offset": 0 }), "not_found"),
        (
            json!({ "node": HEADING, "offset": 0 }),
            at(0),
            "invalid_request",
        ),
        (at(5), at(0), "invalid_request"),
        (at(0), at(2), "invalid_request"),
    ];
    for (anchor, focus, code) in cases {
        let payload = json!({ "doc_id": doc_id, "anchor": anchor, "focus": focus });
        let answers = send(&mut engine, &request(4, "input.select", payload.clone()));
        assert_eq!(error_code(&answers, 4), code, "{payload}");
        let answers = send(&mut engine, &notice("input.select", payload.clone()));
        assert_eq!(recoverable_error(&answers), code, "{payload}");
    }
}

#[test]
fn input_select_during_a_composition_answers_with_the_selection_the_engine_kept() {
    let (mut engine, doc_id) = typed("abc");
    for (id, payload) in [
        (4, json!({ "doc_id": doc_id, "phase": "start" })),
        (
            5,
            json!({ "doc_id": doc_id, "phase": "update", "text": "xy", "selection": { "start": 2, "end": 2 } }),
        ),
    ] {
        ok_payload(
            &send(&mut engine, &request(id, "input.composition", payload)),
            id,
        );
    }
    let select =
        |anchor, focus| json!({ "doc_id": doc_id, "anchor": at(anchor), "focus": at(focus) });
    // The input method owns the caret, at the end of the composition: a selection that differs from it changes nothing, and the engine answers with the overlay and the tree as they are, so that the shell's mirror follows the engine's selection rather than the one it asked for.
    let answers = send(&mut engine, &request(6, "input.select", select(0, 1)));
    ok_payload(&answers, 6);
    assert!(events(&answers, "render.invalidate").is_empty());
    let overlay = events(&answers, "overlay.update")[0];
    assert_eq!(overlay["payload"]["selection"], json!([]));
    let update = events(&answers, "a11y.update")[0];
    assert_eq!(update["payload"]["selection"], json!([]));
    assert_eq!(update["payload"]["caret"]["offset"], json!(5));
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("abcxy".to_owned(), 5, json!([]))
    );
    // The selection the engine has, a caret at 5, gets no events, so that a shell that hears the engine's selection back starts no loop.
    let answers = send(&mut engine, &request(7, "input.select", select(5, 5)));
    assert_eq!(answers.len(), 1, "{answers:?}");
    // A notice without an identifier is answered the same way.
    let answers = send(&mut engine, &notice("input.select", select(1, 1)));
    assert_eq!(events(&answers, "overlay.update").len(), 1);
    assert_eq!(events(&answers, "a11y.update").len(), 1);
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("abcxy".to_owned(), 5, json!([]))
    );
}

#[test]
fn shift_with_the_arrows_home_and_end_extends_the_selection_and_typing_replaces_it() {
    let (mut engine, doc_id) = typed("hello");
    let key = |key: &str, shift: bool| json!({ "doc_id": doc_id, "key": key, "code": key, "modifiers": { "shift": shift } });
    // The caret is at the end; Shift+ArrowLeft twice selects "lo".
    send(
        &mut engine,
        &request(4, "input.key", key("ArrowLeft", true)),
    );
    send(
        &mut engine,
        &request(5, "input.key", key("ArrowLeft", true)),
    );
    assert_eq!(
        line_state(&mut engine, doc_id),
        (
            "hello".to_owned(),
            3,
            json!([{ "anchor": at(5), "focus": at(3) }])
        )
    );
    send(&mut engine, &request(6, "input.key", key("Home", true)));
    assert_eq!(
        line_state(&mut engine, doc_id).2,
        json!([{ "anchor": at(5), "focus": at(0) }])
    );
    // Typing replaces the selection.
    let answers = send(
        &mut engine,
        &request(7, "input.text", json!({ "doc_id": doc_id, "text": "J" })),
    );
    assert_eq!(events(&answers, "render.invalidate").len(), 1);
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("J".to_owned(), 1, json!([]))
    );
    // Shift+End, then Backspace deletes the selection; Shift+Home, then Delete too.
    send(&mut engine, &request(8, "input.key", key("Home", false)));
    send(&mut engine, &request(9, "input.key", key("End", true)));
    send(
        &mut engine,
        &request(10, "input.key", key("Backspace", false)),
    );
    assert_eq!(
        line_state(&mut engine, doc_id),
        (String::new(), 0, json!([]))
    );
    send(
        &mut engine,
        &request(11, "input.text", json!({ "doc_id": doc_id, "text": "abc" })),
    );
    send(
        &mut engine,
        &request(12, "input.key", key("ArrowRight", true)),
    );
    assert_eq!(line_state(&mut engine, doc_id).2, json!([]));
    send(
        &mut engine,
        &request(13, "input.key", key("ArrowLeft", true)),
    );
    send(&mut engine, &request(14, "input.key", key("Delete", false)));
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("ab".to_owned(), 2, json!([]))
    );
}

#[test]
fn query_hit_finds_the_text_position_at_a_point_and_nothing_off_the_pages() {
    let mut engine = engine();
    let doc_id = open_document(&mut engine);
    let hit = |engine: &mut Engine, x: f64, y: f64| {
        let answers = send(
            engine,
            &request(7, "query.hit", json!({ "doc_id": doc_id, "x": x, "y": y })),
        );
        ok_payload(&answers, 7)["hit"].clone()
    };
    // Before view.set the engine cannot place the point.
    assert_eq!(hit(&mut engine, 100.0, 100.0), json!(null));
    send(&mut engine, &request(3, "view.set", view(doc_id, 1.0)));
    // The first heading starts one inch (96 device-independent pixels) from the page's top-left corner: the left half of its first character.
    assert_eq!(
        hit(&mut engine, 96.5, 100.0),
        json!({ "node": HEADING, "caret": 0, "character": 0 })
    );
    // Left of the page, and in the gap below the first page (11 inches, 1,056 pixels, high).
    assert_eq!(hit(&mut engine, -1.0, 100.0), json!(null));
    assert_eq!(hit(&mut engine, 100.0, 1_060.0), json!(null));
    // The Arabic paragraph's first line ends at the right margin (7.5 inches, 720 pixels); a point in the right half of its first character, near its right edge, gives the offset before it.
    let tree = send(
        &mut engine,
        &request(8, "query.a11y", json!({ "doc_id": doc_id, "node": ARABIC })),
    );
    let paragraph = &ok_payload(&tree, 8)["root"];
    let first = &paragraph["chars"][0]["rect"];
    let page = paragraph["bounds"]["page"].as_i64().unwrap();
    let blu = |value: &Value| value.as_f64().unwrap();
    let to_dip = |x: f64| x / 19_050.0;
    let page_top = f64::from(i32::try_from(page).unwrap()) * (20_116_800.0 + 457_200.0);
    let x = to_dip(blu(&first["x"]) + blu(&first["width"]) - 1_000.0);
    let y = to_dip(page_top + blu(&first["y"]) + 1_000.0);
    assert_eq!(
        hit(&mut engine, x, y),
        json!({ "node": ARABIC, "caret": 0, "character": 0 })
    );
    // A coordinate beyond the range a view allows is refused.
    let answers = send(
        &mut engine,
        &request(
            9,
            "query.hit",
            json!({ "doc_id": doc_id, "x": 1.0e10, "y": 0 }),
        ),
    );
    assert_eq!(error_code(&answers, 9), "invalid_request");
}

/// Puts the shell's bytes in a new blob.
fn blob(engine: &mut Engine, bytes: &[u8]) -> u64 {
    engine.blobs().put_shell(bytes).unwrap()
}

#[test]
fn welcome_lists_the_clipboard_formats_the_engine_reads_and_writes() {
    let mut engine = engine();
    let answers = send(
        &mut engine,
        &request(
            1,
            "hello",
            json!({ "protocol_versions": [1], "capabilities": { "clipboard_formats": ["text/html", "text/plain"] } }),
        ),
    );
    assert_eq!(
        ok_payload(&answers, 1)["clipboard_formats"],
        json!(["text/plain"])
    );
}

#[test]
fn input_paste_inserts_plain_text_at_the_selection_and_ignores_other_formats() {
    let (mut engine, doc_id) = typed("hello");
    send(
        &mut engine,
        &request(
            4,
            "input.select",
            json!({ "doc_id": doc_id, "anchor": at(1), "focus": at(4) }),
        ),
    );
    // Raw bytes: an invalid byte becomes U+FFFD and a line feed goes, as for typed text; the image is not read.
    let image = blob(&mut engine, b"\x89PNG not read");
    let text = blob(&mut engine, b"ipp\xff\n");
    let answers = send(
        &mut engine,
        &request(
            5,
            "input.paste",
            json!({ "doc_id": doc_id, "items": [
            { "mime": "image/png", "blob": image },
            { "mime": "text/plain", "blob": text },
        ] }),
        ),
    );
    ok_payload(&answers, 5);
    assert_eq!(events(&answers, "render.invalidate").len(), 1);
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("hipp\u{FFFD}o".to_owned(), 5, json!([]))
    );
    // No format the engine reads: nothing happens.
    let answers = send(
        &mut engine,
        &request(
            6,
            "input.paste",
            json!({ "doc_id": doc_id, "items": [{ "mime": "text/html", "blob": image }] }),
        ),
    );
    assert_eq!(answers.len(), 1);
    // Blobs that do not exist, could not be stored, or hold more text than the line may: refused, and nothing changes.
    for (item_blob, code) in [(999_999, "not_found"), (0, "limit_exceeded")] {
        let answers = send(
            &mut engine,
            &request(
                7,
                "input.paste",
                json!({ "doc_id": doc_id, "items": [{ "mime": "text/plain", "blob": item_blob }] }),
            ),
        );
        assert_eq!(error_code(&answers, 7), code);
    }
    let long = blob(&mut engine, &[b'x'; 1025]);
    let answers = send(
        &mut engine,
        &request(
            8,
            "input.paste",
            json!({ "doc_id": doc_id, "items": [{ "mime": "text/plain", "blob": long }] }),
        ),
    );
    assert_eq!(error_code(&answers, 8), "limit_exceeded");
    assert_eq!(
        answers[0]["error"]["args"],
        json!({ "limit": "text_length" })
    );
    assert_eq!(line_state(&mut engine, doc_id).0, "hipp\u{FFFD}o");
}

#[test]
fn clipboard_copy_gives_the_selection_as_utf8_bytes_and_cut_deletes_it() {
    let (mut engine, doc_id) = typed("a😀b");
    // Nothing selected: no items.
    let answers = send(
        &mut engine,
        &request(4, "clipboard.copy", json!({ "doc_id": doc_id })),
    );
    assert_eq!(ok_payload(&answers, 4)["items"], json!([]));
    send(
        &mut engine,
        &request(
            5,
            "input.select",
            json!({ "doc_id": doc_id, "anchor": at(4), "focus": at(1) }),
        ),
    );
    let answers = send(
        &mut engine,
        &request(6, "clipboard.copy", json!({ "doc_id": doc_id })),
    );
    let items = ok_payload(&answers, 6)["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["mime"], json!("text/plain"));
    let bytes = engine
        .blobs()
        .take(items[0]["blob"].as_u64().unwrap())
        .unwrap();
    assert_eq!(&*bytes, "😀b".as_bytes());
    // A copy changes nothing; a cut deletes the selection after its bytes are stored.
    assert_eq!(answers.len(), 1);
    let answers = send(
        &mut engine,
        &request(
            7,
            "clipboard.copy",
            json!({ "doc_id": doc_id, "cut": true }),
        ),
    );
    let items = ok_payload(&answers, 7)["items"].as_array().unwrap().clone();
    let bytes = engine
        .blobs()
        .take(items[0]["blob"].as_u64().unwrap())
        .unwrap();
    assert_eq!(&*bytes, "😀b".as_bytes());
    assert_eq!(events(&answers, "render.invalidate").len(), 1);
    assert_eq!(
        line_state(&mut engine, doc_id),
        ("a".to_owned(), 1, json!([]))
    );
    // Without an identifier, a cut is carried out but no blob is kept, since nobody could release it.
    send(
        &mut engine,
        &request(8, "input.text", json!({ "doc_id": doc_id, "text": "bc" })),
    );
    send(
        &mut engine,
        &request(
            9,
            "input.select",
            json!({ "doc_id": doc_id, "anchor": at(0), "focus": at(3) }),
        ),
    );
    let blobs_before = engine.blobs().len();
    let answers = send(
        &mut engine,
        &notice("clipboard.copy", json!({ "doc_id": doc_id, "cut": true })),
    );
    assert!(events(&answers, "engine.error").is_empty());
    assert_eq!(engine.blobs().len(), blobs_before);
    assert_eq!(line_state(&mut engine, doc_id).0, "");
}

#[test]
fn the_tree_carries_language_direction_lines_runs_and_the_selection() {
    let (mut engine, doc_id) = typed("hello");
    let answers = send(
        &mut engine,
        &request(4, "query.a11y", json!({ "doc_id": doc_id })),
    );
    let tree = ok_payload(&answers, 4);
    assert_eq!(tree["selection"], json!([]));
    let root = &tree["root"];
    assert_eq!((&root["lang"], &root["dir"]), (&json!("en"), &json!("ltr")));
    let children = root["children"].as_array().unwrap();
    let arabic = children
        .iter()
        .find(|node| node["id"] == json!(ARABIC))
        .unwrap();
    assert_eq!(
        (&arabic["lang"], &arabic["dir"]),
        (&json!("ar"), &json!("rtl"))
    );
    assert_eq!(
        arabic["runs"],
        json!([{ "start": 52, "end": 64, "lang": "en", "dir": "ltr" }])
    );
    assert_eq!(arabic["line_starts"], json!([0, 84]));
    // Every other node inherits the document's language and direction; every node with text has its lines.
    for node in children.iter().filter(|node| node["id"] != json!(ARABIC)) {
        assert!(
            node.get("lang").is_none() && node.get("dir").is_none(),
            "{}",
            node["id"]
        );
        if node.get("text").is_some() {
            assert_eq!(node["line_starts"][0], json!(0), "{}", node["id"]);
        }
    }
    let editable = children
        .iter()
        .find(|node| node["id"] == json!(EDITABLE))
        .unwrap();
    assert_eq!(editable["line_starts"], json!([0]));
    // The selection appears in replies to query.a11y as in a11y.update.
    send(
        &mut engine,
        &request(
            5,
            "input.select",
            json!({ "doc_id": doc_id, "anchor": at(0), "focus": at(5) }),
        ),
    );
    let answers = send(
        &mut engine,
        &request(6, "query.a11y", json!({ "doc_id": doc_id })),
    );
    assert_eq!(
        ok_payload(&answers, 6)["selection"],
        json!([{ "anchor": at(0), "focus": at(5) }])
    );
}
