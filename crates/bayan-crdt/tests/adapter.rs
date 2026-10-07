//! Tests of the adapter's primitives, across replicas where it matters.

use std::collections::BTreeMap;

use bayan_crdt::{
    ATOM_KEY, CrdtError, Doc, Expand, ImportError, ImportLimits, MarkFamily, PeerId, UndoManager,
    Value, VersionVector,
};

const FAMILIES: [MarkFamily; 3] = [
    MarkFamily {
        name: "r",
        expand: Expand::After,
    },
    MarkFamily {
        name: "cmt",
        expand: Expand::None,
    },
    MarkFamily {
        name: "link",
        expand: Expand::None,
    },
];

fn doc(peer: u64) -> Doc {
    Doc::new(PeerId(peer), &FAMILIES).expect("a document with the test families")
}

/// Sends everything `from` has that `to` lacks.
fn sync(from: &Doc, to: &Doc) {
    let update = from
        .export_updates(&to.version_vector())
        .expect("exporting an update");
    to.import(&update, &ImportLimits::UPDATE)
        .expect("importing an update");
}

fn marks_at(doc: &Doc, pos: usize) -> BTreeMap<String, Value> {
    let mut offset = 0;
    for run in doc.main_story().runs() {
        let len = run.len();
        if pos < offset + len {
            return run.marks;
        }
        offset += len;
    }
    panic!("position {pos} is outside the story");
}

#[test]
fn an_atom_binds_exactly_its_placeholder_and_never_spreads() {
    let a = doc(1);
    let story = a.main_story();
    story.insert(0, "ab").unwrap();
    story.insert_atom(1, '\u{0D}', "P:0001").unwrap();
    a.commit();
    assert_eq!(story.text(), "a\u{0D}b");
    assert_eq!(marks_at(&a, 1).get(ATOM_KEY), Some(&Value::from("P:0001")));
    // Typing right before and right after the placeholder never inherits its binding.
    story.insert(2, "x").unwrap();
    story.insert(1, "y").unwrap();
    a.commit();
    assert_eq!(story.text(), "ay\u{0D}xb");
    for pos in [0, 1, 3, 4] {
        assert!(!marks_at(&a, pos).contains_key(ATOM_KEY), "position {pos}");
    }
    assert_eq!(marks_at(&a, 2).get(ATOM_KEY), Some(&Value::from("P:0001")));
    // Adjacent placeholders with different bindings stay separate runs.
    story.insert_atom(3, '\u{0D}', "P:0002").unwrap();
    a.commit();
    assert_eq!(marks_at(&a, 3).get(ATOM_KEY), Some(&Value::from("P:0002")));
}

#[test]
fn mark_families_expand_as_configured_including_dynamic_keys() {
    let a = doc(1);
    let story = a.main_story();
    story.insert(0, "0123456789").unwrap();
    story.mark(2..5, "r:b", &Value::Bool(true)).unwrap();
    story.mark(2..5, "cmt:00aa", &Value::Bool(true)).unwrap();
    story.mark(2..5, "cmt:00bb", &Value::Bool(true)).unwrap();
    story
        .mark(2..5, "link", &Value::from("https://example.org"))
        .unwrap();
    story.mark(2..5, "unknown:x", &Value::Bool(true)).unwrap();
    a.commit();
    // Typing at the end of the range: only the `r` family (After) grows.
    story.insert(5, "E").unwrap();
    // Typing at the start: nothing grows.
    story.insert(2, "S").unwrap();
    a.commit();
    let end = marks_at(&a, 6);
    assert_eq!(end.keys().collect::<Vec<_>>(), ["r:b"]);
    assert!(marks_at(&a, 2).is_empty());
    let inside = marks_at(&a, 4);
    assert_eq!(
        inside.keys().collect::<Vec<_>>(),
        ["cmt:00aa", "cmt:00bb", "link", "r:b", "unknown:x"]
    );
}

