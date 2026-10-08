//! Undo reverts only this replica's operations, and composes with concurrent remote edits (ADR-0008 §5).

use bayan_crdt::Value;
use bayan_model::simulation::Rng;
use bayan_model::{EntityId, View, marks};

use crate::common::{document, marks_at, replica_of, sync, sync_both, text, valid_view};

const MAIN: EntityId = EntityId::MAIN_STORY;

#[test]
fn undo_reverts_only_local_typing() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "local").unwrap();
    let mut b = replica_of(&a, 2);
    b.insert_text(MAIN, 5, " remote").unwrap();
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "local remote\n");
    assert!(a.undo().unwrap());
    assert_eq!(text(&a), " remote\n");
    // Creating the document is not an undo step, and B's typing is not A's to undo.
    assert!(!a.undo().unwrap());
    sync_both(&mut a, &mut b);
    assert_eq!(text(&b), " remote\n");
    assert!(a.redo().unwrap());
    sync_both(&mut a, &mut b);
    assert_eq!(text(&b), "local remote\n");
    assert_eq!(valid_view(&a), valid_view(&b));
}

/// Undoing formatting reverts it on the characters it was applied to. Text another replica typed inside the range concurrently took the formatting (it is strictly inside the mark), and Loro 1.16.2's undo leaves it there: the inverse is computed for the original characters only. Both replicas still converge. This pins the behavior the CORE-004 report describes; an editing layer that wants "undo formatting" to cover such text has to re-apply the previous formatting over the range's current extent.
#[test]
fn undoing_formatting_leaves_it_on_text_typed_inside_concurrently() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    let mut b = replica_of(&a, 2);
    a.format(MAIN, 0..6, marks::BOLD, &Value::Bool(true))
        .unwrap();
    b.insert_text(MAIN, 3, "XY").unwrap();
    sync_both(&mut a, &mut b);
    let view = valid_view(&a);
    assert_eq!(view.plain_text(MAIN), "abcXYdef\n");
    // Typed strictly inside the bold range, so bold.
    assert!(marks_at(&view, MAIN, 3).contains_key(marks::BOLD));
    assert!(a.undo().unwrap());
    sync_both(&mut a, &mut b);
    let view = valid_view(&b);
    assert_eq!(view, valid_view(&a));
    let bold: Vec<bool> = (0..8)
        .map(|pos| marks_at(&view, MAIN, pos).contains_key(marks::BOLD))
        .collect();
    assert_eq!(bold, [false, false, false, true, true, false, false, false]);
}

#[test]
fn undoing_a_split_keeps_text_typed_concurrently_in_the_new_paragraph() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "hello world").unwrap();
    a.split_paragraph(MAIN, 5).unwrap();
    let mut b = replica_of(&a, 2);
    b.insert_text(MAIN, 7, "X").unwrap();
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "hello\n Xworld\n");
    assert!(a.undo().unwrap());
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "hello Xworld\n");
    assert_eq!(valid_view(&a), valid_view(&b));
}

/// The risk of binding atoms to entities with a mark: if undoing a deletion brought back the placeholder characters but not their bindings, every table, comment, field and object in the deleted range would disappear on undo. It does not: the view after undo is exactly the view before the deletion.
#[test]
fn undoing_a_delete_restores_every_atom_with_its_entity() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "Title and some body text").unwrap();
    a.split_paragraph(MAIN, 5).unwrap();
    a.format(MAIN, 6..10, marks::ITALIC, &Value::Bool(true))
        .unwrap();
    a.add_comment(MAIN, 6..9, "Reviewer", "Check this").unwrap();
    a.insert_field(MAIN, 12, "DATE", "today").unwrap();
    a.insert_object(MAIN, 2).unwrap();
    a.insert_bookmark(MAIN, 0..3, "top").unwrap();
    let table_at = a.story_len(MAIN).unwrap() - 1;
    // Make the last paragraph empty so that the table can stand at a block position before it.
    a.split_paragraph(MAIN, table_at).unwrap();
    let table = a.insert_table(MAIN, table_at + 1, 2, 2).unwrap();
    let before = valid_view(&a);
    assert!(before.tables.contains_key(&table));
    // Delete everything but the final paragraph end.
    let len = a.story_len(MAIN).unwrap();
    a.delete(MAIN, 0..len - 1).unwrap();
    let emptied = valid_view(&a);
    assert_eq!(emptied.plain_text(MAIN), "\n");
    assert!(emptied.tables.is_empty() && emptied.comments.is_empty() && emptied.fields.is_empty());
    assert!(a.undo().unwrap());
    assert_eq!(valid_view(&a), before);
    // And the same through another replica that receives the deletion and the undo.
    let b = replica_of(&a, 2);
    assert_eq!(valid_view(&b), before);
}

