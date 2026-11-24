//! The conformance vectors of `crates/bayan-engine/protocol/conformance/` (spec §15, CORE-013): every `engine` and `binding` case is checked against the code, and every expected result of a `shell` case against a reference implementation written here, independently of the code under test, so that a wrong expectation fails in bayan-core before it reaches a shell.
//!
//! The files are compiled in (`DATA_FILES` in `xtask/src/verify.rs`), so these tests read no files and also run in WebAssembly, except those that start the native engine thread.

use std::collections::BTreeSet;

use bayan_engine::a11y::{self, Limits, MAX_DEPTH, MAX_NODES};
use bayan_engine::keys::{self, Meaning, Press};
use bayan_engine::limits::MAX_MESSAGE_BYTES;
use bayan_engine::protocol::{A11yCaret, A11yNode, A11yRole, ChildRange, Platform, Runtime};
use serde_json::{Value, json};

use crate::support::schema_validator::Validator;
use crate::support::{engine, ok_payload, request, send};

const SCHEMA: &str = include_str!("../../protocol/conformance/conformance.schema.json");
const RESTART: &str = include_str!("../../protocol/conformance/restart.json");
const TILE_GRID: &str = include_str!("../../protocol/conformance/tile-grid.json");
const A11Y_LIMITS: &str = include_str!("../../protocol/conformance/a11y-limits.json");
const KEYS: &str = include_str!("../../protocol/conformance/keys.json");

/// The four files of the first set, by area.
const FILES: [(&str, &str); 4] = [
    ("restart", RESTART),
    ("tile-grid", TILE_GRID),
    ("a11y-limits", A11Y_LIMITS),
    ("keys", KEYS),
];

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// The cases of a file that `who` must pass.
fn cases_for(file: &Value, who: &str) -> Vec<Value> {
    file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| case["for"].as_array().unwrap().contains(&json!(who)))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The format (spec §15)
// ---------------------------------------------------------------------------------------------

#[test]
fn every_file_has_the_format_of_spec_15_and_follows_its_schema() {
    let schema = parse(SCHEMA);
    let validator = Validator::new(&schema);
    for (area, text) in FILES {
        let file = parse(text);
        validator
            .validate(&file)
            .unwrap_or_else(|error| panic!("{area}.json: {error}"));
        assert_eq!(file["area"], json!(area));
        assert_eq!(file["protocol_version"], json!(1));
        let mut ids = BTreeSet::new();
        for case in file["cases"].as_array().unwrap() {
            let id = case["id"].as_str().unwrap();
            let words_joined_by_hyphens = !id.is_empty()
                && id.split('-').all(|word| {
                    !word.is_empty()
                        && word
                            .bytes()
                            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                });
            assert!(words_joined_by_hyphens, "{area}.json: {id}");
            assert!(ids.insert(id.to_owned()), "{area}.json: {id} twice");
            assert!(!case["for"].as_array().unwrap().is_empty(), "{id}");
        }
        for removed in file["removed"].as_array().unwrap() {
            assert!(
                !ids.contains(removed.as_str().unwrap()),
                "{removed} is reused"
            );
        }
    }
}

#[test]
fn the_schema_refuses_a_case_it_does_not_know() {
    let schema = parse(SCHEMA);
    let validator = Validator::new(&schema);
    let mut file = parse(TILE_GRID);
    file["cases"][0]["for"] = json!(["printer"]);
    assert!(validator.validate(&file).is_err());
    let mut file = parse(KEYS);
    file["cases"][0]["unknown_input"] = json!(1);
    assert!(validator.validate(&file).is_err());
}

// ---------------------------------------------------------------------------------------------
// keys.json (spec §14.3)
// ---------------------------------------------------------------------------------------------

fn platform_of(name: &str) -> Platform {
    serde_json::from_value(json!(name)).unwrap()
}

fn runtime_of(name: &str) -> Runtime {
    serde_json::from_value(json!(name)).unwrap()
}