#[test]
fn rejects_family_names_the_library_cannot_configure() {
    for bad in ["", "r:b", "atom"] {
        let families = [MarkFamily {
            name: bad,
            expand: Expand::After,
        }];
        assert!(
            matches!(
                Doc::new(PeerId(1), &families),
                Err(CrdtError::InvalidFamily(_))
            ),
            "{bad:?}"
        );
    }
    let twice = [FAMILIES[0], FAMILIES[0]];
    assert!(Doc::new(PeerId(1), &twice).is_err());
}

#[test]
fn rejects_positions_outside_the_story() {
    let a = doc(1);
    let story = a.main_story();
    story.insert(0, "abc").unwrap();
    assert!(matches!(
        story.insert(4, "x"),
        Err(CrdtError::OutOfRange { pos: 4, len: 3 })
    ));
    assert!(story.delete(2..4).is_err());
    #[expect(
        clippy::reversed_empty_ranges,
        reason = "the test checks that a reversed range is refused"
    )]
    let reversed = 3..2;
    assert!(story.mark(reversed, "r:b", &Value::Bool(true)).is_err());
    assert!(story.runs_in(0..4).is_err());
    assert_eq!(story.runs_in(1..3).unwrap()[0].text, "bc");
}

#[test]
fn registries_and_id_lists_merge_across_replicas() {
    let a = doc(1);
    let b = doc(2);
    let rows = a
        .registry("tables")
        .create("t1")
        .unwrap()
        .id_list("rows")
        .unwrap();
    for (index, row) in ["r1", "r2", "r3", "r4"].iter().enumerate() {
        rows.insert(index, row).unwrap();
    }
    a.commit();
    sync(&a, &b);
    // Concurrent moves of the same row, a move racing a delete, and an insert.
    let rows_b = b
        .registry("tables")
        .get("t1")
        .unwrap()
        .get_id_list("rows")
        .unwrap();
    rows.move_item(0, 3).unwrap();
    let r2 = rows
        .items()
        .iter()
        .position(|item| item.as_str() == Some("r2"))
        .unwrap();
    rows.delete(r2).unwrap();
    a.commit();
    rows_b.move_item(0, 1).unwrap();
    rows_b.insert(4, "r5").unwrap();
    b.commit();
    sync(&a, &b);
    sync(&b, &a);
    let items_a = rows.items();
    assert_eq!(items_a, rows_b.items());
    let mut ids: Vec<&str> = items_a.iter().filter_map(Value::as_str).collect();
    ids.sort_unstable();
    // r2 was deleted; r1 was moved twice but exists once; r5 was added.
    assert_eq!(ids, ["r1", "r3", "r4", "r5"]);
}

#[test]
fn entities_created_concurrently_with_the_same_identifier_merge() {
    let a = doc(1);
    let b = doc(2);
    a.registry("paragraphs")
        .create("p")
        .unwrap()
        .set("style", &Value::from("Title"))
        .unwrap();
    a.commit();
    b.registry("paragraphs")
        .create("p")
        .unwrap()
        .set("jc", &Value::from("center"))
        .unwrap();
    b.commit();
    sync(&a, &b);
    sync(&b, &a);
    for replica in [&a, &b] {
        let entries = replica.registry("paragraphs").get("p").unwrap().entries();
        assert_eq!(entries.get("style"), Some(&Value::from("Title")));
        assert_eq!(entries.get("jc"), Some(&Value::from("center")));
    }
    // Stories too.
    a.stories().create("s").unwrap().insert(0, "A").unwrap();
    a.commit();
    b.stories().create("s").unwrap().insert(0, "B").unwrap();
    b.commit();
    sync(&a, &b);
    sync(&b, &a);
    let text = a.stories().get("s").unwrap().text();
    assert_eq!(text.len(), 2);
    assert_eq!(text, b.stories().get("s").unwrap().text());
}

