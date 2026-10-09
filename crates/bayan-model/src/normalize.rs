//! Normalization: the deterministic projection of the raw state onto a view that satisfies the invariants (document model §14).
//!
//! Concurrent edits can leave the raw state inconsistent: a story without its final paragraph end, field delimiters that no longer match, a table in the middle of a paragraph, two atoms that reference the same entity. Normalization repairs all of that **in the view only**; it never writes to the CRDT. Every replica that has received the same changes computes the same view, because the procedure depends only on the raw state and visits it in one fixed order: the main story first, entering each table at its atom (depth first), then comment stories in the order of their references.
//!
//! For each story the rules apply in this order:
//!
//! 1. **Decoding and N5.** Placeholder characters become atoms when their binding names an entity of the right kind that exists; otherwise they are dropped (N5). Other control characters are dropped (placeholders never leak into the view). The binding mark itself, null marks, and marks and property values that the adapter cannot store (a floating-point number, a container reference or a value nested too deeply, which another replica may store) are removed: the model reads such a value as missing.
//! 2. **N7, with N6 and nesting.** An atom whose entity already has an atom of the same role earlier in document order is dropped (N7). A table atom that survives is entered at once: its rows and cells are read, rows without cells are omitted, and a table without rows is omitted together with its atom (N6). Tables nested deeper than [`MAX_TABLE_DEPTH`] are dropped (a resource limit, proposed as rule N9).
//! 3. **N2.** Field delimiters are matched like brackets: a separator belongs to the innermost open field if that field has none yet, an end closes the innermost open field if it is that field's end; every delimiter that is not part of a complete field is dropped, and the text between them becomes ordinary text.
//! 4. **N3.** A range end is kept only after its range's start in the same story; a start without an end gets a zero-length range (an end right after it).
//! 5. **N1.** A story that does not end with a paragraph end gets one.
//! 6. **N4.** A block-level atom that is not at a block position splits the paragraph: a paragraph end with the containing paragraph's properties is inserted before it.
//!
//! Then, once every story is normalized: **N5 for marks**, a comment highlight (`cmt:<id>`) whose comment is not in the view is removed, and text that differed only by it is merged; and **N8** (proposed), final-section properties that are missing, not integers, or outside what OOXML allows (a page size that is not positive, a negative left or right margin) get defaults, so I7 holds.
//!
//! Structure that normalization adds has identifiers derived deterministically from its context, so that every replica derives the same identifiers, and writing it into the CRDT later ("materialization", see [`Report::virtual_paragraph_ends`]) leaves the view unchanged. A derived paragraph identifier never takes one that a stored atom binds, wherever that atom stands, so that a derived paragraph end never takes over a stored one.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use bayan_crdt::{ATOM_KEY, Run, Value};

use crate::atoms::{decode_binding, is_text_character};
use crate::marks;
use crate::raw::{RawDocument, registry};
use crate::view::{Cell, Comment, Item, Row, Table, View};
use crate::{AtomKind, EntityId, Props};

/// The deepest nesting of tables the view shows: a table whose atom lies in a story nested this many tables deep is dropped. Without a limit a hostile document could nest tables until the recursion exhausts the stack.
pub const MAX_TABLE_DEPTH: usize = 32;

/// The final-section properties that I7 requires, with the defaults that N8 supplies (twips: A4 portrait, 2.54 cm margins). The defaults are provisional until a Word Behavior Note records what Word assumes for a document without them, because they change layout.
pub const SECTION_DEFAULTS: [(&str, i64); 6] = [
    ("pgSz.w", 11_906),
    ("pgSz.h", 16_838),
    ("pgMar.top", 1_440),
    ("pgMar.bottom", 1_440),
    ("pgMar.left", 1_440),
    ("pgMar.right", 1_440),
];

/// Tags mixed into derived identifiers, one per kind of added structure.
const TAG_N1: u64 = 0x4E31;
const TAG_N4: u64 = 0x4E34;
const TAG_STORY: u64 = 0x5354;

/// Which rule added a virtual paragraph end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// N1: the final paragraph end of a story.
    N1,
    /// N4: a paragraph end before a block-level atom found mid-paragraph.
    N4,
}