#[test]
fn the_engines_manifest_has_the_key_data_of_every_platform_and_runtime() {
    let file = parse(KEYS);
    let manifests: Vec<Value> = cases_for(&file, "engine")
        .into_iter()
        .filter(|case| case.get("platform").is_some())
        .collect();
    assert_eq!(manifests.len(), 7 * 2);
    for case in manifests {
        let mut engine = engine();
        let shell = json!({ "name": "conformance", "version": "1", "platform": case["platform"], "runtime": case["runtime"] });
        send(
            &mut engine,
            &request(
                1,
                "hello",
                json!({ "protocol_versions": [1], "shell": shell }),
            ),
        );
        let answers = send(&mut engine, &request(2, "ui.manifest", json!({})));
        let manifest = ok_payload(&answers, 2);
        assert_eq!(
            manifest["commands"], case["expect"]["commands"],
            "{}",
            case["id"]
        );
        assert_eq!(manifest["keys"], case["expect"]["keys"], "{}", case["id"]);
    }
}

/// A press of a case as the engine's matcher takes it.
fn press_of(press: &Value) -> Press<'_> {
    Press {
        key: press["key"].as_str().unwrap(),
        code: press["code"].as_str().unwrap(),
        ctrl: press["ctrl"].as_bool().unwrap(),
        shift: press["shift"].as_bool().unwrap(),
        alt: press["alt"].as_bool().unwrap(),
        meta: press["meta"].as_bool().unwrap(),
    }
}

#[test]
fn the_engine_runs_a_command_for_exactly_the_presses_a_shell_sends() {
    let file = parse(KEYS);
    for case in cases_for(&file, "engine")
        .into_iter()
        .filter(|case| case.get("press").is_some())
    {
        let manifest = manifest_case(&file, &case);
        let platform = platform_of(manifest["platform"].as_str().unwrap());
        let runtime = runtime_of(manifest["runtime"].as_str().unwrap());
        let press = &case["press"];
        let meaning = keys::meaning(
            &press_of(press),
            press["composing"].as_bool().unwrap(),
            platform,
            runtime,
        );
        let runs = match meaning {
            Meaning::Command(id) => json!({ "command": id }),
            Meaning::DocumentKey(key) => json!({ "document_key": key }),
            Meaning::Nothing => Value::Null,
        };
        assert_eq!(runs, case["expect"]["runs"], "{}", case["id"]);
        assert_eq!(
            case["expect"]["send"],
            json!(!runs.is_null()),
            "{}",
            case["id"]
        );
    }
}