#[test]
fn snapshots_shallow_snapshots_and_checkout() {
    let a = doc(1);
    let story = a.main_story();
    story.insert(0, "first").unwrap();
    a.commit();
    let first = a.version();
    story.insert(5, " second").unwrap();
    a.commit();
    let snapshot = a.export_snapshot().unwrap();
    let loaded = Doc::load(
        &snapshot,
        PeerId(9),
        &FAMILIES,
        &ImportLimits::LOCAL_SNAPSHOT,
    )
    .unwrap();
    assert_eq!(loaded.main_story().text(), "first second");
    // The version survives encoding, and checkout shows the old state.
    let first = bayan_crdt::Version::decode(&first.encode()).unwrap();
    loaded.checkout(&first).unwrap();
    assert!(loaded.is_detached());
    assert_eq!(loaded.main_story().text(), "first");
    loaded.checkout_latest();
    assert_eq!(loaded.main_story().text(), "first second");
    // A shallow snapshot keeps the state but not the trimmed history.
    let shallow = a.export_shallow_snapshot(&a.version()).unwrap();
    let trimmed = Doc::load(
        &shallow,
        PeerId(10),
        &FAMILIES,
        &ImportLimits::LOCAL_SNAPSHOT,
    )
    .unwrap();
    assert_eq!(trimmed.main_story().text(), "first second");
    assert!(trimmed.checkout(&first).is_err());
}

#[test]
fn updates_may_arrive_out_of_order() {
    let a = doc(1);
    let b = doc(2);
    let story = a.main_story();
    story.insert(0, "one").unwrap();
    a.commit();
    let first = a.export_updates(&VersionVector::new()).unwrap();
    let after_first = a.version_vector();
    story.insert(3, " two").unwrap();
    a.commit();
    let second = a.export_updates(&after_first).unwrap();
    let report = b.import(&second, &ImportLimits::UPDATE).unwrap();
    assert!(report.pending);
    assert_eq!(b.main_story().text(), "");
    let report = b.import(&first, &ImportLimits::UPDATE).unwrap();
    assert!(!report.pending);
    assert_eq!(b.main_story().text(), "one two");
}

#[test]
fn undo_reverts_only_local_changes() {
    let a = doc(1);
    let b = doc(2);
    let mut undo = UndoManager::new(&a);
    a.main_story().insert(0, "local").unwrap();
    a.commit();
    sync(&a, &b);
    b.main_story().insert(5, " remote").unwrap();
    b.commit();
    sync(&b, &a);
    assert_eq!(a.main_story().text(), "local remote");
    assert_eq!(undo.undo_count(), 1);
    assert!(undo.undo().unwrap());
    assert_eq!(a.main_story().text(), " remote");
    assert!(!undo.undo().unwrap());
    assert!(undo.redo().unwrap());
    assert_eq!(a.main_story().text(), "local remote");
}

#[test]
fn import_refuses_blobs_beyond_the_limits() {
    let a = doc(1);
    for index in 0..10 {
        a.main_story().insert(0, "x").unwrap();
        a.registry("paragraphs")
            .create(&format!("p{index}"))
            .unwrap();
        a.commit();
    }
    let updates = a.export_updates(&VersionVector::new()).unwrap();
    let snapshot = a.export_snapshot().unwrap();
    let b = doc(2);
    let limits = ImportLimits::UPDATE;
    assert!(matches!(
        b.import(
            &updates,
            &ImportLimits {
                max_bytes: 10,
                ..limits
            }
        ),
        Err(ImportError::TooLarge { .. })
    ));
    assert!(matches!(
        b.import(
            &updates,
            &ImportLimits {
                max_ops: 5,
                ..limits
            }
        ),
        Err(ImportError::TooManyOps { .. })
    ));
    assert_eq!(
        b.import(&snapshot, &limits),
        Err(ImportError::SnapshotNotAccepted)
    );
    let mut corrupted = updates.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0x55;
    assert!(matches!(
        b.import(&corrupted, &limits),
        Err(ImportError::Malformed(_))
    ));
    assert!(matches!(
        b.import(b"not a loro blob at all, just text", &limits),
        Err(ImportError::Malformed(_))
    ));
    // Nothing above was applied; the real update still is.
    assert_eq!(b.main_story().text(), "");
    b.import(&updates, &limits).unwrap();
    assert_eq!(b.main_story().text(), "xxxxxxxxxx");
}