/// A paragraph end that exists only in the view. Writing it into the CRDT (materialization) inserts a paragraph-end atom with this identifier at `raw_position` of the story and creates the paragraph with these properties, which leaves the view unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualParagraphEnd {
    /// The story.
    pub story: EntityId,
    /// Where it belongs in the raw story: insert before the atom at this position (the end of the story for N1).
    pub raw_position: usize,
    /// Its derived identifier.
    pub id: EntityId,
    /// The paragraph's properties.
    pub props: Props,
    /// The rule that added it.
    pub rule: Rule,
}

/// What normalization did, for tests, measurements and materialization. Not part of the view.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// Control characters dropped because they are not placeholders, or placeholders whose binding is missing or names another kind of atom.
    pub dropped_characters: usize,
    /// N1: virtual final paragraph ends added.
    pub n1: usize,
    /// N2: unmatched field delimiters dropped.
    pub n2: usize,
    /// N3: range ends dropped and zero-length ranges added.
    pub n3: usize,
    /// N4: paragraphs split before a block-level atom.
    pub n4: usize,
    /// N5: atoms and list items whose entity is missing, and comment highlights whose comment is not in the view, dropped.
    pub n5: usize,
    /// Of `n5`: comment highlights dropped (one per text item or atom that carried one).
    pub highlights: usize,
    /// N6: rows without cells and tables without rows, omitted.
    pub n6: usize,
    /// N7: atoms and list items that reference an entity already referenced earlier, dropped.
    pub n7: usize,
    /// N8: final-section properties supplied by default.
    pub n8: usize,
    /// Tables dropped for nesting deeper than [`MAX_TABLE_DEPTH`].
    pub too_deep: usize,
    /// Stories that a cell or comment references but that do not exist (or belong to another owner), shown as empty stories.
    pub virtual_stories: Vec<(EntityId, EntityId)>,
    /// Every paragraph end added by N1 or N4, in the order they were added.
    pub virtual_paragraph_ends: Vec<VirtualParagraphEnd>,
}

/// Normalizes a raw document into its view.
#[must_use]
pub fn normalize(raw: &RawDocument) -> (View, Report) {
    let mut normalizer = Normalizer {
        raw,
        view: View::default(),
        report: Report::default(),
        seen_atoms: BTreeSet::new(),
        seen_rows: BTreeSet::new(),
        seen_cells: BTreeSet::new(),
        seen_stories: BTreeSet::from([EntityId::MAIN_STORY]),
        comment_stories: VecDeque::new(),
        stored_paragraphs: None,
    };
    normalizer.story(EntityId::MAIN_STORY, Some(&raw.main), 0);
    while let Some(story) = normalizer.comment_stories.pop_front() {
        let runs = raw.stories.get(&story).map(Vec::as_slice);
        normalizer.story(story, runs, 0);
    }
    normalizer.comment_marks();
    normalizer.section();
    (normalizer.view, normalizer.report)
}

/// One element of a story while it is being normalized.
#[derive(Debug, Clone)]
enum Piece {
    Text {
        text: String,
        marks: Props,
    },
    Atom {
        kind: AtomKind,
        id: Option<EntityId>,
        marks: Props,
        /// Its position in the raw story (for an added atom: where it would be inserted).
        raw: usize,
    },
}

impl Piece {
    const fn atom(&self) -> Option<(AtomKind, Option<EntityId>)> {
        match self {
            Self::Atom { kind, id, .. } => Some((*kind, *id)),
            Self::Text { .. } => None,
        }
    }

    fn is(&self, kind: AtomKind) -> bool {
        self.atom().is_some_and(|(atom, _)| atom == kind)
    }
}

struct Normalizer<'a> {
    raw: &'a RawDocument,
    view: View,
    report: Report,
    /// Every (role, entity) pair already referenced in document order (N7).
    seen_atoms: BTreeSet<(AtomKind, EntityId)>,
    seen_rows: BTreeSet<EntityId>,
    seen_cells: BTreeSet<EntityId>,
    seen_stories: BTreeSet<EntityId>,
    /// Comment stories to normalize after the main story, in the order of their references.
    comment_stories: VecDeque<EntityId>,
    /// Every paragraph identifier that the stored state takes (bound by a stored atom, in any story, or with an entry in the paragraph registry other than a mergeable map); collected the first time an identifier is derived.
    stored_paragraphs: Option<BTreeSet<EntityId>>,
}

