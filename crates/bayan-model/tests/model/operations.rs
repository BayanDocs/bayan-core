//! Every operation of the brief, on one replica and across replicas, with the invariants checked on every view.

use bayan_crdt::Value;
use bayan_model::{
    AtomKind, Document, EditError, EntityId, Item, MaterializationCheck, encode_binding, marks,
};

use crate::common::{document, replica_of, sync_both, text, valid_view};

const MAIN: EntityId = EntityId::MAIN_STORY;

#[test]
fn typing_splitting_and_merging_paragraphs() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "Hello world").unwrap();
    assert_eq!(text(&a), "Hello world\n");
    let first = a.split_paragraph(MAIN, 5).unwrap();
    assert_eq!(text(&a), "Hello\n world\n");
    // The new paragraph ends the first part and copies the properties of the paragraph it was split from.
    let view = valid_view(&a);
    assert_eq!(view.paragraphs.len(), 2);
    assert_eq!(
        view.paragraphs[&first].get("style"),
        Some(&Value::from("Normal"))
    );
    a.merge_paragraph(MAIN, 5).unwrap();
    assert_eq!(text(&a), "Hello world\n");
    assert!(!valid_view(&a).paragraphs.contains_key(&first));
}

#[test]
fn refuses_what_word_refuses() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    // Control characters are reserved for atoms.
    assert_eq!(
        a.insert_text(MAIN, 0, "a\u{0D}b"),
        Err(EditError::InvalidText)
    );
    assert_eq!(a.insert_text(MAIN, 0, "\t"), Err(EditError::InvalidText));
    // Nothing goes after the final paragraph end, and it cannot be deleted or merged.
    assert!(matches!(
        a.insert_text(MAIN, 4, "x"),
        Err(EditError::InvalidPosition { .. })
    ));
    assert!(matches!(
        a.delete(MAIN, 2..4),
        Err(EditError::InvalidPosition { .. })
    ));
    assert_eq!(a.merge_paragraph(MAIN, 3), Err(EditError::NotAParagraphEnd));
    assert_eq!(a.merge_paragraph(MAIN, 1), Err(EditError::NotAParagraphEnd));
    // A table only at a block position.
    assert_eq!(
        a.insert_table(MAIN, 1, 1, 1),
        Err(EditError::NotABlockPosition)
    );
    assert_eq!(text(&a), "abc\n");
}

#[test]
fn tables_rows_and_columns() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "after").unwrap();
    let table = a.insert_table(MAIN, 0, 2, 3).unwrap();
    let view = valid_view(&a);
    let shape = |view: &bayan_model::View| -> Vec<usize> {
        view.tables[&table]
            .rows
            .iter()
            .map(|row| view.rows[row].cells.len())
            .collect()
    };
    assert_eq!(shape(&view), [3, 3]);
    assert_eq!(
        view.main()[0],
        Item::Atom {
            kind: AtomKind::TableBlock,
            id: Some(table),
            marks: Default::default()
        }
    );
    // Every cell owns a story with its final paragraph end.
    for cell in view.cells.values() {
        assert_eq!(view.plain_text(cell.story), "\n");
    }
    // Typing at the block position before the table is refused; after it, it is fine.
    assert_eq!(
        a.insert_text(MAIN, 0, "x"),
        Err(EditError::NotABlockPosition)
    );
    a.insert_row(table, 1).unwrap();
    assert_eq!(shape(&valid_view(&a)), [3, 3, 3]);
    a.move_row(table, 0, 2).unwrap();
    a.delete_row(table, 0).unwrap();
    assert_eq!(shape(&valid_view(&a)), [3, 3]);
    a.insert_column(table, 1).unwrap();
    assert_eq!(shape(&valid_view(&a)), [4, 4]);
    a.delete_column(table, 0).unwrap();
    a.delete_column(table, 0).unwrap();
    a.delete_column(table, 0).unwrap();
    assert_eq!(shape(&valid_view(&a)), [1, 1]);
    assert_eq!(a.delete_column(table, 0), Err(EditError::LastRowOrColumn));
    a.delete_row(table, 0).unwrap();
    assert_eq!(a.delete_row(table, 0), Err(EditError::LastRowOrColumn));
    assert_eq!(text(&a), "after\n");
}

