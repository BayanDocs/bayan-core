//! Mark expansion as the document model specifies it (§5): run properties expand after their end; hyperlinks, comments (one key per comment, overlapping freely) and revisions never expand; paragraph-mark formatting never spreads.

use std::collections::{BTreeMap, BTreeSet};

use bayan_crdt::{Expand, Value};
use bayan_model::simulation::Rng;
use bayan_model::{EntityId, marks};

use crate::common::{document, marks_at, replica_of, sync_both, valid_view};

const MAIN: EntityId = EntityId::MAIN_STORY;

/// The expansion of a key, by its family (everything before the first colon).
fn expansion(key: &str) -> Expand {
    let family = key.split(':').next().unwrap_or(key);
    marks::FAMILIES
        .iter()
        .find(|candidate| candidate.name == family)
        .map_or(Expand::None, |candidate| candidate.expand)
}

/// The marks of every stored atom of the main story, by position.
fn raw_marks(document: &bayan_model::Document) -> Vec<BTreeMap<String, Value>> {
    let mut marks = Vec::new();
    for run in document.crdt().main_story().runs() {
        for _ in run.text.chars() {
            marks.push(run.marks.clone());
        }
    }
    marks
}

#[test]
fn run_properties_expand_after_their_end_only() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "0123456789").unwrap();
    a.format(MAIN, 2..5, marks::BOLD, &Value::Bool(true))
        .unwrap();
    a.format(MAIN, 2..5, "r:preserved", &Value::from("<w:foo/>"))
        .unwrap();
    a.insert_text(MAIN, 5, "E").unwrap();
    a.insert_text(MAIN, 2, "S").unwrap();
    a.insert_text(MAIN, 4, "M").unwrap();
    let view = valid_view(&a);
    assert_eq!(view.plain_text(MAIN), "01S2M34E56789\n");
    let bold = |pos| marks_at(&view, MAIN, pos).contains_key(marks::BOLD);
    // Typed after the end: bold. Typed before the start: not. Typed inside: bold.
    assert!(bold(7), "after the end");
    assert!(!bold(2), "before the start");
    assert!(bold(4), "inside");
    // Unknown run properties (preserved XML) expand the same way.
    assert!(marks_at(&view, MAIN, 7).contains_key("r:preserved"));
}

#[test]
fn hyperlinks_comments_and_revisions_never_expand() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "0123456789").unwrap();
    a.format(MAIN, 2..5, marks::LINK, &Value::from("https://example.org"))
        .unwrap();
    a.format(MAIN, 2..5, marks::INSERTED, &Value::from("author:1"))
        .unwrap();
    let comment = a.add_comment(MAIN, 2..5, "Reviewer", "note").unwrap();
    let key = marks::comment_key(comment);
    // The comment reference now stands at 5; type right after the range, before it, and inside.
    a.insert_text(MAIN, 5, "E").unwrap();
    a.insert_text(MAIN, 2, "S").unwrap();
    a.insert_text(MAIN, 4, "M").unwrap();
    let view = valid_view(&a);
    for family in [marks::LINK, marks::INSERTED, key.as_str()] {
        let marked = |pos| marks_at(&view, MAIN, pos).contains_key(family);
        assert!(!marked(2), "{family} expanded before its start");
        assert!(!marked(7), "{family} expanded after its end");
        assert!(marked(4), "{family} lost text typed inside it");
    }
}