impl<'a> Normalizer<'a> {
    /// Normalizes one story (`runs` is `None` for a story that does not exist, which then becomes empty) at table nesting depth `depth`.
    fn story(&mut self, id: EntityId, runs: Option<&'a [Run]>, depth: usize) {
        let raw_len = runs.map_or(0, |runs| runs.iter().map(Run::len).sum());
        let pieces = self.decode(runs.unwrap_or(&[]));
        let pieces = self.references(pieces, depth);
        let pieces = self.fields(pieces);
        let pieces = self.ranges(pieces);
        let pieces = self.final_paragraph_end(id, pieces, raw_len);
        let pieces = self.block_positions(id, pieces);
        let items = self.items(pieces);
        self.view.stories.insert(id, items);
    }

    /// Step 1: placeholders into atoms, and N5.
    fn decode(&mut self, runs: &[Run]) -> Vec<Piece> {
        let mut pieces: Vec<Piece> = Vec::new();
        let mut raw = 0;
        for run in runs {
            let marks = clean_marks(&run.marks);
            let binding = run
                .marks
                .get(ATOM_KEY)
                .and_then(Value::as_str)
                .and_then(decode_binding);
            let mut chunk = String::new();
            for character in run.text.chars() {
                if let Some(kind) = AtomKind::from_placeholder(character) {
                    push_text(&mut pieces, &mut chunk, &marks);
                    let id = match binding {
                        _ if kind == AtomKind::Tab => Some(None),
                        Some((bound, id)) if bound == kind && self.raw.has_entity(kind, id) => {
                            Some(Some(id))
                        }
                        Some((bound, _)) if bound == kind => {
                            self.report.n5 += 1;
                            None
                        }
                        _ => {
                            self.report.dropped_characters += 1;
                            None
                        }
                    };
                    if let Some(id) = id {
                        pieces.push(Piece::Atom {
                            kind,
                            id,
                            marks: marks.clone(),
                            raw,
                        });
                    }
                } else if is_text_character(character) {
                    chunk.push(character);
                } else {
                    self.report.dropped_characters += 1;
                }
                raw += 1;
            }
            push_text(&mut pieces, &mut chunk, &marks);
        }
        pieces
    }

    /// Step 2: N7, entering tables (N6, nesting limit) and registering comments.
    fn references(&mut self, pieces: Vec<Piece>, depth: usize) -> Vec<Piece> {
        let mut kept = Vec::with_capacity(pieces.len());
        for piece in pieces {
            if let Some((kind, Some(id))) = piece.atom() {
                if !self.seen_atoms.insert((kind, id)) {
                    self.report.n7 += 1;
                    continue;
                }
                match kind {
                    AtomKind::TableBlock => {
                        if depth >= MAX_TABLE_DEPTH {
                            self.report.too_deep += 1;
                            continue;
                        }
                        if !self.table(id, depth + 1) {
                            continue;
                        }
                    }
                    AtomKind::CommentReference => self.comment(id),
                    _ => {}
                }
            }
            kept.push(piece);
        }
        kept
    }

    /// Reads table `id` into the view; returns whether it has any rows (N6).
    fn table(&mut self, id: EntityId, depth: usize) -> bool {
        let Some(table) = self.raw.tables.get(&id) else {
            return false;
        };
        let mut rows = Vec::new();
        for item in &table.rows {
            let Some(row_id) = item.as_str().and_then(EntityId::parse) else {
                self.report.n5 += 1;
                continue;
            };
            let Some(row) = self.raw.rows.get(&row_id) else {
                self.report.n5 += 1;
                continue;
            };
            if !self.seen_rows.insert(row_id) {
                self.report.n7 += 1;
                continue;
            }
            let mut cells = Vec::new();
            for item in &row.cells {
                let Some(cell_id) = item.as_str().and_then(EntityId::parse) else {
                    self.report.n5 += 1;
                    continue;
                };
                let Some(cell) = self.raw.cells.get(&cell_id) else {
                    self.report.n5 += 1;
                    continue;
                };
                if !self.seen_cells.insert(cell_id) {
                    self.report.n7 += 1;
                    continue;
                }
                let story = self.owned_story(cell_id, cell, Some(depth));
                self.view.cells.insert(
                    cell_id,
                    Cell {
                        props: without_story(cell),
                        story,
                    },
                );
                cells.push(cell_id);
            }
            if cells.is_empty() {
                self.report.n6 += 1;
                continue;
            }
            self.view.rows.insert(
                row_id,
                Row {
                    props: storable(&row.props),
                    cells,
                },
            );
            rows.push(row_id);
        }
        if rows.is_empty() {
            self.report.n6 += 1;
            return false;
        }
        self.view.tables.insert(
            id,
            Table {
                props: storable(&table.props),
                rows,
            },
        );
        true
    }

