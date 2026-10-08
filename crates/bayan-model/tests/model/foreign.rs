//! Values that another replica stored and this adapter refuses to store: a floating-point number, a container reference or a value nested too deeply. Another implementation, or a modified client, can store them through Loro directly; the view reads them as missing, and operations that copy stored values must skip them instead of failing halfway (review of CORE-004, second round, item A1).

use bayan_crdt::{ATOM_KEY, ImportLimits, Value};
use bayan_model::{
    AtomKind, Document, EditError, EntityId, MaterializationCheck, encode_binding, marks,
};
use loro::{ExpandType, ExportMode, LoroDoc, StyleConfig, ValueOrContainer};

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
