//! Values that another replica stored and this adapter refuses to store: a floating-point number, a container reference or a value nested too deeply. Another implementation, or a modified client, can store them through Loro directly; the view reads them as missing, and operations that copy stored values must skip them instead of failing halfway (review of CORE-004, second round, item A1).

use bayan_crdt::{ATOM_KEY, ImportLimits, Value};
use bayan_model::{
    AtomKind, Document, EditError, EntityId, Item, MaterializationCheck, encode_binding, marks,
};
use loro::{ExpandType, ExportMode, LoroDoc, LoroMap, StyleConfig, ValueOrContainer};

use crate::common::{document, marks_at, replica_of, text, valid_view};

const MAIN: EntityId = EntityId::MAIN_STORY;

/// Plays a replica that writes, through Loro's own API, what `write` writes: it receives everything `document` has, writes, and sends its changes back.
fn foreign_replica_writes(document: &mut Document, write: impl FnOnce(&LoroDoc)) {
    let replica = LoroDoc::new();
    replica.set_peer_id(99).expect("a peer identifier");
    replica.config_default_text_style(Some(StyleConfig {
        expand: ExpandType::None,
    }));
    replica
        .import(&document.export_snapshot().expect("a snapshot"))
        .expect("importing the snapshot");
    let before = replica.oplog_vv();
    write(&replica);
    replica.commit();
    let update = replica
        .export(ExportMode::updates(&before))
        .expect("exporting the update");
    document
        .import(&update, &ImportLimits::UPDATE)
        .expect("importing the update");
}

/// Stores `value` under `key` in the properties of `paragraph`, as the foreign replica.
fn store_in_paragraph(replica: &LoroDoc, paragraph: EntityId, key: &str, value: f64) {
    let Some(ValueOrContainer::Container(container)) =
        replica.get_map("paragraphs").get(&paragraph.to_string())
    else {
        panic!("the paragraph's properties are a map");
    };
    let map = container.into_map().expect("a map");
    map.insert(key, value).expect("storing a float");
}

#[test]
fn a_move_leaves_out_marks_whose_value_this_adapter_cannot_store() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    foreign_replica_writes(&mut a, |replica| {
        replica
            .get_text("main")
            .mark(1..4, marks::SIZE, 10.5)
            .expect("marking with a float");
    });
    let before = valid_view(&a);
    assert!(!marks_at(&before, MAIN, 2).contains_key(marks::SIZE));
    // Before the fix, pasting "bcd" failed on its float mark after the cut, and "e" was lost.
    a.move_range(MAIN, 1..5, MAIN, 6).unwrap();
    assert_eq!(text(&a), "afbcde\n");
    a.insert_text(MAIN, 0, ">").unwrap();
    assert_eq!(valid_view(&replica_of(&a, 3)), valid_view(&a));
    assert!(a.undo().unwrap());
    assert_eq!(text(&a), "afbcde\n");
    assert!(a.undo().unwrap());
    assert_eq!(text(&a), "abcdef\n");
}

#[test]
fn a_split_copies_only_the_properties_this_adapter_can_store() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    let paragraph = a.paragraphs_in(MAIN)[0];
    a.set_paragraph_property(paragraph, "jc", &Value::from("center"))
        .unwrap();
    foreign_replica_writes(&mut a, |replica| {
        store_in_paragraph(replica, paragraph, "spacing", 1.5);
    });
    let before = valid_view(&a);
    assert!(!before.paragraphs[&paragraph].contains_key("spacing"));
    // Before the fix, every Enter in this paragraph failed after creating the new paragraph's map, and the next undo took that write instead of the user's edit.
    let first = a.split_paragraph(MAIN, 3).unwrap();
    let view = valid_view(&a);
    assert_eq!(view.plain_text(MAIN), "abc\ndef\n");
    assert_eq!(view.paragraphs[&first], view.paragraphs[&paragraph]);
    assert_eq!(
        view.paragraphs[&first].get("jc"),
        Some(&Value::from("center"))
    );
    a.split_paragraph(MAIN, 1).unwrap();
    assert_eq!(text(&a), "a\nbc\ndef\n");
    assert!(a.undo().unwrap());
    assert_eq!(text(&a), "abc\ndef\n");
    assert!(a.undo().unwrap());
    assert_eq!(valid_view(&a), before);
}