#[test]
fn concurrent_column_deletions_can_empty_a_table_which_normalization_omits() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "x").unwrap();
    let table = a.insert_table(MAIN, 0, 2, 2).unwrap();
    let mut b = replica_of(&a, 2);
    // Each replica deletes a different column: together they delete both.
    a.delete_column(table, 0).unwrap();
    b.delete_column(table, 1).unwrap();
    sync_both(&mut a, &mut b);
    let view = valid_view(&a);
    assert_eq!(view, valid_view(&b));
    assert!(!view.tables.contains_key(&table));
    assert_eq!(text(&a), "x\n");
}

#[test]
fn concurrent_row_operations_merge() {
    let mut a = document(1);
    let table = a.insert_table(MAIN, 0, 3, 1).unwrap();
    let mut b = replica_of(&a, 2);
    a.move_row(table, 0, 2).unwrap();
    b.move_row(table, 0, 1).unwrap();
    b.insert_row(table, 3).unwrap();
    a.delete_row(table, 1).unwrap();
    sync_both(&mut a, &mut b);
    let view = valid_view(&a);
    assert_eq!(view, valid_view(&b));
    // One row deleted, one inserted: still three, and the row moved twice exists once.
    assert_eq!(view.tables[&table].rows.len(), 3);
}

#[test]
fn comments_fields_objects_and_bookmarks() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "Hello world").unwrap();
    let comment = a.add_comment(MAIN, 0..5, "Reviewer", "Nice").unwrap();
    let view = valid_view(&a);
    assert_eq!(view.plain_text(view.comments[&comment].story), "Nice\n");
    assert_eq!(
        view.comments[&comment].props.get("author"),
        Some(&Value::from("Reviewer"))
    );
    let key = marks::comment_key(comment);
    assert!(crate::common::marks_at(&view, MAIN, 0).contains_key(&key));
    assert!(crate::common::marks_at(&view, MAIN, 4).contains_key(&key));
    // The reference atom follows the range and carries no highlight.
    assert!(view.main()[1].is(AtomKind::CommentReference));
    assert!(!crate::common::marks_at(&view, MAIN, 6).contains_key(&key));

    let field = a.insert_field(MAIN, 6, "PAGE", "7").unwrap();
    let object = a.insert_object(MAIN, 0).unwrap();
    let bookmark = a.insert_bookmark(MAIN, 1..3, "start").unwrap();
    let view = valid_view(&a);
    assert!(view.fields.contains_key(&field));
    assert!(view.objects.contains_key(&object));
    assert_eq!(
        view.ranges[&bookmark].get("name"),
        Some(&Value::from("start"))
    );
    // The field code is hidden, the result shown; the object shows as U+FFFC; no placeholder leaks.
    assert_eq!(view.plain_text(MAIN), "\u{FFFC}Hello7 world\n");
}

#[test]
fn a_moved_range_keeps_its_entities_and_formatting() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "one two three").unwrap();
    a.format(MAIN, 0..3, marks::BOLD, &Value::Bool(true))
        .unwrap();
    let object = a.insert_object(MAIN, 3).unwrap();
    // Move "one" plus the object to the end of "three".
    a.move_range(MAIN, 0..4, MAIN, 14).unwrap();
    let view = valid_view(&a);
    assert_eq!(view.plain_text(MAIN), " two threeone\u{FFFC}\n");
    assert!(view.objects.contains_key(&object));
    assert_eq!(
        crate::common::marks_at(&view, MAIN, 10).get(marks::BOLD),
        Some(&Value::Bool(true))
    );
    // The text before the moved range was not bold and must not have become so.
    assert!(!crate::common::marks_at(&view, MAIN, 9).contains_key(marks::BOLD));
}