/// The manifest case that a press case names.
fn manifest_case(file: &Value, case: &Value) -> Value {
    let name = case["manifest"].as_str().unwrap();
    file["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == json!(name))
        .unwrap_or_else(|| panic!("no manifest case {name}"))
        .clone()
}

/// The reference for shells (spec §14.3 item 3), written apart from the engine's matcher: whether a press matches an entry of the manifest's key data.
fn reference_matches(entry: &Value, press: &Value) -> bool {
    for modifier in ["ctrl", "shift", "alt", "meta"] {
        if entry
            .get(modifier)
            .is_some_and(|given| given != &press[modifier])
        {
            return false;
        }
    }
    if let Some(code) = entry.get("code") {
        return code == &press["code"];
    }
    let wanted = entry["key"].as_str().unwrap();
    let typed = press["key"].as_str().unwrap();
    let fold = |text: &str| -> String {
        text.chars()
            .map(|character| {
                if character.is_ascii_uppercase() {
                    character.to_ascii_lowercase()
                } else {
                    character
                }
            })
            .collect()
    };
    if fold(wanted) == fold(typed) {
        return true;
    }
    let single_letter_or_digit = |text: &str| {
        let mut characters = text.chars();
        matches!((characters.next(), characters.next()), (Some(character), None) if character.is_ascii_alphanumeric())
    };
    let altgr = press["ctrl"] == json!(true) && press["alt"] == json!(true);
    if !single_letter_or_digit(wanted) || single_letter_or_digit(typed) || altgr {
        return false;
    }
    let code = press["code"].as_str().unwrap();
    let named = match (code.strip_prefix("Key"), code.strip_prefix("Digit")) {
        (Some(letter), _)
            if letter.len() == 1 && letter.chars().all(|c| c.is_ascii_uppercase()) =>
        {
            Some(letter.to_ascii_lowercase())
        }
        (_, Some(digit)) if digit.len() == 1 && digit.chars().all(|c| c.is_ascii_digit()) => {
            Some(digit.to_owned())
        }
        _ => None,
    };
    named.is_some_and(|named| named == fold(wanted))
}

/// The reference for shells (spec §14.3 item 2): whether a shell sends a press as `input.key`.
fn reference_sends(manifest: &Value, press: &Value) -> bool {
    if press["composing"] == json!(true) {
        return false;
    }
    let keys = &manifest["expect"]["keys"];
    if keys["reserved"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| reference_matches(entry, press))
    {
        return false;
    }
    let bound = manifest["expect"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|command| command.get("shell_action").is_none())
        .flat_map(|command| command["bindings"].as_array().unwrap().iter())
        .any(|binding| reference_matches(binding, press));
    bound
        || keys["document"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| reference_matches(entry, press))
}

#[test]
fn a_reference_shell_sends_exactly_the_presses_that_the_shell_cases_name() {
    let file = parse(KEYS);
    let cases = cases_for(&file, "shell");
    assert!(cases.len() >= 30);
    for case in cases {
        let manifest = manifest_case(&file, &case);
        assert_eq!(
            json!(reference_sends(&manifest, &case["press"])),
            case["expect"]["send"],
            "{}",
            case["id"]
        );
    }
}

#[test]
fn the_reserved_keys_follow_the_rows_of_the_specification() {
    // A few independent counts of spec §14.3's table: every shell keeps Tab, F6, F10, ContextMenu and Mod+C, X and V; Windows and Linux add Alt alone, 36 KeyTips and three clipboard keys; macOS adds Command+H, M and Q; a browser adds 4 zoom keys, 4 function keys, the developer tools (3 on Windows, Linux and macOS), Alt+D on Windows and Linux, 6 Mod+N/T/W keys, 9 Mod+digit keys, Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+PageUp and Ctrl+PageDown, and the browser's history on Windows and Linux.
    let file = parse(KEYS);
    let count = |platform: &str, runtime: &str| {
        cases_for(&file, "engine")
            .into_iter()
            .find(|case| case["platform"] == json!(platform) && case["runtime"] == json!(runtime))
            .unwrap()["expect"]["keys"]["reserved"]
            .as_array()
            .unwrap()
            .len()
    };
    let every_shell = 4 + 3;
    let browser = 4 + 4 + 6 + 9 + 4;
    assert_eq!(count("android", "native"), every_shell);
    assert_eq!(count("windows", "native"), every_shell + 1 + 36 + 3);
    assert_eq!(count("macos", "native"), every_shell + 3);
    assert_eq!(count("ios", "browser"), every_shell + browser);
    assert_eq!(count("macos", "browser"), every_shell + 3 + browser + 3);
    assert_eq!(
        count("linux", "browser"),
        every_shell + 1 + 36 + 3 + browser + 3 + 1 + 2
    );
}

// ---------------------------------------------------------------------------------------------
// tile-grid.json (spec §14.6 item 3)
// ---------------------------------------------------------------------------------------------

const BLU_PER_DIP: i128 = 19_050;

/// A positive, finite double as an exact fraction `numerator / 2^shift`.
fn exact(value: f64) -> (i128, u32) {
    let bits = value.to_bits();
    let exponent = i32::try_from((bits >> 52) & 0x7ff).unwrap();
    let fraction = i128::from(bits & ((1 << 52) - 1));
    assert!(
        exponent > 0 && exponent < 0x7ff && bits >> 63 == 0,
        "{value}"
    );
    let mantissa = fraction | (1 << 52);
    let power = exponent - 1075;
    if power >= 0 {
        (mantissa << power, 0)
    } else {
        (mantissa, u32::try_from(-power).unwrap())
    }
}

/// The smallest integer not below `numerator / denominator`, for a positive denominator.
fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    if numerator >= 0 {
        (numerator + denominator - 1) / denominator
    } else {
        -((-numerator) / denominator)
    }
}

/// `numerator / denominator` rounded to the nearest integer, halves away from zero, for a positive denominator.
fn round_div(numerator: i128, denominator: i128) -> i128 {
    let away = (2 * numerator.abs() + denominator) / (2 * denominator);
    if numerator < 0 { -away } else { away }
}

/// The page's size in device pixels with exact arithmetic on the exact value of `s`: `max(1, ⌈side × s / 19,050 − 10⁻⁶⌉)`.
fn exact_page_pixels(side: i128, s: (i128, u32)) -> i128 {
    let (numerator, shift) = s;
    let scale = 1_i128 << shift;
    let pixels = ceil_div(
        side * numerator * 1_000_000 - BLU_PER_DIP * scale,
        BLU_PER_DIP * 1_000_000 * scale,
    );
    if pixels < 1 { 1 } else { pixels }
}

