//! Tests of the invariant checker itself: every invariant, broken on its own in a view built by hand, is reported, and nothing else is. The property tests and the convergence runs judge the normalization with this checker, so a checker that missed a broken invariant would let them pass vacuously (review of CORE-004, item 19).

use std::collections::BTreeMap;

use bayan_crdt::{ATOM_KEY, Run, Value};
use bayan_model::{
    AtomKind, Cell, Comment, EntityId, Item, RawDocument, RawRow, RawTable, Row, Table, View,
    check_invariants, encode_binding, normalize, registry,
};

const MAIN: EntityId = EntityId::MAIN_STORY;
const FIELD: EntityId = EntityId(0xF);
const RANGE: EntityId = EntityId(0xE);
const OBJECT: EntityId = EntityId(0xD);
const COMMENT: EntityId = EntityId(0xC);
const COMMENT_STORY: EntityId = EntityId(0xC5);
const TABLE: EntityId = EntityId(0x7);
const ROW: EntityId = EntityId(0x71);
const CELL: EntityId = EntityId(0x72);
const CELL_STORY: EntityId = EntityId(0x73);
const PARAGRAPHS: [EntityId; 4] = [
    EntityId(0xA1),
    EntityId(0xA2),
    EntityId(0xA3),
    EntityId(0xA4),
];

fn atom(kind: AtomKind, id: EntityId) -> Run {
    Run {
        text: kind.placeholder().to_string(),
        marks: BTreeMap::from([(ATOM_KEY.to_owned(), Value::Str(encode_binding(kind, id)))]),
    }
}

fn text(text: &str, marks: &[(&str, Value)]) -> Run {
    Run {
        text: text.to_owned(),
        marks: marks
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    }
}

/// A valid view with one of everything: text with marks and a comment highlight, a field, a bookmark, an object, a comment, and a table whose cell holds text.
fn valid() -> View {
    let highlight = format!("cmt:{COMMENT}");
    let mut raw = RawDocument {
        main: vec![
            text("a", &[("r:b", Value::Bool(true))]),
            atom(AtomKind::FieldBegin, FIELD),
            text("PAGE", &[]),
            atom(AtomKind::FieldSeparator, FIELD),
            text("1", &[]),
            atom(AtomKind::FieldEnd, FIELD),
            atom(AtomKind::RangeStart, RANGE),
            text("b", &[(highlight.as_str(), Value::Bool(true))]),
            atom(AtomKind::RangeEnd, RANGE),
            atom(AtomKind::ObjectAnchor, OBJECT),
            atom(AtomKind::CommentReference, COMMENT),
            atom(AtomKind::ParagraphEnd, PARAGRAPHS[0]),
            atom(AtomKind::TableBlock, TABLE),
            text("c", &[]),
            atom(AtomKind::ParagraphEnd, PARAGRAPHS[1]),
        ],
        ..RawDocument::default()
    };
    raw.stories.insert(
        CELL_STORY,
        vec![text("x", &[]), atom(AtomKind::ParagraphEnd, PARAGRAPHS[2])],
    );
    raw.stories.insert(
        COMMENT_STORY,
        vec![
            text("note", &[]),
            atom(AtomKind::ParagraphEnd, PARAGRAPHS[3]),
        ],
    );
    for paragraph in PARAGRAPHS {
        raw.paragraphs.insert(paragraph, BTreeMap::new());
    }
    raw.fields.insert(FIELD, BTreeMap::new());
    raw.ranges.insert(RANGE, BTreeMap::new());
    raw.objects.insert(OBJECT, BTreeMap::new());
    raw.comments.insert(
        COMMENT,
        BTreeMap::from([(
            registry::STORY.to_owned(),
            Value::Str(COMMENT_STORY.to_string()),
        )]),
    );
    raw.tables.insert(
        TABLE,
        RawTable {
            props: BTreeMap::new(),
            rows: vec![Value::Str(ROW.to_string())],
        },
    );
    raw.rows.insert(
        ROW,
        RawRow {
            props: BTreeMap::new(),
            cells: vec![Value::Str(CELL.to_string())],
        },
    );
    raw.cells.insert(
        CELL,
        BTreeMap::from([(
            registry::STORY.to_owned(),
            Value::Str(CELL_STORY.to_string()),
        )]),
    );
    let (view, _) = normalize(&raw);
    assert!(
        check_invariants(&view).is_empty(),
        "{:?}",
        check_invariants(&view)
    );
    // Everything made it into the view.
    assert_eq!(
        (
            view.stories.len(),
            view.tables.len(),
            view.comments.len(),
            view.fields.len(),
            view.ranges.len(),
            view.objects.len()
        ),
        (3, 1, 1, 1, 1, 1)
    );
    view
}