#[test]
fn concurrent_moves_of_the_same_object_keep_one() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    let object = a.insert_object(MAIN, 3).unwrap();
    let mut b = replica_of(&a, 2);
    a.move_range(MAIN, 3..4, MAIN, 0).unwrap();
    b.move_range(MAIN, 3..4, MAIN, 7).unwrap();
    sync_both(&mut a, &mut b);
    let view = valid_view(&a);
    assert_eq!(view, valid_view(&b));
    // Both replicas re-inserted the object's atom: the stored story holds it twice, and normalization keeps the first (N7).
    let raw_count = a
        .raw()
        .main
        .iter()
        .filter(|run| run.text.contains(AtomKind::ObjectAnchor.placeholder()))
        .count();
    assert_eq!(raw_count, 2);
    assert_eq!(view.plain_text(MAIN).matches('\u{FFFC}').count(), 1);
    assert!(view.objects.contains_key(&object));
}

#[test]
fn a_table_cannot_be_moved_into_itself_but_concurrent_moves_can_create_a_cycle() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "x").unwrap();
    let outer = a.insert_table(MAIN, 0, 1, 1).unwrap();
    let inner = a.insert_table(MAIN, 1, 1, 1).unwrap();
    let view = valid_view(&a);
    let cell_story = |view: &bayan_model::View, table: EntityId| {
        view.cells[&view.rows[&view.tables[&table].rows[0]].cells[0]].story
    };
    let outer_cell = cell_story(&view, outer);
    let inner_cell = cell_story(&view, inner);
    assert_eq!(
        a.move_range(MAIN, 0..1, outer_cell, 0),
        Err(EditError::WouldCreateCycle)
    );
    // Two replicas each move one table into the other's cell: the merged state nests them in each other.
    let mut b = replica_of(&a, 2);
    a.move_range(MAIN, 0..1, inner_cell, 0).unwrap();
    b.move_range(MAIN, 1..2, outer_cell, 0).unwrap();
    sync_both(&mut a, &mut b);
    let view = valid_view(&a);
    assert_eq!(view, valid_view(&b));
    // Neither table is reachable from the main story any more; the cycle never makes normalization loop.
    assert_eq!(view.plain_text(MAIN), "x\n");
}

#[test]
fn a_paragraph_mark_formatted_with_its_text_stores_the_formatting_on_the_paragraph() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    let first = a.split_paragraph(MAIN, 3).unwrap();
    a.insert_text(MAIN, 4, "def").unwrap();
    // Select the first paragraph including its mark (as a triple click does) and make it bold.
    a.format(MAIN, 0..4, marks::BOLD, &Value::Bool(true))
        .unwrap();
    let view = valid_view(&a);
    assert_eq!(
        view.paragraphs[&first].get("rPr.b"),
        Some(&Value::Bool(true))
    );
    assert!(crate::common::marks_at(&view, MAIN, 2).contains_key(marks::BOLD));
    // The paragraph end itself carries no marks in the view.
    assert!(crate::common::marks_at(&view, MAIN, 3).is_empty());
    // Typing at the start of the next paragraph does not inherit the bold.
    a.insert_text(MAIN, 4, "X").unwrap();
    assert!(!crate::common::marks_at(&valid_view(&a), MAIN, 4).contains_key(marks::BOLD));
    a.clear_format(MAIN, 0..4, marks::BOLD).unwrap();
    assert!(!valid_view(&a).paragraphs[&first].contains_key("rPr.b"));
}

/// Two replicas whose story shows a paragraph split that only normalization added (N4): a table inserted at the start of a paragraph while the other replica typed there, merged with the text before the table. Returns `None` when this peer order put the text after the table instead.
fn replicas_with_a_virtual_split(
    peer_a: u64,
    peer_b: u64,
) -> Option<(Document, Document, EntityId)> {
    let mut a = document(peer_a);
    a.insert_text(MAIN, 0, "first").expect("typing");
    a.split_paragraph(MAIN, 5).expect("splitting");
    let mut b = replica_of(&a, peer_b);
    let table = a.insert_table(MAIN, 6, 1, 1).expect("a table");
    b.insert_text(MAIN, 6, "typed")
        .expect("typing concurrently");
    sync_both(&mut a, &mut b);
    (bayan_model::normalize(&a.raw()).1.n4 > 0).then_some((a, b, table))
}