/// The BLU coordinate where pixel column `a` starts, exactly: `a × 19,050 / s` rounded halves away from zero.
fn exact_blu(a: i128, s: (i128, u32)) -> i128 {
    let (numerator, shift) = s;
    round_div(a * BLU_PER_DIP * (1_i128 << shift), numerator)
}

/// The same two formulas in IEEE 754 double precision, in the spec's order, as the shells compute them.
fn double_page_pixels(side: f64, s: f64) -> f64 {
    let pixels = (side * s / 19_050.0 - 1e-6).ceil();
    if pixels < 1.0 { 1.0 } else { pixels }
}

fn double_blu(a: f64, s: f64) -> f64 {
    (a * 19_050.0 / s).round()
}

#[test]
fn a_reference_tile_grid_gives_every_expected_size_and_rectangle() {
    let file = parse(TILE_GRID);
    let cases = cases_for(&file, "shell");
    assert!(cases.len() >= 10);
    for case in cases {
        let id = &case["id"];
        let zoom = case["zoom"].as_f64().unwrap();
        let device_scale = case["device_scale"].as_f64().unwrap();
        let s = zoom * device_scale;
        let tile = i128::from(case["tile_size"].as_i64().unwrap());
        let mut pixels = [0_i128; 2];
        for (index, side) in ["width", "height"].into_iter().enumerate() {
            let blu = case["page"][side].as_i64().unwrap();
            let exact_value = exact_page_pixels(i128::from(blu), exact(s));
            #[expect(
                clippy::cast_precision_loss,
                reason = "page sizes in BLU are far below 2^53, which a double holds exactly"
            )]
            let double_value = double_page_pixels(blu as f64, s);
            #[expect(
                clippy::cast_precision_loss,
                reason = "the exact pixel count is at most a few million"
            )]
            let exact_as_double = exact_value as f64;
            assert_eq!(double_value, exact_as_double, "{id} {side}");
            assert_eq!(
                case["expect"]["pixels"][side],
                json!(exact_value),
                "{id} {side}"
            );
            pixels[index] = exact_value;
        }
        let columns = (pixels[0] + tile - 1) / tile;
        let rows = (pixels[1] + tile - 1) / tile;
        assert_eq!(case["expect"]["columns"], json!(columns), "{id}");
        assert_eq!(case["expect"]["rows"], json!(rows), "{id}");
        for chosen in case["expect"]["tiles"].as_array().unwrap() {
            let column = i128::from(chosen["column"].as_i64().unwrap());
            let row = i128::from(chosen["row"].as_i64().unwrap());
            let (x, y) = (column * tile, row * tile);
            let (width, height) = (tile.min(pixels[0] - x), tile.min(pixels[1] - y));
            assert_eq!(
                chosen["pixels"],
                json!({ "x": x, "y": y, "width": width, "height": height }),
                "{id}"
            );
            let span = |start: i128, end: i128| {
                let (from, to) = (exact_blu(start, exact(s)), exact_blu(end, exact(s)));
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "pixel coordinates are at most a few million"
                )]
                let doubles = (double_blu(start as f64, s), double_blu(end as f64, s));
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "BLU coordinates of a page are far below 2^53"
                )]
                let exact_doubles = (from as f64, to as f64);
                assert_eq!(doubles, exact_doubles, "{id}");
                (from, (to - from).max(1))
            };
            let (blu_x, blu_width) = span(x, x + width);
            let (blu_y, blu_height) = span(y, y + height);
            assert_eq!(
                chosen["blu"],
                json!({ "x": blu_x, "y": blu_y, "width": blu_width, "height": blu_height }),
                "{id}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// a11y-limits.json (spec §14.7 item 1)
// ---------------------------------------------------------------------------------------------

fn paragraph(id: u64, text: String) -> A11yNode {
    A11yNode {
        id,
        role: A11yRole::Paragraph,
        level: None,
        name: None,
        text: Some(text),
        editable: false,
        bounds: None,
        chars: None,
        child_count: None,
        children_start: None,
        children: None,
    }
}

/// The tree that a case's generator describes (spec §15): see the generator's description in `conformance.schema.json`.
fn generate(tree: &Value) -> A11yNode {
    let number = |name: &str| tree.get(name).and_then(Value::as_u64);
    let mut document = paragraph(1, String::new());
    document.role = A11yRole::Document;
    document.text = None;
    match tree["generator"].as_str().unwrap() {
        "chain" => {
            let depth = number("depth").unwrap();
            let mut below: Option<A11yNode> = None;
            for level in (2..=depth).rev() {
                let mut node = paragraph(level, level.to_string());
                node.children = below.take().map(|child| vec![child]);
                below = Some(node);
            }
            document.children = below.map(|child| vec![child]);
        }
        "wide" => {
            let held = number("children").unwrap();
            let start = number("children_start").unwrap_or(0);
            let text_units = number("text_units");
            let children: Vec<A11yNode> = (start..start + held)
                .map(|index| {
                    let text = text_units.map_or_else(
                        || index.to_string(),
                        |units| "x".repeat(usize::try_from(units).unwrap()),
                    );
                    paragraph(index + 2, text)
                })
                .collect();
            if let Some(count) = number("child_count") {
                document.child_count = Some(u32::try_from(count).unwrap());
                if held > 0 {
                    document.children_start = Some(u32::try_from(start).unwrap());
                }
            }
            if held > 0 || number("child_count").is_none() {
                document.children = Some(children);
            }
        }
        other => panic!("unknown generator {other}"),
    }
    document
}

/// The reference for shells (spec §14.7): whether a tree a shell receives is within the limits and consistent, and the ranges of children it lacks, written apart from the engine, on the tree as JSON.
fn reference_check(tree: &Value, root_level: u64) -> (bool, Vec<Value>) {
    let mut nodes = 0_u64;
    let mut deepest = 0_u64;
    let mut consistent = true;
    let mut queries = Vec::new();
    // Depth first, in reading order, with a stack: (node, level).
    let mut pending = vec![(tree, root_level)];
    while let Some((node, level)) = pending.pop() {
        nodes += 1;
        deepest = deepest.max(level);
        let held: Vec<&Value> = node
            .get("children")
            .and_then(Value::as_array)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        let held_count = u64::try_from(held.len()).unwrap();
        if let Some(count) = node.get("child_count").and_then(Value::as_u64) {
            let start = node
                .get("children_start")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if held_count > 0 && start + held_count > count {
                consistent = false;
            }
            let id = node["id"].clone();
            if held_count == 0 {
                queries.push(json!({ "node": id, "children": { "start": 0, "count": count } }));
            } else {
                if start > 0 {
                    queries.push(json!({ "node": id, "children": { "start": 0, "count": start } }));
                }
                let end = start + held_count;
                if end < count {
                    queries.push(
                        json!({ "node": id, "children": { "start": end, "count": count - end } }),
                    );
                }
            }
        }
        for child in held.into_iter().rev() {
            pending.push((child, level + 1));
        }
    }
    let within = deepest <= u64::from(MAX_DEPTH) && nodes <= u64::try_from(MAX_NODES).unwrap();
    let accept = within && consistent;
    (accept, if accept { queries } else { Vec::new() })
}

#[test]
fn a_reference_shell_accepts_and_loads_trees_as_the_shell_cases_say() {
    let file = parse(A11Y_LIMITS);
    let cases = cases_for(&file, "shell");
    assert!(cases.len() >= 10);
    for case in cases {
        let tree = serde_json::to_value(generate(&case["tree"])).unwrap();
        let (accept, queries) = reference_check(&tree, case["root_level"].as_u64().unwrap());
        assert_eq!(json!(accept), case["expect"]["accept"], "{}", case["id"]);
        assert_eq!(json!(queries), case["expect"]["queries"], "{}", case["id"]);
    }
}

/// The depth and the number of nodes of a tree.
fn measure(node: &A11yNode) -> (u64, u64) {
    let children = node.children.as_deref().unwrap_or_default();
    children
        .iter()
        .map(measure)
        .fold((1, 1), |(depth, nodes), (child_depth, child_nodes)| {
            (depth.max(child_depth + 1), nodes + child_nodes)
        })
}

/// A reply to `query.a11y` with a tree, as JSON, to measure it against the message limit.
fn reply_size(root: &A11yNode, caret: Option<A11yCaret>) -> usize {
    let reply = json!({ "v": 1, "re": 9_007_199_254_740_991_u64, "ok": true, "payload": { "doc_id": 9_007_199_254_740_991_u64, "root": root, "caret": caret } });
    serde_json::to_string(&reply).unwrap().len()
}

#[test]
fn the_engine_keeps_the_trees_of_the_engine_cases_within_the_limits() {
    let file = parse(A11Y_LIMITS);
    let cases = cases_for(&file, "engine");
    assert!(cases.len() >= 5);
    for case in cases {
        let id = &case["id"];
        let expect = &case["expect"];
        let mut tree = generate(&case["tree"]);
        let mut caret: Option<A11yCaret> = case
            .get("caret")
            .map(|caret| serde_json::from_value(caret.clone()).unwrap());
        a11y::flatten(&mut tree, &mut caret);
        let range: Option<ChildRange> = case
            .get("query")
            .map(|query| serde_json::from_value(query["children"].clone()).unwrap());
        let first = a11y::fill(&tree, caret, range, &Limits::PROTOCOL);
        assert!(reply_size(&first, caret) <= MAX_MESSAGE_BYTES, "{id}");
        if let Some(depth) = expect.get("depth") {
            let (measured_depth, nodes) = measure(&first);
            assert_eq!(json!(measured_depth), *depth, "{id}");
            assert_eq!(json!(nodes), expect["nodes"], "{id}");
        }
        if let Some(texts) = expect.get("texts").and_then(Value::as_object) {
            for (node, text) in texts {
                let found = a11y::find(&first, node.parse().unwrap()).unwrap();
                assert_eq!(json!(found.text), *text, "{id} {node}");
            }
        }
        if let Some(expected) = expect.get("caret") {
            assert_eq!(json!(caret), *expected, "{id}");
        }
        if let Some(expected) = expect.get("first") {
            let held = first.children.as_ref().map_or(0, Vec::len);
            assert_eq!(json!(held), expected["held"], "{id}");
            assert_eq!(
                json!(first.child_count),
                expected.get("child_count").cloned().unwrap_or(Value::Null),
                "{id}"
            );
            assert_eq!(
                json!(first.children_start),
                expected
                    .get("children_start")
                    .cloned()
                    .unwrap_or(Value::Null),
                "{id}"
            );
        }
        for load in expect
            .get("loads")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let range: ChildRange =
                serde_json::from_value(json!({ "start": load["start"], "count": load["count"] }))
                    .unwrap();
            let answer = a11y::fill(&tree, None, Some(range), &Limits::PROTOCOL);
            assert!(reply_size(&answer, caret) <= MAX_MESSAGE_BYTES, "{id}");
            assert_eq!(json!(answer.children_start), load["children_start"], "{id}");
            assert_eq!(
                json!(answer.children.as_ref().map_or(0, Vec::len)),
                load["held"],
                "{id}"
            );
        }
        if expect.get("loads_completely") == Some(&json!(true)) {
            loads_completely(&tree, caret, id);
        }
    }
}