/// The invariants the checker reports for `view`, each once, with the details for messages.
fn broken(view: &View) -> (Vec<&'static str>, String) {
    let violations = check_invariants(view);
    let mut invariants: Vec<&'static str> = violations
        .iter()
        .map(|violation| violation.invariant)
        .collect();
    invariants.sort_unstable();
    invariants.dedup();
    (invariants, format!("{violations:?}"))
}

fn assert_only(view: &View, invariant: &str, detail: &str) {
    let (invariants, details) = broken(view);
    assert_eq!(invariants, [invariant], "{details}");
    assert!(details.contains(detail), "expected `{detail}` in {details}");
}

fn main_items(view: &mut View) -> &mut Vec<Item> {
    view.stories.get_mut(&MAIN).expect("the main story")
}

fn position(view: &View, kind: AtomKind) -> usize {
    view.main()
        .iter()
        .position(|item| item.is(kind))
        .expect("the atom")
}

#[test]
fn i1_a_story_without_its_final_paragraph_end() {
    let mut view = valid();
    main_items(&mut view).pop();
    view.paragraphs.remove(&PARAGRAPHS[1]);
    assert_only(&view, "I1", "does not end with a paragraph end");
}

#[test]
fn i2_a_field_that_is_not_closed() {
    let mut view = valid();
    let end = position(&view, AtomKind::FieldEnd);
    main_items(&mut view).remove(end);
    assert_only(&view, "I2", "not closed");
}

#[test]
fn i2_a_separator_outside_its_field() {
    let mut view = valid();
    // The separator before its field's begin: "a" ⟦separator⟧ "PAGE" ⟦begin⟧ "1" ⟦end⟧ …
    let begin = position(&view, AtomKind::FieldBegin);
    let separator = position(&view, AtomKind::FieldSeparator);
    main_items(&mut view).swap(begin, separator);
    assert_only(&view, "I2", "does not belong to the innermost open field");
}

#[test]
fn i3_a_range_that_ends_before_it_starts() {
    let mut view = valid();
    let start = position(&view, AtomKind::RangeStart);
    let end = position(&view, AtomKind::RangeEnd);
    main_items(&mut view).swap(start, end);
    assert_only(&view, "I3", "has no earlier start");
}

#[test]
fn i4_a_table_in_the_middle_of_a_paragraph() {
    let mut view = valid();
    let table = position(&view, AtomKind::TableBlock);
    main_items(&mut view).insert(
        table,
        Item::Text {
            text: "z".to_owned(),
            marks: BTreeMap::new(),
        },
    );
    assert_only(&view, "I4", "not at a block position");
}

#[test]
fn i5_an_atom_whose_entity_is_missing() {
    let mut view = valid();
    view.objects.clear();
    assert_only(&view, "I5", "references no entity");
}

#[test]
fn i5_an_entity_referenced_twice() {
    let mut view = valid();
    let object = position(&view, AtomKind::ObjectAnchor);
    let copy = view.main()[object].clone();
    main_items(&mut view).insert(object, copy);
    assert_only(&view, "I5", "appears 2 times");
}

#[test]
fn i5_a_comment_highlight_without_its_comment() {
    let mut view = valid();
    let missing = format!("cmt:{}", EntityId(0x99));
    let Item::Text { marks, .. } = &mut main_items(&mut view)[0] else {
        panic!("the main story starts with text");
    };
    marks.insert(missing, Value::Bool(true));
    assert_only(&view, "I5", "names no comment of the view");
}

#[test]
fn i6_a_row_without_cells() {
    let mut view = valid();
    view.rows.get_mut(&ROW).expect("the row").cells.clear();
    view.cells.remove(&CELL);
    view.stories.remove(&CELL_STORY);
    view.paragraphs.remove(&PARAGRAPHS[2]);
    assert_only(&view, "I6", "has no cells");
}

