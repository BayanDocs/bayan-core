//! The invariants I1–I7 of a well-formed model (document model §14), checked on a view.
//!
//! The checker is written independently of the normalization, so that the property tests can use it to judge the normalization's output.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use bayan_crdt::{ATOM_KEY, Value};

use crate::atoms::is_text_character;
use crate::normalize::SECTION_DEFAULTS;
use crate::view::{Item, View};
use crate::{AtomKind, EntityId, Props, marks};

/// One broken invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The invariant (`I1` … `I7`, or `form` for the view's canonical form).
    pub invariant: &'static str,
    /// What is wrong, and where.
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.invariant, self.detail)
    }
}

/// Every invariant the view breaks (empty when it is well formed):
///
/// - **I1** every story ends with a paragraph end;
/// - **I2** field delimiters nest properly (begin, optional separator, end), also inside codes and results;
/// - **I3** each range has exactly one start and one end, the end after the start, in one story;
/// - **I4** block-level atoms stand only at block positions;
/// - **I5** every atom references an entity of the view, and every paragraph, object, comment, table, field and range of the view is referenced exactly once (in its role); every comment highlight (`cmt:<id>`) names a comment of the view;
/// - **I6** every table has at least one row and every row at least one cell; every row belongs to one table, every cell to one row, and every cell's story exists;
/// - **I7** the final section has its page size and margins, as integers: page sizes positive, left and right margins not negative;
/// - **form** the view's canonical form: no empty or placeholder text, adjacent text items differ in their marks, atoms that keep no marks have none, no marks hold the atom binding or a null or unsupported value, every story belongs to exactly one owner (the main story, a cell or a comment), and every story can be reached from the main story through table and comment references.
#[must_use]
pub fn check_invariants(view: &View) -> Vec<Violation> {
    let mut checker = Checker::default();
    for (story, items) in &view.stories {
        checker.story(*story, items);
        checker.marks(view, *story, items);
    }
    checker.references(view);
    checker.tables(view);
    checker.owners(view);
    checker.reachable(view);
    checker.section(view);
    checker.violations
}

/// Whether the view keeps the marks of atoms of `kind`: those with a glyph or an anchor do; paragraph ends (whose formatting lives in the paragraph's properties), range delimiters and tables do not. Written out here rather than taken from the model, so that the checker judges the normalization independently.
const fn keeps_marks(kind: AtomKind) -> bool {
    match kind {
        AtomKind::Tab
        | AtomKind::FieldBegin
        | AtomKind::FieldSeparator
        | AtomKind::FieldEnd
        | AtomKind::ObjectAnchor
        | AtomKind::CommentReference => true,
        AtomKind::ParagraphEnd
        | AtomKind::RangeStart
        | AtomKind::RangeEnd
        | AtomKind::TableBlock => false,
    }
}

#[derive(Default)]
struct Checker {
    violations: Vec<Violation>,
    /// How often each (role, entity) appears in the view.
    references: BTreeMap<(AtomKind, EntityId), usize>,
}

impl Checker {
    fn fail(&mut self, invariant: &'static str, detail: String) {
        self.violations.push(Violation { invariant, detail });
    }

    fn story(&mut self, story: EntityId, items: &[Item]) {
        if !items
            .last()
            .is_some_and(|item| item.is(AtomKind::ParagraphEnd))
        {
            self.fail(
                "I1",
                format!("story {story} does not end with a paragraph end"),
            );
        }
        let mut fields: Vec<(EntityId, bool)> = Vec::new();
        let mut range_starts: BTreeSet<EntityId> = BTreeSet::new();
        let mut previous: Option<&Item> = None;
        for (index, item) in items.iter().enumerate() {
            match item {
                Item::Text { text, marks } => {
                    if text.is_empty() {
                        self.fail("form", format!("story {story} has empty text at {index}"));
                    }
                    if let Some(character) = text
                        .chars()
                        .find(|character| !is_text_character(*character))
                    {
                        self.fail(
                            "form",
                            format!("story {story} has the control character U+{:04X} in its text at {index}", u32::from(character)),
                        );
                    }
                    if let Some(Item::Text {
                        marks: previous_marks,
                        ..
                    }) = previous
                        && previous_marks == marks
                    {
                        self.fail("form", format!("story {story} has adjacent text items with the same marks at {index}"));
                    }
                }
                Item::Atom { kind, id, marks } => {
                    if kind.has_entity() != id.is_some() {
                        self.fail("I5", format!("story {story}: atom {kind:?} at {index} has the wrong kind of reference"));
                    }
                    if !keeps_marks(*kind) && !marks.is_empty() {
                        self.fail(
                            "form",
                            format!("story {story}: atom {kind:?} at {index} has marks"),
                        );
                    }
                    if kind.is_block() {
                        let at_block_position = previous.is_none_or(|previous| {
                            previous.kind().is_some_and(|kind| {
                                kind == AtomKind::ParagraphEnd || kind.is_block()
                            })
                        });
                        if !at_block_position {
                            self.fail("I4", format!("story {story}: block-level atom at {index} is not at a block position"));
                        }
                    }
                    if let Some(id) = id {
                        *self.references.entry((*kind, *id)).or_insert(0) += 1;
                        match kind {
                            AtomKind::FieldBegin => fields.push((*id, false)),
                            AtomKind::FieldSeparator => match fields.last_mut() {
                                Some((top, separated @ false)) if top == id => *separated = true,
                                _ => self.fail("I2", format!("story {story}: field separator at {index} does not belong to the innermost open field")),
                            },
                            AtomKind::FieldEnd => match fields.last() {
                                Some((top, _)) if top == id => {
                                    fields.pop();
                                }
                                _ => self.fail("I2", format!("story {story}: field end at {index} does not close the innermost open field")),
                            },
                            AtomKind::RangeStart => {
                                range_starts.insert(*id);
                            }
                            AtomKind::RangeEnd if !range_starts.contains(id) => {
                                self.fail("I3", format!("story {story}: range end at {index} has no earlier start in the story"));
                            }
                            _ => {}
                        }
                    }
                }
            }
            previous = Some(item);
        }
        if !fields.is_empty() {
            self.fail(
                "I2",
                format!("story {story}: {} field(s) are not closed", fields.len()),
            );
        }
    }