/// Loads every child of the root of `tree` the way a shell does, from the first reply and then with ranges, and checks that each answer fits the message limit and holds at least one child, and that every child arrives once, in order.
fn loads_completely(tree: &A11yNode, caret: Option<A11yCaret>, id: &Value) {
    let total = tree.children.as_ref().map_or(0, Vec::len);
    let mut have = vec![None; total];
    let mut answer = a11y::fill(tree, caret, None, &Limits::PROTOCOL);
    loop {
        assert!(reply_size(&answer, caret) <= MAX_MESSAGE_BYTES, "{id}");
        let start = answer
            .children_start
            .map_or(0, |start| usize::try_from(start).unwrap());
        for (offset, child) in answer.children.iter().flatten().enumerate() {
            assert!(
                have[start + offset].replace(child.id).is_none(),
                "{id}: child {} twice",
                start + offset
            );
        }
        let Some(missing) = have.iter().position(Option::is_none) else {
            break;
        };
        let range = ChildRange {
            start: u32::try_from(missing).unwrap(),
            count: u32::try_from(total - missing).unwrap(),
        };
        answer = a11y::fill(tree, None, Some(range), &Limits::PROTOCOL);
        assert!(
            answer
                .children
                .as_ref()
                .is_some_and(|children| !children.is_empty()),
            "{id}"
        );
    }
    let ids: Vec<u64> = have.into_iter().map(Option::unwrap).collect();
    let expected: Vec<u64> = tree
        .children
        .iter()
        .flatten()
        .map(|child| child.id)
        .collect();
    assert_eq!(ids, expected, "{id}");
}