    /// Registers comment `id`, whose story is normalized after the main story.
    fn comment(&mut self, id: EntityId) {
        let Some(props) = self.raw.comments.get(&id) else {
            return;
        };
        let story = self.owned_story(id, props, None);
        self.view.comments.insert(
            id,
            Comment {
                props: without_story(props),
                story,
            },
        );
    }

    /// The story that the cell or comment `owner` (with properties `props`) owns. A story that does not exist, or that an earlier owner already took, is replaced by an empty story with a derived identifier. Cell stories are normalized at once at `depth`; comment stories (`depth` is `None`) are queued.
    fn owned_story(&mut self, owner: EntityId, props: &Props, depth: Option<usize>) -> EntityId {
        let raw = self.raw;
        let stored = props
            .get(registry::STORY)
            .and_then(Value::as_str)
            .and_then(EntityId::parse)
            .filter(|story| *story != EntityId::MAIN_STORY && raw.stories.contains_key(story));
        let story = match stored {
            Some(story) if self.seen_stories.insert(story) => story,
            _ => {
                let [high, low] = owner.halves();
                let mut attempt = 0;
                let story = loop {
                    let candidate = EntityId::derive(&[high, low, TAG_STORY, attempt]);
                    if candidate != EntityId::MAIN_STORY
                        && !raw.stories.contains_key(&candidate)
                        && self.seen_stories.insert(candidate)
                    {
                        break candidate;
                    }
                    attempt += 1;
                };
                self.report.virtual_stories.push((owner, story));
                story
            }
        };
        match depth {
            Some(depth) => {
                let runs = raw.stories.get(&story).map(Vec::as_slice);
                self.story(story, runs, depth);
            }
            None => self.comment_stories.push_back(story),
        }
        story
    }

    /// Step 3: N2, field delimiters matched like brackets.
    fn fields(&mut self, pieces: Vec<Piece>) -> Vec<Piece> {
        let mut keep = vec![true; pieces.len()];
        let mut matched = vec![false; pieces.len()];
        // Open fields: identifier, position of the begin, position of the separator.
        let mut open: Vec<(EntityId, usize, Option<usize>)> = Vec::new();
        for (index, piece) in pieces.iter().enumerate() {
            let Some((kind, Some(id))) = piece.atom() else {
                continue;
            };
            match kind {
                AtomKind::FieldBegin => open.push((id, index, None)),
                AtomKind::FieldSeparator => match open.last_mut() {
                    Some((top, _, separator @ None)) if *top == id => *separator = Some(index),
                    _ => keep[index] = false,
                },
                AtomKind::FieldEnd => match open.last() {
                    Some((top, begin, separator)) if *top == id => {
                        matched[*begin] = true;
                        if let Some(separator) = separator {
                            matched[*separator] = true;
                        }
                        matched[index] = true;
                        open.pop();
                    }
                    _ => keep[index] = false,
                },
                _ => {}
            }
        }
        let mut kept = Vec::with_capacity(pieces.len());
        for (index, piece) in pieces.into_iter().enumerate() {
            let is_delimiter = matches!(
                piece.atom(),
                Some((
                    AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd,
                    _
                ))
            );
            if is_delimiter && !(keep[index] && matched[index]) {
                self.report.n2 += 1;
                continue;
            }
            kept.push(piece);
        }
        kept
    }