#[test]
fn undoing_a_row_insertion_hides_a_concurrent_edit_in_it_and_redo_brings_both_back() {
    let mut a = document(1);
    let table = a.insert_table(MAIN, 0, 1, 1).unwrap();
    let row = a.insert_row(table, 1).unwrap();
    let mut b = replica_of(&a, 2);
    let view = valid_view(&b);
    let cell_story = view.cells[&view.rows[&row].cells[0]].story;
    b.insert_text(cell_story, 0, "typed by B").unwrap();
    sync_both(&mut a, &mut b);
    assert_eq!(valid_view(&a).plain_text(cell_story), "typed by B\n");
    assert!(a.undo().unwrap());
    sync_both(&mut a, &mut b);
    let view = valid_view(&b);
    assert_eq!(view.tables[&table].rows.len(), 1);
    assert!(!view.stories.contains_key(&cell_story));
    assert!(a.redo().unwrap());
    sync_both(&mut a, &mut b);
    let view = valid_view(&b);
    assert_eq!(view.tables[&table].rows, [view.tables[&table].rows[0], row]);
    assert_eq!(view.plain_text(cell_story), "typed by B\n");
}

#[test]
fn undoing_typing_that_another_replica_already_deleted_is_harmless() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    let mut b = replica_of(&a, 2);
    b.delete(MAIN, 0..3).unwrap();
    sync(&b, &mut a);
    assert_eq!(text(&a), "\n");
    a.undo().unwrap();
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "\n");
    assert_eq!(valid_view(&a), valid_view(&b));
}

/// Undo steps back through exactly the views a single replica went through, and redo forward again. An operation that changes nothing (bolding text that is already bold) is not an undo step.
#[test]
fn undo_and_redo_retrace_every_view() {
    let mut rng = Rng::new(42);
    let mut a = document(1);
    let mut views: Vec<View> = vec![valid_view(&a)];
    while views.len() < 40 {
        let steps = a.undo_count();
        let len = a.story_len(MAIN).unwrap();
        let pos = rng.below(len);
        let result = match rng.below(6) {
            0 | 1 => a.insert_text(MAIN, pos, ["ab", "\u{0628}", " x "][rng.below(3)]),
            2 => a.split_paragraph(MAIN, pos).map(|_| ()),
            3 if len > 2 => {
                let start = rng.below(len - 1);
                a.delete(MAIN, start..(start + 1 + rng.below(3)).min(len - 1))
            }
            4 => a.format(
                MAIN,
                pos..(pos + 3).min(len),
                marks::BOLD,
                &Value::Bool(true),
            ),
            _ => a.insert_table(MAIN, pos, 1, 1).map(|_| ()),
        };
        if result.is_ok() && a.undo_count() > steps {
            views.push(valid_view(&a));
        }
    }
    for expected in views.iter().rev().skip(1) {
        assert!(a.undo().unwrap());
        assert_eq!(&valid_view(&a), expected);
    }
    assert!(!a.undo().unwrap());
    for expected in views.iter().skip(1) {
        assert!(a.redo().unwrap());
        assert_eq!(&valid_view(&a), expected);
    }
}

/// Pins how Loro 1.16.2's undo composes with text another replica typed inside a deleted range (review of CORE-004, item 24): undoing the delete restores the deleted text as one block, before the concurrent insertion, not around it ("abcdeYf" rather than "abcYdef"). Both replicas converge; an editing layer that wants the text restored around the insertion has to do it itself.
#[test]
fn undoing_a_delete_restores_the_text_before_what_another_replica_typed_inside_it() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    let mut b = replica_of(&a, 2);
    a.delete(MAIN, 2..5).unwrap();
    b.insert_text(MAIN, 3, "Y").unwrap();
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "abYf\n");
    assert!(a.undo().unwrap());
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "abcdeYf\n");
    assert_eq!(valid_view(&a), valid_view(&b));
}

/// Pins how a move composes with text typed inside the moved range concurrently (review of CORE-004, item 24): a move is a cut and a paste of the same content, so text another replica typed inside the range stays where the range was ("Zdefabc"), and undoing the move pastes the range back before it ("abcZdef"). Both replicas converge.
#[test]
fn text_typed_inside_a_moved_range_stays_behind_and_undo_restores_the_range_before_it() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abcdef").unwrap();
    let mut b = replica_of(&a, 2);
    a.move_range(MAIN, 0..3, MAIN, 6).unwrap();
    b.insert_text(MAIN, 1, "Z").unwrap();
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "Zdefabc\n");
    assert!(a.undo().unwrap());
    sync_both(&mut a, &mut b);
    assert_eq!(text(&a), "abcZdef\n");
    assert_eq!(valid_view(&a), valid_view(&b));
}