// ---------------------------------------------------------------------------------------------
// restart.json (spec §12.1, §14.2)
// ---------------------------------------------------------------------------------------------

/// What the reference shell does with a message (spec §12.1 steps 1 to 4, §14.1 item 5, §14.2 items 2 and 3).
#[derive(Default)]
struct ReferenceShell {
    /// The identifiers of the requests sent since the current session's hello, which is among them.
    current: BTreeSet<u64>,
    /// The identifier of the current session's hello, and whether its welcome has arrived.
    hello: Option<(u64, bool)>,
    /// The documents opened in the current session.
    documents: BTreeSet<u64>,
}

impl ReferenceShell {
    fn sends(&mut self, message: &Value) {
        let id = message.get("id").and_then(Value::as_u64);
        if message["type"] == json!("hello") {
            self.current.clear();
            self.documents.clear();
            self.hello = id.map(|id| (id, false));
        }
        if let Some(id) = id {
            self.current.insert(id);
        }
    }

    fn receives(&mut self, message: &Value) -> Value {
        if let Some(re) = message.get("re").and_then(Value::as_u64) {
            // The panic reply to the message that failed comes just before the engine.error, and counts as no further error.
            if message["ok"] == json!(false) && message["error"]["code"] == json!("panic") {
                return json!("ignore");
            }
            if !self.current.contains(&re) {
                return json!("ignore");
            }
            if let Some((hello, welcomed)) = &mut self.hello
                && *hello == re
            {
                *welcomed = message["ok"] == json!(true);
            }
            if message["type"] == json!("doc.opened") {
                self.documents
                    .insert(message["payload"]["doc_id"].as_u64().unwrap());
            }
            return json!("handle");
        }
        let payload = &message["payload"];
        if message["type"] == json!("engine.error") && payload["recoverable"] != json!(true) {
            return match payload.get("restart") {
                Some(Value::Object(restart)) => json!({ "restart": restart }),
                // null stops; a missing restart breaks the protocol, which stops too.
                _ => json!("stop"),
            };
        }
        let welcomed = self.hello.is_some_and(|(_, welcomed)| welcomed);
        let about_an_old_document = payload
            .get("doc_id")
            .and_then(Value::as_u64)
            .is_some_and(|doc_id| !self.documents.contains(&doc_id));
        if !welcomed || about_an_old_document {
            json!("ignore")
        } else {
            json!("handle")
        }
    }
}