    /// Step 4: N3, ranges.
    fn ranges(&mut self, pieces: Vec<Piece>) -> Vec<Piece> {
        let mut starts: BTreeMap<EntityId, usize> = BTreeMap::new();
        let mut ends: BTreeMap<EntityId, usize> = BTreeMap::new();
        for (index, piece) in pieces.iter().enumerate() {
            match piece.atom() {
                Some((AtomKind::RangeStart, Some(id))) => {
                    starts.insert(id, index);
                }
                Some((AtomKind::RangeEnd, Some(id))) => {
                    ends.insert(id, index);
                }
                _ => {}
            }
        }
        let mut kept = Vec::with_capacity(pieces.len());
        for (index, piece) in pieces.into_iter().enumerate() {
            match piece.atom() {
                Some((AtomKind::RangeEnd, Some(id))) => {
                    if starts.get(&id).is_some_and(|start| *start < index) {
                        kept.push(piece);
                    } else {
                        self.report.n3 += 1;
                    }
                }
                Some((AtomKind::RangeStart, Some(id))) => {
                    let raw = match &piece {
                        Piece::Atom { raw, .. } => *raw,
                        Piece::Text { .. } => 0,
                    };
                    kept.push(piece);
                    if !ends.get(&id).is_some_and(|end| *end > index) {
                        self.report.n3 += 1;
                        kept.push(Piece::Atom {
                            kind: AtomKind::RangeEnd,
                            id: Some(id),
                            marks: Props::new(),
                            raw: raw + 1,
                        });
                    }
                }
                _ => kept.push(piece),
            }
        }
        kept
    }

    /// Step 5: N1, the final paragraph end.
    fn final_paragraph_end(
        &mut self,
        story: EntityId,
        mut pieces: Vec<Piece>,
        raw_len: usize,
    ) -> Vec<Piece> {
        if pieces
            .last()
            .is_some_and(|piece| piece.is(AtomKind::ParagraphEnd))
        {
            return pieces;
        }
        self.report.n1 += 1;
        let [high, low] = story.halves();
        let id = self.virtual_paragraph_id(&[high, low, TAG_N1]);
        let props = self
            .raw
            .paragraphs
            .get(&id)
            .map(storable)
            .unwrap_or_default();
        self.report
            .virtual_paragraph_ends
            .push(VirtualParagraphEnd {
                story,
                raw_position: raw_len,
                id,
                props: props.clone(),
                rule: Rule::N1,
            });
        self.view.paragraphs.insert(id, props);
        pieces.push(Piece::Atom {
            kind: AtomKind::ParagraphEnd,
            id: Some(id),
            marks: Props::new(),
            raw: raw_len,
        });
        pieces
    }

    /// Step 6: N4, block-level atoms at block positions.
    fn block_positions(&mut self, story: EntityId, pieces: Vec<Piece>) -> Vec<Piece> {
        // The paragraph each piece belongs to: the one whose end is the first at or after it (N1 guarantees there is one). Found in one pass from the end, so that many tables in one paragraph cost linear time, not quadratic.
        let mut containing_paragraph: Vec<Option<EntityId>> = vec![None; pieces.len()];
        let mut following = None;
        for (index, piece) in pieces.iter().enumerate().rev() {
            if let Some((AtomKind::ParagraphEnd, Some(id))) = piece.atom() {
                following = Some(id);
            }
            containing_paragraph[index] = following;
        }
        let mut result: Vec<Piece> = Vec::with_capacity(pieces.len());
        for (index, piece) in pieces.iter().enumerate() {
            if let Piece::Atom {
                kind,
                id: Some(block),
                raw,
                ..
            } = piece
                && kind.is_block()
            {
                let at_block_position = result.last().is_none_or(|previous| {
                    previous
                        .atom()
                        .is_some_and(|(kind, _)| kind == AtomKind::ParagraphEnd || kind.is_block())
                });
                if !at_block_position {
                    let containing = containing_paragraph[index]
                        .and_then(|id| {
                            self.view
                                .paragraphs
                                .get(&id)
                                .cloned()
                                .or_else(|| self.raw.paragraphs.get(&id).map(storable))
                        })
                        .unwrap_or_default();
                    self.report.n4 += 1;
                    let [high, low] = block.halves();
                    let id = self.virtual_paragraph_id(&[high, low, TAG_N4]);
                    let props = self
                        .raw
                        .paragraphs
                        .get(&id)
                        .map(storable)
                        .unwrap_or(containing);
                    self.report
                        .virtual_paragraph_ends
                        .push(VirtualParagraphEnd {
                            story,
                            raw_position: *raw,
                            id,
                            props: props.clone(),
                            rule: Rule::N4,
                        });
                    self.view.paragraphs.insert(id, props);
                    result.push(Piece::Atom {
                        kind: AtomKind::ParagraphEnd,
                        id: Some(id),
                        marks: Props::new(),
                        raw: *raw,
                    });
                }
            }
            result.push(piece.clone());
        }
        result
    }