/// The edit of a story whose view shows a split that only normalization added materializes the split first, which does not change the view, also when two replicas materialize concurrently. A refused edit materializes too, so it is used here to materialize without editing.
#[test]
fn materialization_does_not_change_the_view_even_when_replicas_materialize_concurrently() {
    let mut saw_split = false;
    for (peer_a, peer_b) in [(1, 2), (2, 1)] {
        let Some((mut a, mut b, table)) = replicas_with_a_virtual_split(peer_a, peer_b) else {
            continue;
        };
        saw_split = true;
        let before = valid_view(&a);
        assert_eq!(before, valid_view(&b));
        // The view shows the split before the table.
        let position = before
            .main()
            .iter()
            .position(|item| item.is(AtomKind::TableBlock))
            .unwrap();
        assert!(before.main()[position - 1].is(AtomKind::ParagraphEnd));
        for replica in [&mut a, &mut b] {
            replica.check_materializations();
            assert!(matches!(
                replica.insert_text(MAIN, usize::MAX, "x"),
                Err(EditError::InvalidPosition {
                    pos: usize::MAX,
                    ..
                })
            ));
            assert_eq!(
                replica.materialization_check(),
                Some(MaterializationCheck {
                    performed: 1,
                    changed_view: 0
                })
            );
            assert_eq!(bayan_model::normalize(&replica.raw()).1.n4, 0);
            assert_eq!(valid_view(replica), before);
        }
        // Both replicas materialized the same split; merged, they still show the same view.
        sync_both(&mut a, &mut b);
        assert_eq!(valid_view(&a), before);
        assert_eq!(valid_view(&b), before);
        assert!(before.tables.contains_key(&table));
    }
    assert!(
        saw_split,
        "neither peer order put the typed text before the table"
    );
}

/// The edit that materializes the split is one undo step: undoing it returns to the view before it, with the split still stored.
#[test]
fn undoing_an_edit_that_materialized_a_split_returns_to_the_view_before_it() {
    let mut saw_split = false;
    for (peer_a, peer_b) in [(1, 2), (2, 1)] {
        let Some((mut a, _, _)) = replicas_with_a_virtual_split(peer_a, peer_b) else {
            continue;
        };
        saw_split = true;
        let before = valid_view(&a);
        a.check_materializations();
        a.insert_text(MAIN, 0, "z").unwrap();
        assert_eq!(a.materialization_check().unwrap().performed, 1);
        assert_eq!(bayan_model::normalize(&a.raw()).1.n4, 0);
        assert_eq!(text(&a), format!("z{}", before.plain_text(MAIN)));
        assert!(a.undo().unwrap());
        assert_eq!(valid_view(&a), before);
        assert_eq!(bayan_model::normalize(&a.raw()).1.n4, 0);
        assert!(a.redo().unwrap());
        assert_eq!(text(&a), format!("z{}", before.plain_text(MAIN)));
    }
    assert!(saw_split);
}