#[test]
fn a_reference_shell_does_what_the_shell_cases_say() {
    let file = parse(RESTART);
    let mut cases = cases_for(&file, "shell");
    cases.extend(cases_for(&file, "web"));
    cases.extend(cases_for(&file, "desktop"));
    assert!(cases.len() >= 5);
    for case in cases {
        let mut shell = ReferenceShell::default();
        for step in case["steps"].as_array().unwrap() {
            if let Some(message) = step.get("shell_sends") {
                shell.sends(message);
            } else if let Some(message) = step.get("engine_sends") {
                assert_eq!(
                    shell.receives(message),
                    step["expect"],
                    "{}: {message}",
                    case["id"]
                );
            } else {
                let control = &step["worker_sends"];
                assert_eq!(control["worker"], json!("failed"), "{}", case["id"]);
                assert_eq!(step["expect"], json!("stop"), "{}", case["id"]);
            }
        }
    }
}

#[test]
fn the_binding_cases_follow_one_rule() {
    // An independent statement of spec §14.2 items 1 and 2: restart n of 3 while fewer than three restarts are less than 600,000 ms old.
    let file = parse(RESTART);
    for case in cases_for(&file, "binding")
        .into_iter()
        .filter(|case| case.get("failures_ms").is_some())
    {
        let mut granted: Vec<u64> = Vec::new();
        let mut expected = Vec::new();
        for failure in case["failures_ms"].as_array().unwrap() {
            let now = failure.as_u64().unwrap();
            granted.retain(|&time| now - time < 600_000);
            if granted.len() < 3 {
                granted.push(now);
                expected.push(json!({ "number": granted.len(), "limit": 3 }));
            } else {
                expected.push(Value::Null);
            }
        }
        assert_eq!(
            json!(expected),
            case["expect"]["restarts"],
            "{}",
            case["id"]
        );
    }
}

// Host only: these start the native engine thread, and WebAssembly (wasm32-wasip1) has no threads.
#[cfg(not(target_arch = "wasm32"))]
mod native_binding {
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use bayan_engine::blobs::BlobStore;
    use bayan_engine::config::Config;
    use bayan_engine::thread::EngineThread;
    use serde_json::{Value, json};

    use super::{RESTART, cases_for, parse};

    const WAIT: Duration = Duration::from_secs(20);