/// An update whose map entry is a value nested `depth` levels deep, written with the CRDT library directly, as a hostile replica could. Built on a thread with a large stack, and leaked, because the library both writes and frees such values recursively.
fn hostile_update(depth: usize, overwrite: bool) -> Vec<u8> {
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let mut value = loro::LoroValue::I64(1);
            for _ in 0..depth {
                value = loro::LoroValue::List(vec![value].into());
            }
            let hostile = loro::LoroDoc::new();
            hostile.set_peer_id(66).expect("a valid peer");
            let map = hostile.get_map("paragraphs");
            map.insert("deep", value).expect("writing the deep value");
            if overwrite {
                // The deep value then survives only in the history.
                map.insert("deep", 1).expect("overwriting it");
            }
            hostile.commit();
            let update = hostile
                .export(loro::ExportMode::all_updates())
                .expect("exporting the update");
            std::mem::forget(hostile);
            update
        })
        .expect("a thread with a large stack")
        .join()
        .expect("building the hostile update")
}

#[test]
fn a_deeply_nested_value_poisons_the_document_instead_of_crashing_it() {
    // 1,000 levels is far beyond MAX_VALUE_DEPTH but shallow enough for the library to decode on a 2 MiB stack in a debug build, so this test checks the logic: detect, poison, refuse, and leak instead of freeing. At about 20,000 levels the library's own decoder overflows such a stack before the adapter can look (CORE-004 report, "Resource limits"); the spike demonstrates the adapter's protection at depths where freeing would crash.
    for overwrite in [false, true] {
        let update = hostile_update(1_000, overwrite);
        let outcome = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let target = doc(2);
                let result = target.import(&update, &ImportLimits::UPDATE);
                let poisoned = target.is_poisoned();
                let refused_export = target.export_snapshot() == Err(CrdtError::Poisoned);
                let refused_import =
                    target.import(&update, &ImportLimits::UPDATE) == Err(ImportError::Poisoned);
                drop(target);
                (result, poisoned, refused_export, refused_import)
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(
            outcome,
            (Err(ImportError::ValueTooDeep), true, true, true),
            "overwrite: {overwrite}"
        );
    }
}

#[test]
fn shallow_nested_values_are_accepted() {
    let update = hostile_update(bayan_crdt::MAX_VALUE_DEPTH - 1, false);
    let target = doc(2);
    target.import(&update, &ImportLimits::UPDATE).unwrap();
    assert!(!target.is_poisoned());
    let value = target.registry("paragraphs").len();
    assert_eq!(value, 1);
}

/// Pins a bug of Loro 1.16.2 (reported in the CORE-004 report): undoing the creation of a child container and redoing it duplicates the container's content, including what other replicas wrote into it in between. If this test starts failing, Loro has fixed the bug and the workaround below may be reconsidered.
#[test]
fn redoing_the_creation_of_a_child_container_duplicates_its_content() {
    let a = doc(1);
    let b = doc(2);
    let mut undo = UndoManager::new(&a);
    a.stories().create("s").unwrap().insert(0, "P").unwrap();
    a.commit();
    sync(&a, &b);
    b.stories().get("s").unwrap().insert(0, "typed").unwrap();
    b.commit();
    sync(&b, &a);
    assert!(undo.undo().unwrap());
    assert!(a.stories().get("s").is_none());
    assert!(undo.redo().unwrap());
    assert_eq!(a.stories().get("s").unwrap().text(), "typedPtypedP");
}