/// Text moved from another story to just after a table whose split only normalization showed lands after the table: the position the caller computed before the target's materialization is mapped onto the story after it.
#[test]
fn a_move_into_a_story_that_needs_materialization_lands_where_the_caller_meant() {
    let mut saw_split = false;
    for (peer_a, peer_b) in [(1, 2), (2, 1)] {
        let Some((mut a, _, table)) = replicas_with_a_virtual_split(peer_a, peer_b) else {
            continue;
        };
        saw_split = true;
        let comment = a.add_comment(MAIN, 0..1, "Reviewer", "zz").unwrap();
        let story = valid_view(&a).comments[&comment].story;
        let after_table = a
            .story_text(MAIN)
            .unwrap()
            .chars()
            .position(|character| character == AtomKind::TableBlock.placeholder())
            .unwrap()
            + 1;
        a.move_range(story, 0..2, MAIN, after_table).unwrap();
        let view = valid_view(&a);
        let main = view.main();
        let position = main
            .iter()
            .position(|item| item.is(AtomKind::TableBlock))
            .unwrap();
        assert_eq!(
            main[position],
            Item::Atom {
                kind: AtomKind::TableBlock,
                id: Some(table),
                marks: Default::default()
            }
        );
        assert!(main[position - 1].is(AtomKind::ParagraphEnd));
        assert!(matches!(&main[position + 1], Item::Text { text, .. } if text == "zz"));
    }
    assert!(saw_split);
}

#[test]
fn undoing_an_edit_that_materialized_returns_to_the_view_before_it() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    // A misbehaving replica deletes the final paragraph end, bypassing the model's checks.
    let b = replica_of(&a, 2);
    b.crdt().main_story().delete(3..4).unwrap();
    b.crdt().commit();
    crate::common::sync(&b, &mut a);
    let before = valid_view(&a);
    assert_eq!(bayan_model::normalize(&a.raw()).1.n1, 1);
    assert_eq!(before.plain_text(MAIN), "abc\n");
    // The next edit materializes the final paragraph end first.
    a.insert_text(MAIN, 3, "d").unwrap();
    assert_eq!(bayan_model::normalize(&a.raw()).1.n1, 0);
    assert_eq!(text(&a), "abcd\n");
    // Undo removes the typing; the materialized paragraph end stays stored, and the view is the one before the edit.
    assert!(a.undo().unwrap());
    assert_eq!(bayan_model::normalize(&a.raw()).1.n1, 0);
    assert_eq!(valid_view(&a), before);
    // The next undo step is the typing of "abc", not the materialization.
    assert!(a.undo().unwrap());
    assert_eq!(text(&a), "\n");
    assert!(!a.undo().unwrap());
    assert!(a.redo().unwrap());
    assert!(a.redo().unwrap());
    assert_eq!(text(&a), "abcd\n");
}

#[test]
fn a_placeholder_typed_through_the_adapter_never_leaks() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    // Bypass the model's checks: a raw C0 character and an unbound paragraph-end placeholder.
    let main = a.crdt().main_story();
    main.insert(1, "\u{01}\u{0D}\u{13}").unwrap();
    // And a placeholder bound to the wrong kind of entity.
    main.insert_atom(
        0,
        '\u{07}',
        &encode_binding(AtomKind::ParagraphEnd, EntityId(99)),
    )
    .unwrap();
    a.crdt().commit();
    let view = valid_view(&a);
    assert_eq!(view.plain_text(MAIN), "abc\n");
}

/// A refused move cuts nothing: everything is checked before the source is touched, so neither this replica nor one that receives its later changes ever loses the text (review of CORE-004, item 1). Covered: a target position beyond the same story, beyond another story, and exactly at the end of either (after the final paragraph end).
#[test]
fn a_refused_move_changes_nothing_here_or_on_other_replicas() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "Hello").unwrap();
    let comment = a.add_comment(MAIN, 4..5, "Reviewer", "note").unwrap();
    let other = valid_view(&a).comments[&comment].story;
    let before = valid_view(&a);
    let len = a.story_len(MAIN).unwrap();
    let other_len = a.story_len(other).unwrap();
    for (to, to_pos, story_len) in [
        (MAIN, 100, len),
        (other, 1_000, other_len),
        (MAIN, len, len),
        (other, other_len, other_len),
    ] {
        assert_eq!(
            a.move_range(MAIN, 0..2, to, to_pos),
            Err(EditError::InvalidPosition {
                pos: to_pos,
                len: story_len
            }),
            "to {to} at {to_pos}"
        );
        assert_eq!(valid_view(&a), before);
    }
    // The next operation commits whatever the refused ones left pending: there must be nothing.
    a.insert_text(MAIN, 0, ">").unwrap();
    assert_eq!(text(&a), ">Hello\n");
    let b = replica_of(&a, 2);
    assert_eq!(valid_view(&b), valid_view(&a));
}