#[test]
fn a_materialization_copies_only_the_properties_this_adapter_can_store() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "ab").unwrap();
    a.split_paragraph(MAIN, 2).unwrap();
    a.insert_table(MAIN, 3, 1, 1).unwrap();
    let paragraphs = a.paragraphs_in(MAIN);
    // The foreign replica types right before the table, so the view splits the paragraph there (N4) with the containing paragraph's properties, and stores a float in every paragraph's properties.
    foreign_replica_writes(&mut a, |replica| {
        replica.get_text("main").insert(3, "x").expect("typing");
        for paragraph in &paragraphs {
            store_in_paragraph(replica, *paragraph, "spacing", 1.5);
        }
    });
    assert!(bayan_model::normalize(&a.raw()).1.n4 > 0);
    a.check_materializations();
    let before = valid_view(&a);
    // Typing materializes the split first. Before the fix, copying the float failed after a partial write: the edit was refused, the view changed, and the next undo took the partial write.
    a.insert_text(MAIN, 0, "Q").unwrap();
    assert_eq!(
        a.materialization_check(),
        Some(MaterializationCheck {
            performed: 1,
            changed_view: 0,
            failed: 0
        })
    );
    assert_eq!(text(&a), format!("Q{}", before.plain_text(MAIN)));
    assert!(a.undo().unwrap());
    assert_eq!(valid_view(&a), before);
}

#[test]
fn values_this_adapter_cannot_store_are_refused_before_anything_is_written() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    let paragraph = a.paragraphs_in(MAIN)[0];
    // A foreign replica stores a paragraph end whose paragraph has no properties map: the view drops it (N5), but formatting a run property over it would create that map before the value fails to store.
    let mapless = EntityId(0xBAD);
    foreign_replica_writes(&mut a, |replica| {
        let main = replica.get_text("main");
        main.insert(1, &AtomKind::ParagraphEnd.placeholder().to_string())
            .expect("typing a placeholder");
        main.mark(
            1..2,
            ATOM_KEY,
            encode_binding(AtomKind::ParagraphEnd, mapless),
        )
        .expect("binding it");
    });
    let before = valid_view(&a);
    let steps = a.undo_count();
    for value in [
        Value::Unsupported,
        Value::List(vec![Value::Int(1), Value::Unsupported]),
    ] {
        assert!(matches!(
            a.format(MAIN, 0..5, marks::SIZE, &value),
            Err(EditError::Crdt(_))
        ));
        assert!(matches!(
            a.set_paragraph_property(paragraph, "spacing", &value),
            Err(EditError::Crdt(_))
        ));
    }
    assert_eq!(valid_view(&a), before);
    assert_eq!(a.undo_count(), steps);
    // Nothing is pending: the next edit is one undo step of its own, and committing it writes no properties map for the foreign paragraph end.
    a.insert_text(MAIN, 0, "z").unwrap();
    assert!(!a.raw().paragraphs.contains_key(&mapless));
    assert_eq!(a.undo_count(), steps + 1);
    assert!(a.undo().unwrap());
    assert_eq!(valid_view(&a), before);
}

/// What another replica stores in the paragraph registry, under one paragraph's identifier.
#[derive(Clone, Copy, Debug)]
enum ForeignEntry {
    /// A map container created directly, not as the adapter creates entity maps.
    PlainMap,
    /// A value where a map belongs.
    Float,
}

/// Stores `entry` under `paragraph` in the paragraph registry, as the foreign replica, replacing what was there.
fn replace_paragraph_entry(replica: &LoroDoc, paragraph: EntityId, entry: ForeignEntry) {
    let paragraphs = replica.get_map("paragraphs");
    match entry {
        ForeignEntry::PlainMap => {
            let map = paragraphs
                .insert_container(&paragraph.to_string(), LoroMap::new())
                .expect("storing a plain map");
            map.insert("style", "Normal").expect("a property");
        }
        ForeignEntry::Float => {
            paragraphs
                .insert(&paragraph.to_string(), 1.5)
                .expect("storing a float");
        }
    }
}

