//! The normalized view: what layout, export and accessibility read (document model §14, §17).

use std::collections::BTreeMap;

use bayan_crdt::{ATOM_KEY, Run, Value};

use crate::atoms::encode_binding;
use crate::raw::{RawDocument, RawRow, RawTable, registry};
use crate::{AtomKind, EntityId, Props};

/// One element of a normalized story.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    /// Text characters (never placeholder characters) with their marks. Adjacent text items always differ in their marks, and no text item is empty.
    Text {
        /// The characters.
        text: String,
        /// The marks.
        marks: Props,
    },
    /// A special atom.
    Atom {
        /// Its kind.
        kind: AtomKind,
        /// The entity it references (`None` only for a tab).
        id: Option<EntityId>,
        /// Its marks (always empty for kinds that do not keep marks, see [`AtomKind::keeps_marks`]).
        marks: Props,
    },
}

impl Item {
    /// The atom kind, if this is an atom.
    #[must_use]
    pub const fn kind(&self) -> Option<AtomKind> {
        match self {
            Self::Atom { kind, .. } => Some(*kind),
            Self::Text { .. } => None,
        }
    }

    /// Whether this is an atom of `kind`.
    #[must_use]
    pub fn is(&self, kind: AtomKind) -> bool {
        self.kind() == Some(kind)
    }

    /// The number of positions it occupies (characters for text, one for an atom).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Text { text, .. } => text.chars().count(),
            Self::Atom { .. } => 1,
        }
    }

    /// Whether it occupies no position (never true in a view).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A table in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Its properties.
    pub props: Props,
    /// Its rows, in order (at least one).
    pub rows: Vec<EntityId>,
}

/// A table row in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Its properties.
    pub props: Props,
    /// Its cells, in order (at least one; rows of one table may have different numbers of cells, as in Word).
    pub cells: Vec<EntityId>,
}

/// A table cell in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// Its properties.
    pub props: Props,
    /// Its story.
    pub story: EntityId,
}

/// A comment in the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// Its properties (author, date…).
    pub props: Props,
    /// The story holding its text.
    pub story: EntityId,
}

/// A normalized document: deterministic, identical on every replica that has received the same changes, and satisfying the invariants I1–I7 (checked by [`crate::check_invariants`]).
///
/// Only what is reachable from the main story is in the view: stories of tables and comments that atoms reference, and the entities that atoms reference. Entities that nothing references stay in the CRDT (undo may restore their reference) but are invisible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// Every story, by identifier: the main story ([`EntityId::MAIN_STORY`]), table cell stories and comment stories.
    pub stories: BTreeMap<EntityId, Vec<Item>>,
    /// Paragraph properties, for every paragraph end in the view.
    pub paragraphs: BTreeMap<EntityId, Props>,
    /// Tables.
    pub tables: BTreeMap<EntityId, Table>,
    /// Rows.
    pub rows: BTreeMap<EntityId, Row>,
    /// Cells.
    pub cells: BTreeMap<EntityId, Cell>,
    /// Objects.
    pub objects: BTreeMap<EntityId, Props>,
    /// Fields.
    pub fields: BTreeMap<EntityId, Props>,
    /// Comments.
    pub comments: BTreeMap<EntityId, Comment>,
    /// Ranges.
    pub ranges: BTreeMap<EntityId, Props>,
    /// The final section's properties (invariant I7).
    pub section: Props,
}

impl View {
    /// The main story.
    #[must_use]
    pub fn main(&self) -> &[Item] {
        self.stories
            .get(&EntityId::MAIN_STORY)
            .map_or(&[], Vec::as_slice)
    }

    /// The plain text of a story, as a text export or an accessibility tree would present it: paragraph ends become line feeds, tabs become tab characters, objects become U+FFFC OBJECT REPLACEMENT CHARACTER, field codes are left out (only results are shown), and nothing else is added. Placeholder characters never appear in it.
    #[must_use]
    pub fn plain_text(&self, story: EntityId) -> String {
        let mut text = String::new();
        // For every open field, whether its code is still being read (the code is hidden; the result is shown).
        let mut fields: Vec<bool> = Vec::new();
        let in_code = |fields: &[bool]| fields.iter().any(|in_code| *in_code);
        for item in self.stories.get(&story).map_or(&[][..], Vec::as_slice) {
            match item {
                Item::Text { text: chars, .. } => {
                    if !in_code(&fields) {
                        text.push_str(chars);
                    }
                }
                Item::Atom { kind, .. } => match kind {
                    AtomKind::ParagraphEnd => text.push('\n'),
                    AtomKind::Tab => {
                        if !in_code(&fields) {
                            text.push('\t');
                        }
                    }
                    AtomKind::ObjectAnchor => {
                        if !in_code(&fields) {
                            text.push('\u{FFFC}');
                        }
                    }
                    AtomKind::FieldBegin => fields.push(true),
                    AtomKind::FieldSeparator => {
                        if let Some(last) = fields.last_mut() {
                            *last = false;
                        }
                    }
                    AtomKind::FieldEnd => {
                        fields.pop();
                    }
                    AtomKind::RangeStart
                    | AtomKind::RangeEnd
                    | AtomKind::TableBlock
                    | AtomKind::CommentReference => {}
                },
            }
        }
        text
    }

    /// The view written back as a raw document, as if every structure that normalization added had been stored. Normalizing the result gives this view again (normalization is idempotent), which the tests check.
    #[must_use]
    pub fn to_raw(&self) -> RawDocument {
        let mut raw = RawDocument {
            paragraphs: self.paragraphs.clone(),
            objects: self.objects.clone(),
            fields: self.fields.clone(),
            ranges: self.ranges.clone(),
            body: self.section.clone(),
            ..RawDocument::default()
        };
        for (id, items) in &self.stories {
            let runs = items.iter().map(item_run).collect();
            if *id == EntityId::MAIN_STORY {
                raw.main = runs;
            } else {
                raw.stories.insert(*id, runs);
            }
        }
        let ids = |ids: &[EntityId]| ids.iter().map(|id| Value::Str(id.to_string())).collect();
        for (id, table) in &self.tables {
            raw.tables.insert(
                *id,
                RawTable {
                    props: table.props.clone(),
                    rows: ids(&table.rows),
                },
            );
        }
        for (id, row) in &self.rows {
            raw.rows.insert(
                *id,
                RawRow {
                    props: row.props.clone(),
                    cells: ids(&row.cells),
                },
            );
        }
        for (id, cell) in &self.cells {
            let mut props = cell.props.clone();
            props.insert(
                registry::STORY.to_owned(),
                Value::Str(cell.story.to_string()),
            );
            raw.cells.insert(*id, props);
        }
        for (id, comment) in &self.comments {
            let mut props = comment.props.clone();
            props.insert(
                registry::STORY.to_owned(),
                Value::Str(comment.story.to_string()),
            );
            raw.comments.insert(*id, props);
        }
        raw
    }
}

/// An item as the run that stores it.
fn item_run(item: &Item) -> Run {
    match item {
        Item::Text { text, marks } => Run {
            text: text.clone(),
            marks: marks.clone(),
        },
        Item::Atom { kind, id, marks } => {
            let mut marks = marks.clone();
            if let Some(id) = id {
                marks.insert(ATOM_KEY.to_owned(), Value::Str(encode_binding(*kind, *id)));
            }
            Run {
                text: kind.placeholder().to_string(),
                marks,
            }
        }
    }
}