    /// A derived paragraph identifier that no stored atom binds, in any story, whose entry in the paragraph registry is missing or a mergeable map, and that no paragraph end of the view has taken yet (trying again with a counter if needed), reserved at once.
    ///
    /// Avoiding only the paragraph ends met so far is not enough: a cell story is normalized in the middle of the story that contains its table, so a stored paragraph end later in that story would otherwise lose its identifier to a derived one and be dropped as a repeat (N7). An identifier whose entry is anything but a mergeable map is avoided too (a value, another kind of container, or a map that another replica created another way), because materialization could not create its map there without failing. A mergeable map (left by an earlier materialization whose paragraph end was since deleted) is reused, not avoided: the view reads its properties, so materializing into it adds none the view did not show, and every replica derives the same identifier whether or not it received that map, so that concurrent materializations still merge into one paragraph end.
    fn virtual_paragraph_id(&mut self, parts: &[u64]) -> EntityId {
        let raw = self.raw;
        let stored = self
            .stored_paragraphs
            .get_or_insert_with(|| stored_paragraph_ends(raw));
        let mut attempt = 0;
        loop {
            let mut all = parts.to_vec();
            all.push(attempt);
            let id = EntityId::derive(&all);
            if id != EntityId::MAIN_STORY
                && !stored.contains(&id)
                && self.seen_atoms.insert((AtomKind::ParagraphEnd, id))
            {
                return id;
            }
            attempt += 1;
        }
    }

    /// The final step: entities into the view, marks of mark-less atoms removed, text merged.
    fn items(&mut self, pieces: Vec<Piece>) -> Vec<Item> {
        let mut items: Vec<Item> = Vec::with_capacity(pieces.len());
        for piece in pieces {
            match piece {
                Piece::Text { text, marks } => {
                    if text.is_empty() {
                        continue;
                    }
                    if let Some(Item::Text {
                        text: previous,
                        marks: previous_marks,
                    }) = items.last_mut()
                        && *previous_marks == marks
                    {
                        previous.push_str(&text);
                        continue;
                    }
                    items.push(Item::Text { text, marks });
                }
                Piece::Atom {
                    kind, id, marks, ..
                } => {
                    if let Some(id) = id {
                        self.add_entity(kind, id);
                    }
                    let marks = if kind.keeps_marks() {
                        marks
                    } else {
                        Props::new()
                    };
                    items.push(Item::Atom { kind, id, marks });
                }
            }
        }
        items
    }

    /// Copies the entity of a kept atom into the view (tables and comments were copied when they were entered).
    fn add_entity(&mut self, kind: AtomKind, id: EntityId) {
        let raw = self.raw;
        let (registry, view) = match kind {
            AtomKind::ParagraphEnd => (&raw.paragraphs, &mut self.view.paragraphs),
            AtomKind::ObjectAnchor => (&raw.objects, &mut self.view.objects),
            AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd => {
                (&raw.fields, &mut self.view.fields)
            }
            AtomKind::RangeStart | AtomKind::RangeEnd => (&raw.ranges, &mut self.view.ranges),
            AtomKind::Tab | AtomKind::TableBlock | AtomKind::CommentReference => return,
        };
        if let Some(props) = registry.get(&id) {
            view.entry(id).or_insert_with(|| storable(props));
        }
    }