/// Positions are checked before they are mapped past materialization, and refused positions are reported as the caller gave them (review of CORE-004, item 16).
#[test]
fn refused_positions_are_reported_as_given_even_after_materialization() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    // A misbehaving replica deletes the final paragraph end, so the next edit materializes it first.
    let b = replica_of(&a, 2);
    b.crdt().main_story().delete(3..4).unwrap();
    b.crdt().commit();
    crate::common::sync(&b, &mut a);
    assert_eq!(a.story_len(MAIN), Some(3));
    assert_eq!(
        a.insert_text(MAIN, usize::MAX, "X"),
        Err(EditError::InvalidPosition {
            pos: usize::MAX,
            len: 3
        })
    );
    // The refused edit materialized the final paragraph end, which the stored story now holds.
    assert_eq!(a.story_len(MAIN), Some(4));
    assert_eq!(
        a.insert_text(MAIN, 99, "X"),
        Err(EditError::InvalidPosition { pos: 99, len: 4 })
    );
    assert_eq!(text(&a), "abc\n");
    // Typing at the end of the story the caller saw lands before the materialized paragraph end.
    a.insert_text(MAIN, 3, "d").unwrap();
    assert_eq!(text(&a), "abcd\n");
}

/// The atom binding is written only by the adapter, and keys of no mark family have no agreed expansion: formatting with either is refused (review of CORE-004, item 17).
#[test]
fn formatting_refuses_the_atom_binding_and_unknown_families() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "a").unwrap();
    a.split_paragraph(MAIN, 1).unwrap();
    a.insert_text(MAIN, 2, "bc").unwrap();
    a.insert_table(MAIN, 2, 1, 1).unwrap();
    let before = valid_view(&a);
    let len = a.story_len(MAIN).unwrap();
    assert_eq!(
        a.clear_format(MAIN, 0..len, bayan_crdt::ATOM_KEY),
        Err(EditError::InvalidKey)
    );
    assert_eq!(
        a.format(MAIN, 0..1, bayan_crdt::ATOM_KEY, &Value::from("p:0")),
        Err(EditError::InvalidKey)
    );
    assert_eq!(
        a.format(MAIN, 0..1, "unknown:x", &Value::Bool(true)),
        Err(EditError::InvalidKey)
    );
    assert_eq!(valid_view(&a), before);
    // Keys of the model's families are accepted, including the family name alone.
    a.format(MAIN, 0..1, marks::LINK, &Value::from("https://example.org"))
        .unwrap();
}