/// The identifier of the paragraph end that the view shows and the stored state does not bind: the one normalization derived.
fn derived_paragraph_end(document: &Document) -> EntityId {
    let raw = document.raw();
    let stored: Vec<EntityId> = raw
        .main
        .iter()
        .filter_map(|run| run.marks.get(ATOM_KEY).and_then(Value::as_str))
        .filter_map(bayan_model::decode_binding)
        .map(|(_, id)| id)
        .collect();
    let derived: Vec<EntityId> = valid_view(document)
        .main()
        .iter()
        .filter_map(|item| match item {
            Item::Atom {
                kind: AtomKind::ParagraphEnd,
                id: Some(id),
                ..
            } if !stored.contains(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(derived.len(), 1, "exactly one derived paragraph end");
    derived[0]
}

#[test]
fn formatting_writes_into_or_leaves_out_paragraph_entries_another_replica_stored() {
    for entry in [ForeignEntry::PlainMap, ForeignEntry::Float] {
        let mut a = document(1);
        a.insert_text(MAIN, 0, "abc").unwrap();
        a.split_paragraph(MAIN, 2).unwrap();
        a.split_paragraph(MAIN, 1).unwrap();
        let paragraphs = a.paragraphs_in(MAIN);
        assert_eq!(text(&a), "a\nb\nc\n");
        // The second of the three paragraphs: before the fix, formatting wrote the first paragraph's mark property and then failed on this one, leaving the write pending.
        let middle = paragraphs[1];
        foreign_replica_writes(&mut a, |replica| {
            replace_paragraph_entry(replica, middle, entry);
        });
        let before = valid_view(&a);
        a.format(MAIN, 0..6, marks::BOLD, &Value::Bool(true))
            .unwrap();
        assert!(!a.has_pending_changes());
        let view = valid_view(&a);
        let bold = Some(&Value::Bool(true));
        for paragraph in &paragraphs {
            match (entry, *paragraph == middle) {
                // A float is no map: the view does not show that paragraph end, and formatting leaves it alone.
                (ForeignEntry::Float, true) => {
                    assert!(!view.paragraphs.contains_key(paragraph));
                    assert!(!a.raw().paragraphs.contains_key(paragraph));
                }
                // A plain map is written into, as the view shows its properties.
                _ => assert_eq!(view.paragraphs[paragraph].get("rPr.b"), bold),
            }
        }
        a.clear_format(MAIN, 0..6, marks::BOLD).unwrap();
        assert!(!a.has_pending_changes());
        let cleared = valid_view(&a);
        assert!(
            cleared
                .paragraphs
                .values()
                .all(|props| !props.contains_key("rPr.b"))
        );
        assert!(a.undo().unwrap());
        assert!(a.undo().unwrap());
        assert_eq!(valid_view(&a), before);
    }
}

/// Builds a main story whose view needs a paragraph end that the stored state lacks: before a table that another replica's typing left in the middle of a paragraph (N4), or at the end of a story whose final paragraph end another replica deleted (N1).
fn needs_a_derived_paragraph_end(n1: bool) -> Document {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "ab").expect("typing");
    a.split_paragraph(MAIN, 2).expect("a split");
    a.insert_table(MAIN, 3, 1, 1).expect("a table");
    foreign_replica_writes(&mut a, |replica| {
        let main = replica.get_text("main");
        if n1 {
            let len = main.len_unicode();
            main.delete(len - 1, 1)
                .expect("deleting the final paragraph end");
        } else {
            main.insert(3, "x").expect("typing before the table");
        }
    });
    a
}

#[test]
fn materialization_avoids_identifiers_where_another_replica_stored_an_entry() {
    for n1 in [false, true] {
        for entry in [ForeignEntry::PlainMap, ForeignEntry::Float] {
            let mut a = needs_a_derived_paragraph_end(n1);
            let derived = derived_paragraph_end(&a);
            // Another replica stores an entry under the identifier that normalization derived for the missing paragraph end. Before the fix, normalization kept deriving it, and materializing it failed on every edit of the story.
            foreign_replica_writes(&mut a, |replica| {
                replace_paragraph_entry(replica, derived, entry);
            });
            assert_ne!(derived_paragraph_end(&a), derived);
            a.check_materializations();
            let before = valid_view(&a);
            a.insert_text(MAIN, 0, "Q").unwrap();
            assert!(!a.has_pending_changes());
            assert_eq!(
                a.materialization_check(),
                Some(MaterializationCheck {
                    performed: 1,
                    changed_view: 0,
                    failed: 0
                }),
                "{entry:?}, n1 {n1}"
            );
            assert_eq!(text(&a), format!("Q{}", before.plain_text(MAIN)));
            assert!(a.undo().unwrap());
            assert_eq!(valid_view(&a), before);
            assert!(!a.has_pending_changes());
        }
    }
}

/// A mergeable map under the identifier that normalization derives, as an earlier materialization of the same paragraph end leaves it when that end is deleted, is reused rather than avoided (review of 9c43419): the view shows its properties, materializing into it changes nothing the view shows, and every replica derives that identifier whether or not it received the map.
#[test]
fn materialization_reuses_a_mergeable_map_under_the_derived_identifier() {
    for n1 in [false, true] {
        let mut a = needs_a_derived_paragraph_end(n1);
        let derived = derived_paragraph_end(&a);
        foreign_replica_writes(&mut a, |replica| {
            replica
                .get_map("paragraphs")
                .ensure_mergeable_map(&derived.to_string())
                .expect("a mergeable map")
                .insert("style", "Heading1")
                .expect("a property");
        });
        assert_eq!(derived_paragraph_end(&a), derived, "n1 {n1}");
        let before = valid_view(&a);
        assert_eq!(
            before.paragraphs[&derived].get("style"),
            Some(&Value::from("Heading1"))
        );
        a.check_materializations();
        a.insert_text(MAIN, 0, "Q").unwrap();
        assert!(!a.has_pending_changes());
        assert_eq!(
            a.materialization_check(),
            Some(MaterializationCheck {
                performed: 1,
                changed_view: 0,
                failed: 0
            }),
            "n1 {n1}"
        );
        // The stored paragraph end now binds the derived identifier, whose map keeps its properties.
        assert!(
            bayan_model::normalize(&a.raw())
                .1
                .virtual_paragraph_ends
                .is_empty()
        );
        assert_eq!(
            valid_view(&a).paragraphs[&derived].get("style"),
            Some(&Value::from("Heading1"))
        );
        assert!(a.undo().unwrap());
        assert_eq!(valid_view(&a), before);
    }
}