    fn started(config: &Value) -> (EngineThread, Receiver<Value>) {
        let config = Config::from_json(&serde_json::to_vec(config).unwrap()).unwrap();
        let thread = EngineThread::spawn(config, Arc::new(BlobStore::new())).unwrap();
        let (sender, receiver) = mpsc::channel();
        let sender = Mutex::new(sender);
        thread
            .set_sink(Arc::new(move |message: &str| {
                let _ignored = sender
                    .lock()
                    .unwrap()
                    .send(serde_json::from_str(message).unwrap());
            }))
            .unwrap();
        (thread, receiver)
    }

    /// The next message that satisfies `found`.
    fn next(messages: &Receiver<Value>, found: impl Fn(&Value) -> bool) -> Value {
        loop {
            let message = messages.recv_timeout(WAIT).unwrap();
            if found(&message) {
                return message;
            }
        }
    }

    fn discards(message: &Value) -> bool {
        message["type"] == json!("engine.error")
            && message["payload"]["recoverable"] == json!(false)
    }

    #[test]
    fn every_failure_case_passes_against_the_native_binding() {
        let file = parse(RESTART);
        for case in cases_for(&file, "binding")
            .into_iter()
            .filter(|case| case.get("failures_ms").is_some())
        {
            let (thread, messages) = started(&json!({ "test": { "allow_panic": true } }));
            let mut restarts = Vec::new();
            for (index, failure) in case["failures_ms"].as_array().unwrap().iter().enumerate() {
                let at = failure.as_u64().unwrap();
                let id = u64::try_from(index).unwrap() * 2 + 1;
                let hello = json!({ "v": 1, "id": id, "type": "hello", "payload": { "protocol_versions": [1] } });
                thread
                    .post(serde_json::to_vec(&hello).unwrap(), at)
                    .unwrap();
                let panic = json!({ "v": 1, "id": id + 1, "type": "diag.panic" });
                thread
                    .post(serde_json::to_vec(&panic).unwrap(), at)
                    .unwrap();
                restarts.push(next(&messages, discards)["payload"]["restart"].clone());
            }
            assert_eq!(
                json!(restarts),
                case["expect"]["restarts"],
                "{}",
                case["id"]
            );
            let stopped = restarts.last().is_some_and(Value::is_null);
            assert_eq!(thread.has_stopped(), stopped, "{}", case["id"]);
            if stopped {
                assert!(thread.post(b"{}".to_vec(), 0).is_err());
            }
        }
    }

    #[test]
    fn the_recording_case_passes_against_the_native_binding() {
        let file = parse(RESTART);
        let case = cases_for(&file, "binding")
            .into_iter()
            .find(|case| case.get("messages").is_some())
            .unwrap();
        assert_eq!(case["runtime"], json!("native"));
        let (thread, messages) = started(&case["config"]);
        let mut restarts = Vec::new();
        let posted = case["messages"].as_array().unwrap();
        for entry in posted {
            thread
                .post(
                    serde_json::to_vec(&entry["message"]).unwrap(),
                    entry["at_ms"].as_u64().unwrap(),
                )
                .unwrap();
        }
        let stop_id = posted.last().unwrap()["message"]["id"].clone();
        let mut stopped = None;
        while stopped.is_none() {
            let message = messages.recv_timeout(WAIT).unwrap();
            if discards(&message) {
                restarts.push(message["payload"]["restart"].clone());
            } else if message["re"] == stop_id {
                stopped = Some(message["payload"].clone());
            }
        }
        let stopped = stopped.unwrap();
        assert_eq!(json!(restarts), case["expect"]["restarts"]);
        assert_eq!(stopped["entries"], case["expect"]["entries"]);
        let blob = stopped["blob"].as_u64().unwrap();
        for replay in 0..case["replays"].as_u64().unwrap() {
            let id = 1_000 + replay;
            let message =
                json!({ "v": 1, "id": id, "type": "diag.replay", "payload": { "blob": blob } });
            thread
                .post(serde_json::to_vec(&message).unwrap(), 100)
                .unwrap();
            let report = next(&messages, |message| message["re"] == json!(id));
            assert_eq!(
                report["payload"]["identical"], case["expect"]["identical"],
                "{report}"
            );
            assert_eq!(report["payload"]["entries"], case["expect"]["entries"]);
        }
        assert_eq!(json!(thread.has_stopped()), case["expect"]["stopped"]);
        thread.stop();
    }
}