/// Edits that would separate the parts of a field or the ends of a range are refused, and so are atoms that would stand at the block position before a table (review of CORE-004, item 18).
#[test]
fn edits_that_would_unbalance_fields_ranges_or_tables_are_refused() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "ab").unwrap();
    // "a" field(PAGE | 7) "b" ¶
    a.insert_field(MAIN, 1, "PAGE", "7").unwrap();
    assert_eq!(text(&a), "a7b\n");
    let before = valid_view(&a);
    // The field begin alone, the separator alone, and begin to separator.
    for range in [1..2, 6..7, 1..7] {
        assert_eq!(
            a.delete(MAIN, range.clone()),
            Err(EditError::WouldUnbalance),
            "{range:?}"
        );
        assert_eq!(
            a.move_range(MAIN, range.clone(), MAIN, 0),
            Err(EditError::WouldUnbalance),
            "{range:?}"
        );
    }
    assert_eq!(valid_view(&a), before);
    // The whole field may be deleted.
    a.delete(MAIN, 1..9).unwrap();
    assert_eq!(text(&a), "ab\n");

    let bookmark = a.insert_bookmark(MAIN, 0..1, "first").unwrap();
    // ⟦start⟧ a ⟦end⟧ b ¶: the end alone may not move away from its start.
    assert_eq!(
        a.move_range(MAIN, 2..3, MAIN, 4),
        Err(EditError::WouldUnbalance)
    );
    assert_eq!(a.delete(MAIN, 0..1), Err(EditError::WouldUnbalance));
    assert!(valid_view(&a).ranges.contains_key(&bookmark));
    a.delete(MAIN, 0..3).unwrap();
    assert_eq!(text(&a), "b\n");

    // b ¶ [table] c d ¶ e f ¶: a comment over the first paragraph would put its reference before the table.
    a.split_paragraph(MAIN, 1).unwrap();
    a.insert_table(MAIN, 2, 1, 1).unwrap();
    a.insert_text(MAIN, 3, "cdef").unwrap();
    a.split_paragraph(MAIN, 5).unwrap();
    assert_eq!(text(&a), "b\ncd\nef\n");
    let before = valid_view(&a);
    assert_eq!(
        a.add_comment(MAIN, 0..2, "Reviewer", "x"),
        Err(EditError::NotABlockPosition)
    );
    // A bookmark that starts or ends at the block position before the table.
    assert_eq!(
        a.insert_bookmark(MAIN, 2..3, "t"),
        Err(EditError::NotABlockPosition)
    );
    assert_eq!(
        a.insert_bookmark(MAIN, 0..2, "t"),
        Err(EditError::NotABlockPosition)
    );
    // Text moved to the block position before the table is refused, as typing there is, also when the cut makes the position one.
    assert_eq!(
        a.move_range(MAIN, 3..5, MAIN, 2),
        Err(EditError::NotABlockPosition)
    );
    assert_eq!(
        a.move_range(MAIN, 0..1, MAIN, 2),
        Err(EditError::NotABlockPosition)
    );
    assert_eq!(valid_view(&a), before);
    // A whole paragraph may go there.
    a.move_range(MAIN, 3..6, MAIN, 2).unwrap();
    assert_eq!(text(&a), "b\ncd\nef\n");
    let view = valid_view(&a);
    // "b" ¶ "cd" ¶ [table] "ef" ¶
    assert!(matches!(&view.main()[2], Item::Text { text, .. } if text == "cd"));
    assert!(view.main()[3].is(AtomKind::ParagraphEnd));
    assert!(view.main()[4].is(AtomKind::TableBlock));
}

/// Moving content to its own start or end changes nothing, and writes nothing.
#[test]
fn moving_a_range_onto_itself_writes_nothing() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    let changes = a.crdt().stats().changes;
    a.move_range(MAIN, 1..3, MAIN, 1).unwrap();
    a.move_range(MAIN, 1..3, MAIN, 3).unwrap();
    assert_eq!(a.crdt().stats().changes, changes);
    assert_eq!(text(&a), "abcdef\n");
}

/// Deleting only a comment's reference removes the comment from the view, and its highlight with it (review of CORE-004, item 13).
#[test]
fn a_comment_highlight_leaves_the_view_with_its_comment() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcde").unwrap();
    let comment = a.add_comment(MAIN, 1..4, "Reviewer", "note").unwrap();
    let key = marks::comment_key(comment);
    assert!(crate::common::marks_at(&valid_view(&a), MAIN, 1).contains_key(&key));
    // "a" "bcd" ⟦reference⟧ "e" ¶: delete the reference alone.
    a.delete(MAIN, 4..5).unwrap();
    let view = valid_view(&a);
    assert!(view.comments.is_empty());
    assert_eq!(
        view.main()[0],
        Item::Text {
            text: "abcde".to_owned(),
            marks: Default::default()
        }
    );
    // Undo brings the reference, the comment and its highlight back.
    assert!(a.undo().unwrap());
    let view = valid_view(&a);
    assert!(view.comments.contains_key(&comment));
    assert!(crate::common::marks_at(&view, MAIN, 1).contains_key(&key));
}