    /// The marks of a story's items: no atom binding, no null or unsupported values (form), and every comment highlight names a comment of the view (I5).
    fn marks(&mut self, view: &View, story: EntityId, items: &[Item]) {
        for (index, item) in items.iter().enumerate() {
            let (Item::Text { marks, .. } | Item::Atom { marks, .. }) = item;
            let problems = mark_problems(view, marks);
            for (invariant, problem) in problems {
                self.fail(invariant, format!("story {story} at {index}: {problem}"));
            }
        }
    }

    /// Every story can be reached from the main story, through the tables and comments that its atoms reference, and through theirs.
    fn reachable(&mut self, view: &View) {
        let mut reached: BTreeSet<EntityId> = BTreeSet::new();
        let mut pending = vec![EntityId::MAIN_STORY];
        while let Some(story) = pending.pop() {
            if !reached.insert(story) {
                continue;
            }
            for item in view.stories.get(&story).map_or(&[][..], Vec::as_slice) {
                match item {
                    Item::Atom {
                        kind: AtomKind::TableBlock,
                        id: Some(table),
                        ..
                    } => {
                        let rows = view
                            .tables
                            .get(table)
                            .map_or(&[][..], |table| table.rows.as_slice());
                        for row in rows {
                            let cells = view
                                .rows
                                .get(row)
                                .map_or(&[][..], |row| row.cells.as_slice());
                            pending.extend(
                                cells
                                    .iter()
                                    .filter_map(|cell| view.cells.get(cell))
                                    .map(|cell| cell.story),
                            );
                        }
                    }
                    Item::Atom {
                        kind: AtomKind::CommentReference,
                        id: Some(comment),
                        ..
                    } => pending.extend(view.comments.get(comment).map(|comment| comment.story)),
                    _ => {}
                }
            }
        }
        let unreachable: Vec<EntityId> = view
            .stories
            .keys()
            .filter(|story| !reached.contains(story))
            .copied()
            .collect();
        for story in unreachable {
            self.fail(
                "form",
                format!("story {story} cannot be reached from the main story"),
            );
        }
    }

    /// I3 (one start and one end per range) and I5 (references and entities match one to one).
    fn references(&mut self, view: &View) {
        let count = |references: &BTreeMap<(AtomKind, EntityId), usize>, kind, id| {
            references.get(&(kind, id)).copied().unwrap_or(0)
        };
        let mut problems = Vec::new();
        for ((kind, id), times) in &self.references {
            let exists = match kind {
                AtomKind::ParagraphEnd => view.paragraphs.contains_key(id),
                AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd => {
                    view.fields.contains_key(id)
                }
                AtomKind::ObjectAnchor => view.objects.contains_key(id),
                AtomKind::RangeStart | AtomKind::RangeEnd => view.ranges.contains_key(id),
                AtomKind::TableBlock => view.tables.contains_key(id),
                AtomKind::CommentReference => view.comments.contains_key(id),
                AtomKind::Tab => true,
            };
            if !exists {
                problems.push((
                    "I5",
                    format!("{kind:?} {id} references no entity of the view"),
                ));
            }
            if *times != 1 {
                let invariant = if matches!(kind, AtomKind::RangeStart | AtomKind::RangeEnd) {
                    "I3"
                } else {
                    "I5"
                };
                problems.push((invariant, format!("{kind:?} {id} appears {times} times")));
            }
        }
        let expect_once = |problems: &mut Vec<(&'static str, String)>,
                           kind: AtomKind,
                           ids: &mut dyn Iterator<Item = EntityId>,
                           invariant: &'static str| {
            for id in ids {
                if count(&self.references, kind, id) != 1 {
                    problems.push((
                        invariant,
                        format!("{id} is not referenced exactly once as {kind:?}"),
                    ));
                }
            }
        };
        expect_once(
            &mut problems,
            AtomKind::ParagraphEnd,
            &mut view.paragraphs.keys().copied(),
            "I5",
        );
        expect_once(
            &mut problems,
            AtomKind::ObjectAnchor,
            &mut view.objects.keys().copied(),
            "I5",
        );
        expect_once(
            &mut problems,
            AtomKind::CommentReference,
            &mut view.comments.keys().copied(),
            "I5",
        );
        expect_once(
            &mut problems,
            AtomKind::TableBlock,
            &mut view.tables.keys().copied(),
            "I5",
        );
        expect_once(
            &mut problems,
            AtomKind::FieldBegin,
            &mut view.fields.keys().copied(),
            "I2",
        );
        expect_once(
            &mut problems,
            AtomKind::FieldEnd,
            &mut view.fields.keys().copied(),
            "I2",
        );
        expect_once(
            &mut problems,
            AtomKind::RangeStart,
            &mut view.ranges.keys().copied(),
            "I3",
        );
        expect_once(
            &mut problems,
            AtomKind::RangeEnd,
            &mut view.ranges.keys().copied(),
            "I3",
        );
        for (invariant, detail) in problems {
            self.fail(invariant, detail);
        }
    }