    /// N5 for marks: removes every comment highlight whose comment is not in the view (its reference was deleted, or never arrived), and merges text that differed only by it.
    fn comment_marks(&mut self) {
        let View {
            stories, comments, ..
        } = &mut self.view;
        let dangling = |key: &str| {
            key.split(':').next() == Some(marks::COMMENT)
                && !marks::comment_of_key(key).is_some_and(|id| comments.contains_key(&id))
        };
        for items in stories.values_mut() {
            let mut changed = false;
            for item in items.iter_mut() {
                let (Item::Text { marks, .. } | Item::Atom { marks, .. }) = item;
                let before = marks.len();
                marks.retain(|key, _| !dangling(key));
                if marks.len() != before {
                    self.report.n5 += before - marks.len();
                    self.report.highlights += before - marks.len();
                    changed = true;
                }
            }
            if changed {
                *items = merge_text(std::mem::take(items));
            }
        }
    }

    /// N8: the final section's properties, with defaults for those I7 requires.
    fn section(&mut self) {
        let mut section = storable(&self.raw.body);
        for (key, default) in SECTION_DEFAULTS {
            let valid = section
                .get(key)
                .and_then(Value::as_int)
                .is_some_and(|value| section_value_allowed(key, value));
            if !valid {
                section.insert(key.to_owned(), Value::Int(default));
                self.report.n8 += 1;
            }
        }
        self.view.section = section;
    }
}

/// Whether `value` is allowed for the final-section property `key`, as OOXML allows it: page sizes are positive, left and right margins are not negative (`ST_TwipsMeasure`), and top and bottom margins may be negative (`ST_SignedTwipsMeasure`, text that may overlap the header or footer).
fn section_value_allowed(key: &str, value: i64) -> bool {
    match key {
        "pgSz.w" | "pgSz.h" => value > 0,
        "pgMar.left" | "pgMar.right" => value >= 0,
        _ => true,
    }
}

/// The marks the view keeps: everything except the binding, null values and values the adapter cannot store (see [`storable`]).
fn clean_marks(marks: &Props) -> Props {
    marks
        .iter()
        .filter(|(key, value)| key.as_str() != ATOM_KEY && !value.is_null() && value.is_storable())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Every paragraph identifier that the stored state takes: bound by a placeholder of a stored story, or with an entry in the paragraph registry that is not a mergeable map.
fn stored_paragraph_ends(raw: &RawDocument) -> BTreeSet<EntityId> {
    std::iter::once(&raw.main)
        .chain(raw.stories.values())
        .flatten()
        .filter(|run| run.text.contains(AtomKind::ParagraphEnd.placeholder()))
        .filter_map(|run| {
            run.marks
                .get(ATOM_KEY)
                .and_then(Value::as_str)
                .and_then(decode_binding)
        })
        .filter_map(|(kind, id)| (kind == AtomKind::ParagraphEnd).then_some(id))
        .chain(raw.unmergeable_paragraph_entries.iter().copied())
        .collect()
}

/// Merges adjacent text items with equal marks.
fn merge_text(items: Vec<Item>) -> Vec<Item> {
    let mut merged: Vec<Item> = Vec::with_capacity(items.len());
    for item in items {
        if let (
            Some(Item::Text {
                text: previous,
                marks: previous_marks,
            }),
            Item::Text { text, marks },
        ) = (merged.last_mut(), &item)
            && previous_marks == marks
        {
            previous.push_str(text);
            continue;
        }
        merged.push(item);
    }
    merged
}

/// Ends the current chunk of text, merging it into the previous text piece when the marks are equal.
fn push_text(pieces: &mut Vec<Piece>, chunk: &mut String, marks: &Props) {
    if chunk.is_empty() {
        return;
    }
    if let Some(Piece::Text {
        text,
        marks: previous,
    }) = pieces.last_mut()
        && previous == marks
    {
        text.push_str(chunk);
    } else {
        pieces.push(Piece::Text {
            text: chunk.clone(),
            marks: marks.clone(),
        });
    }
    chunk.clear();
}

/// The properties the view keeps: those whose value the adapter can store. Another replica can store what this adapter refuses (a floating-point number, a container reference, a value nested too deeply); the model reads such a value as missing, as it does for marks, so that copying properties into new structure (a split paragraph, a materialized paragraph end) never fails halfway.
fn storable(props: &Props) -> Props {
    props
        .iter()
        .filter(|(_, value)| value.is_storable())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn without_story(props: &Props) -> Props {
    let mut props = storable(props);
    props.remove(registry::STORY);
    props
}