/// The general rule, checked at every kind of boundary: a character typed at position `p` takes a mark of a non-expanding family only when both its neighbors have it (it is strictly inside), and a mark of an after-expanding family when its left neighbor has it. Two hundred overlapping comments, bold ranges and links, on one replica and then concurrently on two.
#[test]
fn many_overlapping_comments_follow_the_expansion_rules_at_every_boundary() {
    let mut rng = Rng::new(2026);
    let mut a = document(1);
    let text: String = (0..1_000)
        .map(|index| char::from(b'a' + u8::try_from(index % 26).unwrap()))
        .collect();
    a.insert_text(MAIN, 0, &text).unwrap();
    let mut comments = Vec::new();
    for _ in 0..200 {
        let len = a.story_len(MAIN).unwrap() - 1;
        let start = rng.below(len - 1);
        let end = (start + 1 + rng.below(120)).min(len);
        comments.push(a.add_comment(MAIN, start..end, "Reviewer", "c").unwrap());
    }
    for _ in 0..30 {
        let len = a.story_len(MAIN).unwrap() - 1;
        let start = rng.below(len - 1);
        let end = (start + 1 + rng.below(60)).min(len);
        let key = if rng.chance(1, 2) {
            marks::BOLD
        } else {
            marks::LINK
        };
        a.format(MAIN, start..end, key, &Value::from("x")).unwrap();
    }
    let mut b = replica_of(&a, 2);
    // Every comment covers what it was given: its key appears on one contiguous stretch of atoms.
    let stored = raw_marks(&a);
    for comment in &comments {
        let key = marks::comment_key(*comment);
        let positions: Vec<usize> = (0..stored.len())
            .filter(|pos| stored[*pos].contains_key(&key))
            .collect();
        assert!(!positions.is_empty());
        assert_eq!(
            positions.len(),
            positions[positions.len() - 1] - positions[0] + 1,
            "{key} is not contiguous"
        );
    }
    // Type at the boundaries of comments and at random places, on both replicas, checking each insertion against the rule.
    for round in 0..300 {
        let replica = if round % 2 == 0 { &mut a } else { &mut b };
        let before = raw_marks(replica);
        let len = before.len();
        let pos = if round % 3 == 0 {
            1 + rng.below(len - 2)
        } else {
            // Right at the start or right after the end of a comment.
            let comment = comments[rng.below(comments.len())];
            let key = marks::comment_key(comment);
            let covered: Vec<usize> = (0..len)
                .filter(|pos| before[*pos].contains_key(&key))
                .collect();
            match (covered.first(), covered.last()) {
                (Some(first), Some(_)) if rng.chance(1, 2) => (*first).max(1),
                (Some(_), Some(last)) => (last + 1).min(len - 1),
                _ => 1 + rng.below(len - 2),
            }
        };
        if replica.insert_text(MAIN, pos, "#").is_err() {
            continue;
        }
        let left = &before[pos - 1];
        let right = &before[pos];
        let typed = &raw_marks(replica)[pos];
        let keys: BTreeSet<&String> = left
            .keys()
            .chain(right.keys())
            .chain(typed.keys())
            .collect();
        for key in keys {
            if key == "atom" {
                continue;
            }
            let expected = match expansion(key) {
                Expand::None => left.get(key).is_some() && left.get(key) == right.get(key),
                Expand::After => left.contains_key(key),
                Expand::Before => right.contains_key(key),
                Expand::Both => left.contains_key(key) || right.contains_key(key),
            };
            assert_eq!(
                typed.contains_key(key),
                expected,
                "round {round}: key {key} at {pos}"
            );
        }
        if round % 50 == 49 {
            sync_both(&mut a, &mut b);
            assert_eq!(valid_view(&a), valid_view(&b));
        }
    }
    sync_both(&mut a, &mut b);
    assert_eq!(valid_view(&a), valid_view(&b));
}

#[test]
fn concurrent_formatting_and_typing_follow_the_families() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc def").unwrap();
    let mut b = replica_of(&a, 2);
    // A formats "abc" in three ways while B types right after the "c" and between "b" and "c".
    a.format(MAIN, 0..3, marks::BOLD, &Value::Bool(true))
        .unwrap();
    a.format(MAIN, 0..3, marks::LINK, &Value::from("https://example.org"))
        .unwrap();
    let comment = a.add_comment(MAIN, 0..3, "Reviewer", "c").unwrap();
    b.insert_text(MAIN, 3, "X").unwrap();
    b.insert_text(MAIN, 2, "Y").unwrap();
    sync_both(&mut a, &mut b);
    let view = valid_view(&a);
    assert_eq!(view, valid_view(&b));
    let text = view.plain_text(MAIN);
    let x = text.find('X').unwrap();
    let y = text.find('Y').unwrap();
    let key = marks::comment_key(comment);
    let at = |pos: usize| marks_at(&view, MAIN, pos);
    // After the end: bold continues, the link and the comment do not.
    assert!(at(x).contains_key(marks::BOLD));
    assert!(!at(x).contains_key(marks::LINK));
    assert!(!at(x).contains_key(&key));
    // Strictly inside: every mark covers it.
    assert!(
        at(y).contains_key(marks::BOLD)
            && at(y).contains_key(marks::LINK)
            && at(y).contains_key(&key)
    );
}

/// Why paragraph-mark formatting is not stored as run-property marks on the paragraph end: a run-property mark expands after its end, so when a paragraph is formatted together with its mark (a triple click), text typed at the start of the next paragraph inherits the formatting. This test pins the CRDT behavior that motivates the model's choice; the model itself does not bleed (see `a_paragraph_mark_formatted_with_its_text_stores_the_formatting_on_the_paragraph`).
#[test]
fn a_run_mark_ending_on_a_paragraph_end_would_bleed_into_the_next_paragraph() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "abc").unwrap();
    a.split_paragraph(MAIN, 3).unwrap();
    a.insert_text(MAIN, 4, "def").unwrap();
    // Through the adapter, mark the first paragraph including its end.
    let main = a.crdt().main_story();
    main.mark(0..4, marks::BOLD, &Value::Bool(true)).unwrap();
    main.insert(4, "X").unwrap();
    a.crdt().commit();
    let typed = &raw_marks(&a)[4];
    assert!(
        typed.contains_key(marks::BOLD),
        "the CRDT no longer expands run marks past a paragraph end; revisit the model's choice"
    );
}

#[test]
fn typing_next_to_an_atom_never_extends_its_binding() {
    let mut a = document(1);
    a.insert_text(MAIN, 0, "ab").unwrap();
    let object = a.insert_object(MAIN, 1).unwrap();
    a.insert_text(MAIN, 2, "x").unwrap();
    a.insert_text(MAIN, 1, "y").unwrap();
    let view = valid_view(&a);
    assert_eq!(view.plain_text(MAIN), "ay\u{FFFC}xb\n");
    assert_eq!(view.objects.keys().collect::<Vec<_>>(), [&object]);
}