#[test]
fn i6_a_row_in_two_tables() {
    let mut view = valid();
    let mut second = view.tables[&TABLE].clone();
    second.rows = vec![ROW];
    // A second table that lists the same row, referenced from a paragraph of its own.
    let table = EntityId(0x8);
    view.tables.insert(table, second);
    let paragraph = EntityId(0xA9);
    view.paragraphs.insert(paragraph, BTreeMap::new());
    let end = main_items(&mut view).len() - 1;
    main_items(&mut view).insert(
        end + 1,
        Item::Atom {
            kind: AtomKind::TableBlock,
            id: Some(table),
            marks: BTreeMap::new(),
        },
    );
    main_items(&mut view).push(Item::Atom {
        kind: AtomKind::ParagraphEnd,
        id: Some(paragraph),
        marks: BTreeMap::new(),
    });
    assert_only(&view, "I6", "belongs to 2 parents");
}

#[test]
fn i7_a_section_without_a_valid_page_size_or_margin() {
    for (key, value) in [
        ("pgSz.w", Value::Int(0)),
        ("pgSz.h", Value::from("tall")),
        ("pgMar.left", Value::Int(-1)),
    ] {
        let mut view = valid();
        view.section.insert(key.to_owned(), value);
        assert_only(&view, "I7", key);
    }
    // Top and bottom margins may be negative, and left and right ones 0 (OOXML).
    let mut view = valid();
    view.section
        .insert("pgMar.top".to_owned(), Value::Int(-720));
    view.section.insert("pgMar.right".to_owned(), Value::Int(0));
    assert!(check_invariants(&view).is_empty());
}

#[test]
fn form_text_that_is_empty_holds_a_control_character_or_repeats_its_marks() {
    for (items, detail) in [
        (
            vec![Item::Text {
                text: String::new(),
                marks: BTreeMap::new(),
            }],
            "empty text",
        ),
        (
            vec![Item::Text {
                text: "a\u{0D}b".to_owned(),
                marks: BTreeMap::new(),
            }],
            "control character",
        ),
        (
            vec![
                Item::Text {
                    text: "a".to_owned(),
                    marks: BTreeMap::new(),
                },
                Item::Text {
                    text: "b".to_owned(),
                    marks: BTreeMap::new(),
                },
            ],
            "same marks",
        ),
    ] {
        let mut view = valid();
        let comment = view.comments[&COMMENT].story;
        let story = view.stories.get_mut(&comment).expect("the comment story");
        for (index, item) in items.into_iter().enumerate() {
            story.insert(index, item);
        }
        // The comment story starts with "note"; the inserted items must not merge with it.
        story.insert(
            story.len() - 1,
            Item::Atom {
                kind: AtomKind::Tab,
                id: None,
                marks: BTreeMap::new(),
            },
        );
        let (invariants, details) = broken(&view);
        assert_eq!(invariants, ["form"], "{detail}: {details}");
        assert!(details.contains(detail), "{detail}: {details}");
    }
}

#[test]
fn form_marks_on_an_atom_that_keeps_none() {
    let mut view = valid();
    let end = position(&view, AtomKind::ParagraphEnd);
    let Item::Atom { marks, .. } = &mut main_items(&mut view)[end] else {
        unreachable!("an atom")
    };
    marks.insert("r:b".to_owned(), Value::Bool(true));
    assert_only(&view, "form", "has marks");
}

#[test]
fn form_marks_with_the_atom_binding_or_no_supported_value() {
    for (key, value, detail) in [
        (ATOM_KEY, Value::from("p:1"), "the atom binding is a mark"),
        ("link", Value::Null, "no supported value"),
        ("r:sz", Value::Unsupported, "no supported value"),
    ] {
        let mut view = valid();
        let Item::Text { marks, .. } = &mut main_items(&mut view)[0] else {
            panic!("the main story starts with text");
        };
        marks.insert(key.to_owned(), value);
        assert_only(&view, "form", detail);
    }
}

#[test]
fn form_a_story_with_two_owners() {
    let mut view = valid();
    // The comment's story also becomes the cell's story; the cell's own story is gone.
    view.cells.get_mut(&CELL).expect("the cell").story = COMMENT_STORY;
    view.stories.remove(&CELL_STORY);
    view.paragraphs.remove(&PARAGRAPHS[2]);
    assert_only(&view, "form", "has 2 owners");
}