/// The workaround the model uses: create entities in a change that is not an undo step, then reference them in one that is. Undo hides the reference, redo shows it again, and the entity's content, including other replicas' edits, is intact.
#[test]
fn entities_created_without_undo_survive_undo_and_redo_intact() {
    let a = doc(1);
    let b = doc(2);
    let mut undo = UndoManager::new(&a);
    a.stories().create("s").unwrap().insert(0, "P").unwrap();
    a.commit_without_undo();
    a.main_story().insert(0, "R").unwrap();
    a.commit();
    sync(&a, &b);
    b.stories().get("s").unwrap().insert(0, "typed").unwrap();
    b.commit();
    sync(&b, &a);
    assert_eq!(undo.undo_count(), 1);
    assert!(undo.undo().unwrap());
    assert_eq!(a.main_story().text(), "");
    assert_eq!(a.stories().get("s").unwrap().text(), "typedP");
    assert!(undo.redo().unwrap());
    assert_eq!(a.main_story().text(), "R");
    assert_eq!(a.stories().get("s").unwrap().text(), "typedP");
    // An empty change without undo leaves the next change undoable.
    a.commit_without_undo();
    a.main_story().insert(1, "S").unwrap();
    a.commit();
    assert_eq!(undo.undo_count(), 2);
}

/// Pins a limitation of Loro 1.16.2's undo (CORE-004 report): undoing a mark reverts it on the characters it was applied to, but not on characters another replica inserted strictly inside the range concurrently, which took the mark.
#[test]
fn undoing_a_mark_leaves_it_on_text_inserted_inside_concurrently() {
    let a = doc(1);
    let b = doc(2);
    let mut undo = UndoManager::new(&a);
    a.main_story().insert(0, "abcdef").unwrap();
    a.commit();
    sync(&a, &b);
    a.main_story()
        .mark(0..6, "r:b", &Value::Bool(true))
        .unwrap();
    a.commit();
    b.main_story().insert(3, "XY").unwrap();
    b.commit();
    sync(&a, &b);
    sync(&b, &a);
    assert!(undo.undo().unwrap());
    sync(&a, &b);
    let runs = b.main_story().runs();
    assert_eq!(runs, a.main_story().runs());
    let bold: Vec<(&str, bool)> = runs
        .iter()
        .map(|run| (run.text.as_str(), run.marks.contains_key("r:b")))
        .collect();
    assert_eq!(bold, [("abc", false), ("XY", true), ("def", false)]);
}

/// Found by the CORE-004 fuzzer and minimized with `crdt-model minimize`: a valid update for the document in the base snapshot, and the same update with one byte changed (and its checksum recomputed, which anyone can do: it is not a signature). Loro 1.16.2 panics while importing the crafted one.
const PANIC_BASE: &[u8] = include_bytes!("fixtures/loro-panic-base.bin");
const PANIC_UPDATE: &[u8] = include_bytes!("fixtures/loro-panic-update.bin");
const PANIC_CRAFTED: &[u8] = include_bytes!("fixtures/loro-panic-crafted.bin");

#[test]
fn a_panic_during_an_import_poisons_the_document_instead_of_crashing() {
    let load = || {
        Doc::load(
            PANIC_BASE,
            PeerId(9),
            &FAMILIES,
            &ImportLimits::LOCAL_SNAPSHOT,
        )
        .expect("the base document")
    };
    let valid = load();
    valid
        .import(PANIC_UPDATE, &ImportLimits::UPDATE)
        .expect("the valid update");
    let changed: Vec<usize> = (0..PANIC_UPDATE.len())
        .filter(|at| !(16..20).contains(at) && PANIC_UPDATE.get(*at) != PANIC_CRAFTED.get(*at))
        .collect();
    assert_eq!(
        (PANIC_CRAFTED.len(), changed),
        (PANIC_UPDATE.len(), vec![74])
    );

    let doc = load();
    let undo = UndoManager::new(&doc);
    // When Loro stops panicking on this blob, replace the fixtures with a case that still panics (the fuzzer finds them), so that this test keeps exercising the containment.
    assert_eq!(
        doc.import(PANIC_CRAFTED, &ImportLimits::UPDATE),
        Err(ImportError::Panicked)
    );
    assert!(doc.is_poisoned());
    assert_eq!(
        doc.import(PANIC_UPDATE, &ImportLimits::UPDATE),
        Err(ImportError::Poisoned)
    );
    assert_eq!(doc.export_snapshot(), Err(CrdtError::Poisoned));
    // Freeing the document or its undo manager after such a panic can panic again, which during unwinding aborts the process; both are leaked instead, so dropping them returns normally.
    drop(undo);
    drop(doc);
}