    /// I6.
    fn tables(&mut self, view: &View) {
        let mut row_owners: BTreeMap<EntityId, usize> = BTreeMap::new();
        let mut cell_owners: BTreeMap<EntityId, usize> = BTreeMap::new();
        let mut problems = Vec::new();
        for (id, table) in &view.tables {
            if table.rows.is_empty() {
                problems.push(format!("table {id} has no rows"));
            }
            for row in &table.rows {
                *row_owners.entry(*row).or_insert(0) += 1;
                match view.rows.get(row) {
                    None => problems.push(format!("table {id} lists the missing row {row}")),
                    Some(entry) => {
                        if entry.cells.is_empty() {
                            problems.push(format!("row {row} has no cells"));
                        }
                        for cell in &entry.cells {
                            *cell_owners.entry(*cell).or_insert(0) += 1;
                            match view.cells.get(cell) {
                                None => problems
                                    .push(format!("row {row} lists the missing cell {cell}")),
                                Some(entry) if !view.stories.contains_key(&entry.story) => {
                                    problems.push(format!("cell {cell} has no story"));
                                }
                                Some(_) => {}
                            }
                        }
                    }
                }
            }
        }
        for (kind, owners, all) in [
            (
                "row",
                &row_owners,
                view.rows.keys().copied().collect::<Vec<_>>(),
            ),
            (
                "cell",
                &cell_owners,
                view.cells.keys().copied().collect::<Vec<_>>(),
            ),
        ] {
            for id in all {
                let times = owners.get(&id).copied().unwrap_or(0);
                if times != 1 {
                    problems.push(format!("{kind} {id} belongs to {times} parents"));
                }
            }
        }
        for detail in problems {
            self.fail("I6", detail);
        }
    }

    /// Every story belongs to exactly one owner.
    fn owners(&mut self, view: &View) {
        let mut owners: BTreeMap<EntityId, usize> = BTreeMap::from([(EntityId::MAIN_STORY, 1)]);
        for story in view
            .cells
            .values()
            .map(|cell| cell.story)
            .chain(view.comments.values().map(|comment| comment.story))
        {
            *owners.entry(story).or_insert(0) += 1;
        }
        let mut problems = Vec::new();
        for story in view.stories.keys() {
            let times = owners.get(story).copied().unwrap_or(0);
            if times != 1 {
                problems.push(format!("story {story} has {times} owners"));
            }
        }
        for (story, _) in owners
            .iter()
            .filter(|(story, _)| !view.stories.contains_key(story))
        {
            problems.push(format!("story {story} is owned but missing"));
        }
        for detail in problems {
            self.fail("form", detail);
        }
    }

    /// I7.
    fn section(&mut self, view: &View) {
        for (key, _) in SECTION_DEFAULTS {
            // Page sizes are positive and left and right margins are not negative; top and bottom margins may be negative (OOXML).
            let lowest = match key {
                "pgSz.w" | "pgSz.h" => 1,
                "pgMar.left" | "pgMar.right" => 0,
                _ => i64::MIN,
            };
            if !view
                .section
                .get(key)
                .and_then(Value::as_int)
                .is_some_and(|value| value >= lowest)
            {
                self.fail("I7", format!("the final section has no valid `{key}`"));
            }
        }
    }
}

/// What is wrong with a set of marks, if anything: the atom binding, which only stored placeholders carry; null or unsupported values, which normalization removes; and comment highlights of comments that are not in the view.
fn mark_problems(view: &View, marks: &Props) -> Vec<(&'static str, String)> {
    let mut problems = Vec::new();
    for (key, value) in marks {
        if key == ATOM_KEY {
            problems.push(("form", "the atom binding is a mark".to_owned()));
        }
        if value.is_null() || *value == Value::Unsupported {
            problems.push(("form", format!("the mark `{key}` has no supported value")));
        }
        if key.split(':').next() == Some(marks::COMMENT)
            && !marks::comment_of_key(key).is_some_and(|id| view.comments.contains_key(&id))
        {
            problems.push((
                "I5",
                format!("the highlight `{key}` names no comment of the view"),
            ));
        }
    }
    problems
}