#[test]
fn form_two_tables_nested_in_each_other_and_unreachable() {
    let mut view = valid();
    // Tables 0x20 and 0x30, each holding the other in its only cell, referenced from nowhere else.
    for (table, other) in [
        (EntityId(0x20), EntityId(0x30)),
        (EntityId(0x30), EntityId(0x20)),
    ] {
        let [row, cell, story, paragraph] = [1, 2, 3, 4].map(|offset| EntityId(table.0 + offset));
        view.tables.insert(
            table,
            Table {
                props: BTreeMap::new(),
                rows: vec![row],
            },
        );
        view.rows.insert(
            row,
            Row {
                props: BTreeMap::new(),
                cells: vec![cell],
            },
        );
        view.cells.insert(
            cell,
            Cell {
                props: BTreeMap::new(),
                story,
            },
        );
        view.paragraphs.insert(paragraph, BTreeMap::new());
        view.stories.insert(
            story,
            vec![
                Item::Atom {
                    kind: AtomKind::TableBlock,
                    id: Some(other),
                    marks: BTreeMap::new(),
                },
                Item::Atom {
                    kind: AtomKind::ParagraphEnd,
                    id: Some(paragraph),
                    marks: BTreeMap::new(),
                },
            ],
        );
    }
    assert_only(&view, "form", "cannot be reached from the main story");
}

#[test]
fn form_a_comment_referenced_only_from_its_own_story() {
    let mut view = valid();
    let comment = EntityId(0x40);
    let story = EntityId(0x41);
    let paragraph = EntityId(0x42);
    view.comments.insert(
        comment,
        Comment {
            props: BTreeMap::new(),
            story,
        },
    );
    view.paragraphs.insert(paragraph, BTreeMap::new());
    view.stories.insert(
        story,
        vec![
            Item::Atom {
                kind: AtomKind::CommentReference,
                id: Some(comment),
                marks: BTreeMap::new(),
            },
            Item::Atom {
                kind: AtomKind::ParagraphEnd,
                id: Some(paragraph),
                marks: BTreeMap::new(),
            },
        ],
    );
    assert_only(&view, "form", "cannot be reached from the main story");
}

// The checks below had no negative test, so deleting any of them left every suite green (review of CORE-004, second round, item A4).

#[test]
fn i6_a_table_without_rows() {
    let mut view = valid();
    view.tables.get_mut(&TABLE).expect("the table").rows.clear();
    view.rows.remove(&ROW);
    view.cells.remove(&CELL);
    view.stories.remove(&CELL_STORY);
    view.paragraphs.remove(&PARAGRAPHS[2]);
    assert_only(&view, "I6", "has no rows");
}

#[test]
fn i6_a_cell_without_its_story() {
    let mut view = valid();
    view.cells.get_mut(&CELL).expect("the cell").story = EntityId(0x999);
    view.stories.remove(&CELL_STORY);
    view.paragraphs.remove(&PARAGRAPHS[2]);
    // The cell has no story (I6), and the story it names is owned but missing (form).
    let (invariants, details) = broken(&view);
    assert_eq!(invariants, ["I6", "form"], "{details}");
    assert!(details.contains("has no story"), "{details}");
    assert!(details.contains("is owned but missing"), "{details}");
}

#[test]
fn form_a_comment_whose_story_is_missing() {
    let mut view = valid();
    view.stories.remove(&COMMENT_STORY);
    view.paragraphs.remove(&PARAGRAPHS[3]);
    assert_only(&view, "form", "is owned but missing");
}

#[test]
fn i3_a_range_start_without_its_end() {
    let mut view = valid();
    let end = position(&view, AtomKind::RangeEnd);
    main_items(&mut view).remove(end);
    assert_only(&view, "I3", "not referenced exactly once as RangeEnd");
}

#[test]
fn i5_an_atom_with_the_wrong_kind_of_reference() {
    let mut view = valid();
    // A tab references no entity; this one names one.
    main_items(&mut view).insert(
        1,
        Item::Atom {
            kind: AtomKind::Tab,
            id: Some(EntityId(0x55)),
            marks: BTreeMap::new(),
        },
    );
    assert_only(&view, "I5", "wrong kind of reference");
}
