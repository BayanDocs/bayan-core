//! A document stored in the CRDT, and the operations of the CORE-004 brief.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;

use bayan_crdt::{
    ATOM_KEY, CrdtError, Doc, ImportError, ImportLimits, ImportReport, PeerId, PropertyMap, Run,
    Story, UndoManager, Value, Version, VersionVector,
};

use crate::atoms::{decode_binding, encode_binding, is_text_character};
use crate::raw::{RawDocument, registry};
use crate::view::View;
use crate::{AtomKind, EntityId, IdGenerator, Props, Report, SECTION_DEFAULTS, marks, normalize};

const PARAGRAPH_END: char = '\u{0D}';
const TABLE_BLOCK: char = '\u{07}';

/// Why an operation was refused.
///
/// Every operation checks everything that can make it fail before it writes anything, so a refused operation changes nothing and leaves nothing pending (see [`Document::has_pending_changes`]), except that it may have materialized the story first (see [`Document`]), which does not change the view and is committed. Values that another replica stored and the adapter cannot store are skipped, never copied, and entries it stored where a property map belongs are written into when they are maps and left out when they are not, as the view leaves them out. One case is known where an error of the CRDT can still come after a write: identifiers come from a generator that is not cryptographic, so a replica that has seen some of this replica's identifiers can compute its next ones and store a value under one first; creating that entity then fails after the operation's earlier writes (CORE-004 report §11, a follow-up for CORE-101).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The CRDT refused.
    Crdt(CrdtError),
    /// An import was refused.
    Import(ImportError),
    /// The story does not exist.
    NoSuchStory(EntityId),
    /// The table does not exist.
    NoSuchTable(EntityId),
    /// The paragraph does not exist.
    NoSuchParagraph(EntityId),
    /// Text may not be empty or contain control characters (they are reserved for atoms).
    InvalidText,
    /// A position or range is outside what the operation allows; nothing may be inserted after a story's final paragraph end.
    InvalidPosition {
        /// The offending position.
        pos: usize,
        /// The story's length.
        len: usize,
    },
    /// A table may only be inserted at a block position, and text may not be typed at the block position before a table.
    NotABlockPosition,
    /// The position does not hold a paragraph end, or holds the final one, which cannot be removed.
    NotAParagraphEnd,
    /// The operation would leave a table in the middle of a paragraph.
    WouldBreakBlockStructure,
    /// The move would put a table inside itself, or a comment reference inside a comment.
    WouldCreateCycle,
    /// A table must keep at least one row, and every row at least one cell.
    LastRowOrColumn,
    /// The mark key is the reserved atom binding ([`ATOM_KEY`]), or belongs to none of the model's mark families ([`marks::FAMILIES`]).
    InvalidKey,
    /// The edit would delete or move part of a field (its begin, separator or end) or one end of a range without the rest. What Word does in that case is for a Word Behavior Note to establish (ADR-0007, decision 6); until then the model refuses.
    WouldUnbalance,
    /// The document shows an older version (see [`Document::checkout`]) and cannot be edited.
    Detached,
}

impl fmt::Display for EditError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Crdt(error) => write!(formatter, "{error}"),
            Self::Import(error) => write!(formatter, "{error}"),
            Self::NoSuchStory(id) => write!(formatter, "no story {id}"),
            Self::NoSuchTable(id) => write!(formatter, "no table {id}"),
            Self::NoSuchParagraph(id) => write!(formatter, "no paragraph {id}"),
            Self::InvalidText => {
                formatter.write_str("text is empty or contains control characters")
            }
            Self::InvalidPosition { pos, len } => {
                write!(
                    formatter,
                    "position {pos} is not allowed in a story of {len}"
                )
            }
            Self::NotABlockPosition => formatter.write_str("not a block position"),
            Self::NotAParagraphEnd => formatter.write_str("not a removable paragraph end"),
            Self::WouldBreakBlockStructure => {
                formatter.write_str("the edit would leave a table in the middle of a paragraph")
            }
            Self::WouldCreateCycle => formatter.write_str("the move would create a cycle"),
            Self::LastRowOrColumn => {
                formatter.write_str("a table keeps at least one row and column")
            }
            Self::InvalidKey => formatter.write_str("the mark key is reserved or unknown"),
            Self::WouldUnbalance => formatter
                .write_str("the edit would separate the parts of a field or the ends of a range"),
            Self::Detached => formatter.write_str("the document shows an older version"),
        }
    }
}

impl std::error::Error for EditError {}

impl From<CrdtError> for EditError {
    fn from(error: CrdtError) -> Self {
        Self::Crdt(error)
    }
}

/// A document stored in the CRDT: one replica with its own undo history.
///
/// Every operation is one transaction: one commit, one undo step, and one change that other replicas receive. Positions are positions in the stored story (one per atom).
///
/// **Entities and undo.** An operation that creates entities (a paragraph, a table with its rows, cells and cell stories, a comment with its story, a field, an object, a range) commits their creation first, as a change that is not an undo step, and then makes the change that references them (the atom, or the row or cell in a list) as the operation's undo step. Undo removes only the reference: the entity stays stored but invisible (normalization rule N5) and redo shows it again with everything other replicas wrote into it meanwhile. (Undoing the creation itself is not an option with Loro 1.16.2: redoing the creation of a child container duplicates its content; see the CORE-004 report.)
///
/// **Materialization** (document model §14): when an operation edits a story whose view contains structure that only normalization added (a final paragraph end, or a paragraph end before a table found mid-paragraph), the operation first writes that structure into the CRDT, with the identifiers and properties the view already shows. The view therefore does not change by materialization, and concurrent materializations by several replicas merge into the same entities. Materialization is committed as its own change and is not an undo step: undoing the operation returns the view to the state before the operation without removing the materialized structure, which the view showed anyway. Positions that the caller computed on the story before materialization are mapped onto the story after it: a position between two characters keeps its place before an inserted paragraph end, and a range keeps exactly the characters it covered (see `Positions`).
#[derive(Debug)]
pub struct Document {
    crdt: Doc,
    ids: IdGenerator,
    undo: UndoManager,
    /// Stories known not to need materialization since the last import or undo.
    clean: BTreeSet<EntityId>,
    /// Counts of checked materializations, once [`Document::check_materializations`] was called.
    check: Option<MaterializationCheck>,
}

/// What [`Document::check_materializations`] counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MaterializationCheck {
    /// Materializations that wrote structure, since the check started.
    pub performed: usize,
    /// Materializations after which the view differed from the view before them. Materialization must never change the view (document model §14), so this must stay 0.
    pub changed_view: usize,
    /// Materializations that failed, possibly after writing part of their structure. They must never happen, so this must stay 0.
    pub failed: usize,
}

impl Document {
    /// A new document for replica `peer`: an empty main story (one paragraph) and default section properties. `seed` seeds the identifier generator; replicas must use different peers.
    ///
    /// # Errors
    ///
    /// [`EditError::Crdt`] if the CRDT refuses (for example the reserved peer `u64::MAX`).
    pub fn new(peer: u64, seed: u64) -> Result<Self, EditError> {
        let mut document = Self::replica(peer, seed)?;
        let paragraph = document.ids.next_id();
        document
            .paragraph_map(paragraph)?
            .set("style", &Value::from("Normal"))?;
        document.crdt.main_story().insert_atom(
            0,
            PARAGRAPH_END,
            &encode_binding(AtomKind::ParagraphEnd, paragraph),
        )?;
        let body = document.crdt.root_map(registry::BODY)?;
        for (key, value) in SECTION_DEFAULTS {
            body.set(key, &Value::Int(value))?;
        }
        document.crdt.commit();
        // The document's creation is not an undo step.
        document.undo = UndoManager::new(&document.crdt);
        Ok(document)
    }

    /// An empty replica for `peer`, which receives the document from another replica.
    ///
    /// # Errors
    ///
    /// As for [`Document::new`].
    pub fn replica(peer: u64, seed: u64) -> Result<Self, EditError> {
        Ok(Self::around(
            Doc::new(PeerId(peer), &marks::FAMILIES)?,
            peer,
            seed,
        ))
    }

    /// A replica loaded from a snapshot or update blob, within `limits`.
    ///
    /// # Errors
    ///
    /// [`EditError::Import`] if the blob is refused, or as for [`Document::new`].
    pub fn load(
        bytes: &[u8],
        peer: u64,
        seed: u64,
        limits: &ImportLimits,
    ) -> Result<Self, EditError> {
        // The blob is imported before the undo manager subscribes to the replica. Loro decodes the containers of a snapshot only when they are first read, but a subscriber present during the import makes it compute the whole document as one change event, which takes seconds for a 500-page document (CORE-004 report).
        let crdt =
            Doc::load(bytes, PeerId(peer), &marks::FAMILIES, limits).map_err(
                |error| match error {
                    CrdtError::Import(error) => EditError::Import(error),
                    other => EditError::Crdt(other),
                },
            )?;
        Ok(Self::around(crdt, peer, seed))
    }

    /// A document around a replica, with an empty undo history.
    fn around(crdt: Doc, peer: u64, seed: u64) -> Self {
        let undo = UndoManager::new(&crdt);
        Self {
            crdt,
            ids: IdGenerator::new(seed ^ peer.rotate_left(32) ^ 0xD0C0_0004_D0C0_0004),
            undo,
            clean: BTreeSet::new(),
            check: None,
        }
    }

    /// From now on, normalizes the document before and after every materialization and counts the materializations that changed the view (see [`MaterializationCheck`]). For tests and the simulation: each check normalizes the whole document twice.
    pub fn check_materializations(&mut self) {
        self.check.get_or_insert_default();
    }

    /// What [`Document::check_materializations`] has counted, if it was called.
    #[must_use]
    pub const fn materialization_check(&self) -> Option<MaterializationCheck> {
        self.check
    }

    /// The underlying replica.
    #[must_use]
    pub const fn crdt(&self) -> &Doc {
        &self.crdt
    }

    /// The stored state.
    #[must_use]
    pub fn raw(&self) -> RawDocument {
        RawDocument::read(&self.crdt)
    }

    /// The normalized view.
    #[must_use]
    pub fn view(&self) -> View {
        normalize(&self.raw()).0
    }

    /// The length of a stored story, if it exists.
    #[must_use]
    pub fn story_len(&self, story: EntityId) -> Option<usize> {
        self.story(story).ok().map(|story| story.len())
    }

    /// The stored characters of a story, if it exists: the raw story as the CRDT holds it, with a placeholder character for every atom and without normalization (use [`Document::view`] and [`View::plain_text`] for what a reader sees). Positions in it are the positions the operations take.
    #[must_use]
    pub fn story_text(&self, story: EntityId) -> Option<String> {
        self.story(story).ok().map(|story| story.text())
    }

    /// The paragraphs whose ends a story stores, in order.
    #[must_use]
    pub fn paragraphs_in(&self, story: EntityId) -> Vec<EntityId> {
        self.story(story)
            .ok()
            .and_then(|handle| {
                let len = handle.len();
                self.atoms_in(&handle, &(0..len), AtomKind::ParagraphEnd)
                    .ok()
            })
            .unwrap_or_default()
    }

    /// The identifiers of every stored story, the main story first.
    #[must_use]
    pub fn stories(&self) -> Vec<EntityId> {
        let mut ids = vec![EntityId::MAIN_STORY];
        ids.extend(
            self.crdt
                .stories()
                .ids()
                .iter()
                .filter_map(|key| EntityId::parse(key)),
        );
        ids
    }

    /// The identifiers of every stored table.
    #[must_use]
    pub fn tables(&self) -> Vec<EntityId> {
        self.registry(registry::TABLES)
            .map(|tables| tables.ids())
            .unwrap_or_default()
            .iter()
            .filter_map(|key| EntityId::parse(key))
            .collect()
    }

    /// The row identifiers of a stored table, as stored.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`].
    pub fn table_rows(&self, table: EntityId) -> Result<Vec<Value>, EditError> {
        Ok(self.rows_list(table)?.items())
    }

    /// The number of cells of every stored row of a stored table, in row order (rows that do not exist are left out, and a row that the stored list holds twice counts twice).
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`].
    pub fn table_shape(&self, table: EntityId) -> Result<Vec<usize>, EditError> {
        Ok(self
            .table_row_ids(table)?
            .into_iter()
            .filter_map(|row| self.cells_list(row))
            .map(|cells| cells.len())
            .collect())
    }

    // ----- Text -----

    /// Inserts `text` at `pos` of `story`. Text may not contain control characters, may not follow the final paragraph end, and may not be typed at the block position right before a table.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidText`], [`EditError::InvalidPosition`], [`EditError::NotABlockPosition`], or the errors of the CRDT.
    pub fn insert_text(
        &mut self,
        story: EntityId,
        pos: usize,
        text: &str,
    ) -> Result<(), EditError> {
        if text.is_empty() || !text.chars().all(is_text_character) {
            return Err(EditError::InvalidText);
        }
        self.edit(story, |_, handle, positions| {
            let pos = positions.insertion_point(handle, pos)?;
            if before_block_at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            handle.insert(pos, text)?;
            Ok(())
        })
    }

    /// Deletes `range` of `story`, which may not include the final paragraph end, may not leave a table in the middle of a paragraph, and may not hold only part of a field or only one end of a range ([`EditError::WouldUnbalance`]). Entities whose atoms are deleted stay stored (undo may restore their reference) but leave the view.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::WouldBreakBlockStructure`], [`EditError::WouldUnbalance`], or the errors of the CRDT.
    pub fn delete(&mut self, story: EntityId, range: Range<usize>) -> Result<(), EditError> {
        self.edit(story, |_, handle, positions| {
            let range = positions.inner_range(handle, &range)?;
            if would_break_blocks(handle, &range) {
                return Err(EditError::WouldBreakBlockStructure);
            }
            check_whole_markers(handle, &range)?;
            handle.delete(range)?;
            Ok(())
        })
    }

    /// Sets the mark `key` to `value` on `range` of `story`. A run property (`r:…`) is applied to the characters, stopping before a paragraph end that closes the range, and to the paragraph-mark formatting (`rPr.…` properties) of every paragraph end in the range that has a property map (the view shows no other), because run-property marks expand after their end and would otherwise spread into the next paragraph.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidKey`] for the atom binding or a key of no mark family, [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn format(
        &mut self,
        story: EntityId,
        range: Range<usize>,
        key: &str,
        value: &Value,
    ) -> Result<(), EditError> {
        check_mark_key(key)?;
        check_storable(value)?;
        self.edit(story, |document, handle, positions| {
            let range = positions.range(handle, &range)?;
            if marks::is_run_property(key) {
                let property = format!("rPr.{}", key.trim_start_matches("r:"));
                for map in document.paragraph_maps_in(handle, &range)? {
                    map.set(&property, value)?;
                }
                let end = if range.end > range.start
                    && handle.char_at(range.end - 1) == Some(PARAGRAPH_END)
                {
                    range.end - 1
                } else {
                    range.end
                };
                handle.mark(range.start..end, key, value)?;
            } else {
                handle.mark(range, key, value)?;
            }
            Ok(())
        })
    }

    /// Removes the mark `key` from `range` of `story` (and, for a run property, from the paragraph-mark formatting of the paragraph ends in the range).
    ///
    /// # Errors
    ///
    /// As for [`Document::format`].
    pub fn clear_format(
        &mut self,
        story: EntityId,
        range: Range<usize>,
        key: &str,
    ) -> Result<(), EditError> {
        check_mark_key(key)?;
        self.edit(story, |document, handle, positions| {
            let range = positions.range(handle, &range)?;
            if marks::is_run_property(key) {
                let property = format!("rPr.{}", key.trim_start_matches("r:"));
                for map in document.paragraph_maps_in(handle, &range)? {
                    map.remove(&property)?;
                }
            }
            handle.unmark(range, key)?;
            Ok(())
        })
    }

    // ----- Paragraphs -----

    /// Splits the paragraph at `pos` of `story` (Enter): a new paragraph end is inserted at `pos`, ending a new paragraph with the text before it, which takes a copy of the properties of the paragraph it was split from. Returns the new paragraph's identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn split_paragraph(&mut self, story: EntityId, pos: usize) -> Result<EntityId, EditError> {
        self.edit(story, |document, handle, positions| {
            let pos = positions.insertion_point(handle, pos)?;
            let props = document
                .next_paragraph(handle, pos)
                .map(|paragraph| document.paragraph_props(paragraph))
                .unwrap_or_default();
            let paragraph = document.new_paragraph(&props)?;
            document.crdt.commit_without_undo();
            handle.insert_atom(
                pos,
                PARAGRAPH_END,
                &encode_binding(AtomKind::ParagraphEnd, paragraph),
            )?;
            Ok(paragraph)
        })
    }

    /// Merges the paragraph whose end is at `pos` of `story` with the next one (Delete at the end of a paragraph): the paragraph end is deleted, and the merged paragraph keeps the next paragraph's properties (which survive in the atom model; Word's exact rule is a Word Behavior Note). The final paragraph end cannot be merged, and a paragraph cannot be merged into a table.
    ///
    /// # Errors
    ///
    /// [`EditError::NotAParagraphEnd`], [`EditError::WouldBreakBlockStructure`], or the errors of the CRDT.
    pub fn merge_paragraph(&mut self, story: EntityId, pos: usize) -> Result<(), EditError> {
        self.edit(story, |_, handle, positions| {
            let pos = positions
                .character(pos)
                .map_err(|_| EditError::NotAParagraphEnd)?;
            if handle.char_at(pos) != Some(PARAGRAPH_END) || pos + 1 >= handle.len() {
                return Err(EditError::NotAParagraphEnd);
            }
            if would_break_blocks(handle, &(pos..pos + 1)) {
                return Err(EditError::WouldBreakBlockStructure);
            }
            handle.delete(pos..pos + 1)?;
            Ok(())
        })
    }

    /// Sets a property of a paragraph.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchParagraph`], or the errors of the CRDT.
    pub fn set_paragraph_property(
        &mut self,
        paragraph: EntityId,
        key: &str,
        value: &Value,
    ) -> Result<(), EditError> {
        self.check_attached()?;
        check_storable(value)?;
        let map = self
            .registry(registry::PARAGRAPHS)?
            .get(&paragraph.to_string())
            .ok_or(EditError::NoSuchParagraph(paragraph))?;
        map.set(key, value)?;
        self.crdt.commit();
        Ok(())
    }

    // ----- Tables -----

    /// Inserts a table of `rows` × `columns` empty cells at block position `pos` of `story`. Returns its identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::NotABlockPosition`], [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn insert_table(
        &mut self,
        story: EntityId,
        pos: usize,
        rows: usize,
        columns: usize,
    ) -> Result<EntityId, EditError> {
        if rows == 0 || columns == 0 {
            return Err(EditError::LastRowOrColumn);
        }
        self.edit(story, |document, handle, positions| {
            let pos = positions.insertion_point(handle, pos)?;
            if !at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            let table = document.ids.next_id();
            let map = document
                .registry(registry::TABLES)?
                .create(&table.to_string())?;
            map.set("style", &Value::from("TableGrid"))?;
            let list = map.id_list(registry::ROWS_KEY)?;
            for index in 0..rows {
                let row = document.new_row(columns)?;
                list.insert(index, &row.to_string())?;
            }
            document.crdt.commit_without_undo();
            handle.insert_atom(
                pos,
                TABLE_BLOCK,
                &encode_binding(AtomKind::TableBlock, table),
            )?;
            Ok(table)
        })
    }

    /// Inserts a row before row `index` of `table` (`index` equal to the row count appends), with as many cells as the row it is inserted next to. Returns its identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn insert_row(&mut self, table: EntityId, index: usize) -> Result<EntityId, EditError> {
        self.check_attached()?;
        let list = self.rows_list(table)?;
        if index > list.len() {
            return Err(EditError::InvalidPosition {
                pos: index,
                len: list.len(),
            });
        }
        let neighbor = list
            .items()
            .get(index.min(list.len().saturating_sub(1)))
            .and_then(|item| item.as_str().and_then(EntityId::parse));
        let columns = neighbor
            .and_then(|row| self.cells_list(row))
            .map_or(1, |cells| cells.len().max(1));
        let row = self.new_row(columns)?;
        self.crdt.commit_without_undo();
        list.insert(index, &row.to_string())?;
        self.crdt.commit();
        Ok(row)
    }

    /// Deletes row `index` of `table`, which must keep at least one row.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], [`EditError::InvalidPosition`], [`EditError::LastRowOrColumn`], or the errors of the CRDT.
    pub fn delete_row(&mut self, table: EntityId, index: usize) -> Result<(), EditError> {
        self.check_attached()?;
        let list = self.rows_list(table)?;
        if index >= list.len() {
            return Err(EditError::InvalidPosition {
                pos: index,
                len: list.len(),
            });
        }
        if list.len() == 1 {
            return Err(EditError::LastRowOrColumn);
        }
        list.delete(index)?;
        self.crdt.commit();
        Ok(())
    }

    /// Moves row `from` of `table` to position `to`.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], or the errors of the CRDT.
    pub fn move_row(&mut self, table: EntityId, from: usize, to: usize) -> Result<(), EditError> {
        self.check_attached()?;
        self.rows_list(table)?.move_item(from, to)?;
        self.crdt.commit();
        Ok(())
    }

    /// Inserts a column before column `index` of `table`: a new cell in every row, at `index` or at the end of a shorter row. `index` may be at most the number of cells of the longest row (which appends). A row that the stored state lists twice gets one cell, as the view shows it once.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn insert_column(&mut self, table: EntityId, index: usize) -> Result<(), EditError> {
        self.check_attached()?;
        let rows = self.cell_lists(table)?;
        let widest = rows.iter().map(bayan_crdt::IdList::len).max().unwrap_or(0);
        if index > widest {
            return Err(EditError::InvalidPosition {
                pos: index,
                len: widest,
            });
        }
        let mut new_cells = Vec::with_capacity(rows.len());
        for cells in rows {
            new_cells.push((cells, self.new_cell()?));
        }
        self.crdt.commit_without_undo();
        for (cells, cell) in new_cells {
            cells.insert(index.min(cells.len()), &cell.to_string())?;
        }
        self.crdt.commit();
        Ok(())
    }

    /// Deletes column `index` of `table`: the cell at `index` of every row that has one and keeps at least one cell. Refused when no row has a cell at `index`, or when no row could lose one. A row that the stored state lists twice loses one cell, as the view shows it once.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], [`EditError::InvalidPosition`], [`EditError::LastRowOrColumn`], or the errors of the CRDT.
    pub fn delete_column(&mut self, table: EntityId, index: usize) -> Result<(), EditError> {
        self.check_attached()?;
        let rows = self.cell_lists(table)?;
        let widest = rows.iter().map(bayan_crdt::IdList::len).max().unwrap_or(0);
        if index >= widest {
            return Err(EditError::InvalidPosition {
                pos: index,
                len: widest,
            });
        }
        let lists: Vec<_> = rows
            .into_iter()
            .filter(|cells| cells.len() > index && cells.len() > 1)
            .collect();
        if lists.is_empty() {
            return Err(EditError::LastRowOrColumn);
        }
        for cells in lists {
            cells.delete(index)?;
        }
        self.crdt.commit();
        Ok(())
    }

    // ----- Comments, fields, objects, bookmarks -----

    /// Adds a comment by `author` with text `text` over `range` of `story`: a comment entity with its own story, the highlight mark `cmt:<id>` on the range (it never expands), and a comment reference atom right after the range. The range may not be empty, and may not end at the block position before a table, where the reference would leave the table in the middle of a paragraph. Returns its identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::NotABlockPosition`], [`EditError::InvalidText`], or the errors of the CRDT.
    pub fn add_comment(
        &mut self,
        story: EntityId,
        range: Range<usize>,
        author: &str,
        text: &str,
    ) -> Result<EntityId, EditError> {
        if !text.chars().all(is_text_character) {
            return Err(EditError::InvalidText);
        }
        if range.is_empty() {
            return Err(EditError::InvalidPosition {
                pos: range.start,
                len: self.story_len(story).unwrap_or(0),
            });
        }
        self.edit(story, |document, handle, positions| {
            let range = positions.inner_range(handle, &range)?;
            if before_block_at_block_position(handle, range.end) {
                return Err(EditError::NotABlockPosition);
            }
            let comment = document.ids.next_id();
            let comment_story = document.new_story(text)?;
            let map = document
                .registry(registry::COMMENTS)?
                .create(&comment.to_string())?;
            map.set("author", &Value::from(author))?;
            map.set(registry::STORY, &Value::Str(comment_story.to_string()))?;
            document.crdt.commit_without_undo();
            handle.insert_atom(
                range.end,
                AtomKind::CommentReference.placeholder(),
                &encode_binding(AtomKind::CommentReference, comment),
            )?;
            handle.mark(range, &marks::comment_key(comment), &Value::Bool(true))?;
            Ok(comment)
        })
    }

    /// Inserts a field at `pos` of `story`: begin, the code `instruction`, separator, the cached `result`, end. Returns its identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::NotABlockPosition`], [`EditError::InvalidText`], or the errors of the CRDT.
    pub fn insert_field(
        &mut self,
        story: EntityId,
        pos: usize,
        instruction: &str,
        result: &str,
    ) -> Result<EntityId, EditError> {
        if instruction.is_empty()
            || !instruction
                .chars()
                .chain(result.chars())
                .all(is_text_character)
        {
            return Err(EditError::InvalidText);
        }
        self.edit(story, |document, handle, positions| {
            let mut pos = positions.insertion_point(handle, pos)?;
            if before_block_at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            let field = document.ids.next_id();
            document
                .registry(registry::FIELDS)?
                .create(&field.to_string())?
                .set("instr", &Value::from(instruction))?;
            document.crdt.commit_without_undo();
            for (kind, text) in [
                (AtomKind::FieldBegin, instruction),
                (AtomKind::FieldSeparator, result),
                (AtomKind::FieldEnd, ""),
            ] {
                handle.insert_atom(pos, kind.placeholder(), &encode_binding(kind, field))?;
                pos += 1;
                handle.insert(pos, text)?;
                pos += text.chars().count();
            }
            Ok(field)
        })
    }

    /// Inserts an inline picture placeholder (an object anchor and its object) at `pos` of `story`. Returns the object's identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::NotABlockPosition`], or the errors of the CRDT.
    pub fn insert_object(&mut self, story: EntityId, pos: usize) -> Result<EntityId, EditError> {
        self.edit(story, |document, handle, positions| {
            let pos = positions.insertion_point(handle, pos)?;
            if before_block_at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            let object = document.ids.next_id();
            let map = document
                .registry(registry::OBJECTS)?
                .create(&object.to_string())?;
            map.set("kind", &Value::from("picture"))?;
            map.set("cx", &Value::Int(914_400))?;
            map.set("cy", &Value::Int(914_400))?;
            document.crdt.commit_without_undo();
            handle.insert_atom(
                pos,
                AtomKind::ObjectAnchor.placeholder(),
                &encode_binding(AtomKind::ObjectAnchor, object),
            )?;
            Ok(object)
        })
    }

    /// Inserts a bookmark named `name` around `range` of `story`: a range start before it and a range end after it. Neither may stand at the block position before a table, where it would leave the table in the middle of a paragraph. Returns the range's identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::NotABlockPosition`], or the errors of the CRDT.
    pub fn insert_bookmark(
        &mut self,
        story: EntityId,
        range: Range<usize>,
        name: &str,
    ) -> Result<EntityId, EditError> {
        self.edit(story, |document, handle, positions| {
            let range = positions.inner_range(handle, &range)?;
            if before_block_at_block_position(handle, range.start)
                || before_block_at_block_position(handle, range.end)
            {
                return Err(EditError::NotABlockPosition);
            }
            let bookmark = document.ids.next_id();
            let map = document
                .registry(registry::RANGES)?
                .create(&bookmark.to_string())?;
            map.set("kind", &Value::from("bookmark"))?;
            map.set("name", &Value::from(name))?;
            document.crdt.commit_without_undo();
            handle.insert_atom(
                range.end,
                AtomKind::RangeEnd.placeholder(),
                &encode_binding(AtomKind::RangeEnd, bookmark),
            )?;
            handle.insert_atom(
                range.start,
                AtomKind::RangeStart.placeholder(),
                &encode_binding(AtomKind::RangeStart, bookmark),
            )?;
            Ok(bookmark)
        })
    }

    /// Moves `range` of story `from` to position `to_pos` of story `to` (cut and paste of the same content): text keeps its marks, and atoms keep their entities, so a table, object, field or comment moves with its identity. Everything is checked before anything is cut: the range must hold whole fields and both ends of every range in it ([`EditError::WouldUnbalance`]), a move may not put a table or a comment reference into a story inside it, or a comment reference into a comment ([`EditError::WouldCreateCycle`]), content that starts with a table may land only at a block position, as for [`Document::insert_table`], and content that does not end with a paragraph end or a table may not land at the block position before a table, as for typing ([`EditError::NotABlockPosition`]); marks whose value the adapter cannot store (which another replica may have stored) are not pasted, as the view does not show them. Moving content to its own start or end changes nothing. Replicas that move the same content concurrently can still duplicate a reference, which normalization resolves (N7).
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchStory`], [`EditError::InvalidPosition`], [`EditError::WouldCreateCycle`], [`EditError::WouldBreakBlockStructure`], [`EditError::WouldUnbalance`], [`EditError::NotABlockPosition`], or the errors of the CRDT.
    pub fn move_range(
        &mut self,
        from: EntityId,
        range: Range<usize>,
        to: EntityId,
        to_pos: usize,
    ) -> Result<(), EditError> {
        self.check_attached()?;
        let same = from == to;
        let source = self.story(from)?;
        let target = if same {
            source.clone()
        } else {
            self.story(to)?
        };
        // Both stories are materialized first, each as a change of its own that is not an undo step, so that the positions refer to stored structure.
        let source_positions = self.prepare(from, &source)?;
        let target_positions = if same {
            source_positions.clone()
        } else {
            self.prepare(to, &target)?
        };
        let cut = source_positions.inner_range(&source, &range)?;
        if cut.is_empty() {
            return Err(source_positions.invalid(range.start));
        }
        let paste_at = target_positions.insertion_point(&target, to_pos)?;
        if same && paste_at > cut.start && paste_at < cut.end {
            return Err(target_positions.invalid(to_pos));
        }
        if would_break_blocks(&source, &cut) {
            return Err(EditError::WouldBreakBlockStructure);
        }
        check_whole_markers(&source, &cut)?;
        let runs = source.runs_in(cut.clone())?;
        self.check_move_target(&runs, to)?;
        if same && (paste_at == cut.start || paste_at == cut.end) {
            return Ok(());
        }
        // Where the content lands once it is cut out.
        let paste = if same && paste_at >= cut.end {
            paste_at - cut.len()
        } else {
            paste_at
        };
        let ends_a_block = runs
            .last()
            .and_then(|run| run.text.chars().last())
            .is_some_and(|last| last == PARAGRAPH_END || last == TABLE_BLOCK);
        if !ends_a_block && before_block_after_cut(&target, paste, same.then_some(&cut)) {
            return Err(EditError::NotABlockPosition);
        }
        // A table lands only at a block position, as `insert_table` requires: elsewhere it would stand in the middle of a paragraph, and the paragraph end that materialization would write there is not part of the move, so undoing the move would leave it.
        let starts_with_table =
            runs.first().and_then(|run| run.text.chars().next()) == Some(TABLE_BLOCK);
        if starts_with_table && !at_block_position_after_cut(&target, paste, same.then_some(&cut)) {
            return Err(EditError::NotABlockPosition);
        }
        source.delete(cut.clone())?;
        let mut pos = paste;
        for run in &runs {
            let len = run.len();
            // A mark whose value the adapter cannot store (another replica may store one) cannot be pasted; the view leaves it out anyway, so it is dropped, as everything is checked before the cut.
            let own: Props = run
                .marks
                .iter()
                .filter(|(_, value)| value.is_storable())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            target.insert(pos, &run.text)?;
            // The pasted characters keep exactly their own marks: marks they inherited by expansion are removed.
            for inherited in target.runs_in(pos..pos + len)? {
                for key in inherited.marks.keys() {
                    if !own.contains_key(key) {
                        target.unmark(pos..pos + len, key)?;
                    }
                }
            }
            for (key, value) in &own {
                target.mark(pos..pos + len, key, value)?;
            }
            pos += len;
        }
        self.note_structure(from, &source, cut.start);
        self.note_structure(to, &target, pos);
        self.crdt.commit();
        Ok(())
    }

    // ----- Replication -----

    /// The version vector, which another replica sends to ask for what it lacks.
    #[must_use]
    pub fn version_vector(&self) -> VersionVector {
        self.crdt.version_vector()
    }

    /// The latest version.
    #[must_use]
    pub fn version(&self) -> Version {
        self.crdt.version()
    }

    /// The changes a replica at `since` lacks.
    ///
    /// # Errors
    ///
    /// The errors of the CRDT.
    pub fn export_updates(&self, since: &VersionVector) -> Result<Vec<u8>, EditError> {
        Ok(self.crdt.export_updates(since)?)
    }

    /// A snapshot of everything.
    ///
    /// # Errors
    ///
    /// The errors of the CRDT.
    pub fn export_snapshot(&self) -> Result<Vec<u8>, EditError> {
        Ok(self.crdt.export_snapshot()?)
    }

    /// Imports a blob from another replica within `limits`.
    ///
    /// # Errors
    ///
    /// [`EditError::Import`].
    pub fn import(
        &mut self,
        bytes: &[u8],
        limits: &ImportLimits,
    ) -> Result<ImportReport, EditError> {
        let report = self.crdt.import(bytes, limits).map_err(EditError::Import)?;
        self.clean.clear();
        Ok(report)
    }

    /// Shows the document as it was at `version`; editing is refused until [`Document::checkout_latest`].
    ///
    /// # Errors
    ///
    /// The errors of the CRDT.
    pub fn checkout(&mut self, version: &Version) -> Result<(), EditError> {
        Ok(self.crdt.checkout(version)?)
    }

    /// Returns to the latest version.
    pub fn checkout_latest(&mut self) {
        self.crdt.checkout_latest();
        self.clean.clear();
    }

    // ----- Undo -----

    /// Undoes this replica's latest operation (changes received from other replicas are never undone). Returns whether there was one.
    ///
    /// # Errors
    ///
    /// The errors of the CRDT.
    pub fn undo(&mut self) -> Result<bool, EditError> {
        self.check_attached()?;
        let undone = self.undo.undo()?;
        self.clean.clear();
        Ok(undone)
    }

    /// Redoes the latest undone operation. Returns whether there was one.
    ///
    /// # Errors
    ///
    /// The errors of the CRDT.
    pub fn redo(&mut self) -> Result<bool, EditError> {
        self.check_attached()?;
        let redone = self.undo.redo()?;
        self.clean.clear();
        Ok(redone)
    }

    /// Whether there is an operation to undo.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    /// The number of operations that can be undone.
    #[must_use]
    pub fn undo_count(&self) -> usize {
        self.undo.undo_count()
    }

    /// Whether writes wait for the next commit. Every operation, applied or refused, leaves none (with the one exception that [`EditError`] describes): whatever were pending would become part of the next operation's change and undo step.
    #[must_use]
    pub fn has_pending_changes(&self) -> bool {
        self.crdt.has_pending_changes()
    }

    // ----- Internals -----

    /// Runs one operation on `story` as one transaction: materializes the story if it may need it, gives the operation the mapping from the positions the caller computed to the story after materialization, and commits if the operation succeeds. Operations check everything before they write, so a refused operation leaves nothing pending.
    fn edit<T>(
        &mut self,
        story: EntityId,
        operation: impl FnOnce(&mut Self, &Story, &Positions) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        self.check_attached()?;
        let handle = self.story(story)?;
        let positions = self.prepare(story, &handle)?;
        let result = operation(self, &handle, &positions)?;
        self.crdt.commit();
        Ok(result)
    }

    /// Materializes `story` if it may need it, as a change of its own that is not an undo step (materialization does not change the view), and returns how positions computed on the story before map onto it after.
    fn prepare(&mut self, story: EntityId, handle: &Story) -> Result<Positions, EditError> {
        let len = handle.len();
        let mut inserted = Vec::new();
        if !self.clean.contains(&story) {
            if needs_materialization(handle) {
                // One normalization finds the structure to write, and is the view before it for the check.
                let (before, report) = normalize(&self.raw());
                let written = self.materialize(story, handle, report);
                if let Ok(positions) = &written
                    && !positions.is_empty()
                {
                    self.crdt.commit_without_undo();
                }
                // A materialization that failed may have written part of its structure, so it is checked too (it should never fail: it writes only values the adapter can store).
                if self.check.is_some() && !written.as_ref().is_ok_and(Vec::is_empty) {
                    let after = self.view();
                    if let Some(check) = &mut self.check {
                        check.performed += 1;
                        if written.is_err() {
                            check.failed += 1;
                        }
                        if after != before {
                            check.changed_view += 1;
                        }
                    }
                }
                inserted = written?;
            }
            self.clean.insert(story);
        }
        inserted.sort_unstable();
        Ok(Positions { inserted, len })
    }

    /// Writes into the CRDT the structure that the view of `story` shows but the story does not store (N1 and N4 paragraph ends, as `report`, the normalization of the current state, lists them), so that later edits act on stored structure. Returns the positions, in the story before materialization, where paragraph ends were inserted. The caller commits.
    fn materialize(
        &mut self,
        story: EntityId,
        handle: &Story,
        report: Report,
    ) -> Result<Vec<usize>, EditError> {
        let mut virtual_ends: Vec<_> = report
            .virtual_paragraph_ends
            .into_iter()
            .filter(|virtual_end| virtual_end.story == story)
            .collect();
        virtual_ends.sort_by_key(|virtual_end| std::cmp::Reverse(virtual_end.raw_position));
        let mut inserted = Vec::with_capacity(virtual_ends.len());
        for virtual_end in virtual_ends {
            let map = self.paragraph_map(virtual_end.id)?;
            for (key, value) in &virtual_end.props {
                map.set(key, value)?;
            }
            let pos = virtual_end.raw_position.min(handle.len());
            handle.insert_atom(
                pos,
                PARAGRAPH_END,
                &encode_binding(AtomKind::ParagraphEnd, virtual_end.id),
            )?;
            inserted.push(pos);
        }
        Ok(inserted)
    }

    /// Marks `story` for a materialization check when a local edit may have placed a table at a position that is no longer a block position.
    fn note_structure(&mut self, story: EntityId, handle: &Story, pos: usize) {
        if handle.char_at(pos) == Some(TABLE_BLOCK) && !at_block_position(handle, pos) {
            self.clean.remove(&story);
        }
    }

    fn check_attached(&self) -> Result<(), EditError> {
        if self.crdt.is_poisoned() {
            // A hostile import poisoned the replica; it must be discarded (see `ImportError::Panicked`).
            Err(EditError::Crdt(CrdtError::Poisoned))
        } else if self.crdt.is_detached() {
            Err(EditError::Detached)
        } else {
            Ok(())
        }
    }

    fn story(&self, id: EntityId) -> Result<Story, EditError> {
        if id == EntityId::MAIN_STORY {
            Ok(self.crdt.main_story())
        } else {
            self.crdt
                .stories()
                .get(&id.to_string())
                .ok_or(EditError::NoSuchStory(id))
        }
    }

    /// The registry `name` of the CRDT (one of the [`registry`] names, which are all valid root names).
    fn registry(&self, name: &str) -> Result<bayan_crdt::Registry, EditError> {
        Ok(self.crdt.registry(name)?)
    }

    /// Creates the property map of `paragraph`, an identifier with no entry in the paragraph registry yet (a new one, or one that normalization derived, which avoids every identifier with an entry): the library refuses to create a map over a value or over a map it did not create this way.
    fn paragraph_map(&self, paragraph: EntityId) -> Result<PropertyMap, EditError> {
        Ok(self
            .registry(registry::PARAGRAPHS)?
            .create(&paragraph.to_string())?)
    }

    /// The property maps of the paragraph ends in `range` of a story, all obtained before the caller writes into any of them. A paragraph end whose entry in the paragraph registry is not a map (missing, or a value another replica stored) is left out: the view does not show it (N5), and creating a map for it could fail halfway or make it appear. A map that another replica stored by other means is written into, as the view already shows its properties.
    fn paragraph_maps_in(
        &self,
        handle: &Story,
        range: &Range<usize>,
    ) -> Result<Vec<PropertyMap>, EditError> {
        let paragraphs = self.registry(registry::PARAGRAPHS)?;
        Ok(self
            .atoms_in(handle, range, AtomKind::ParagraphEnd)?
            .into_iter()
            .filter_map(|paragraph| paragraphs.get(&paragraph.to_string()))
            .collect())
    }

    /// The properties of a stored paragraph that can be copied: values the adapter cannot store (which another replica may have stored) are left out, as the view leaves them out, so that copying them never fails halfway.
    fn paragraph_props(&self, paragraph: EntityId) -> Props {
        self.registry(registry::PARAGRAPHS)
            .ok()
            .and_then(|paragraphs| paragraphs.get(&paragraph.to_string()))
            .map(|map| map.entries())
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, value)| value.is_storable())
            .collect()
    }

    fn new_paragraph(&mut self, props: &Props) -> Result<EntityId, EditError> {
        let paragraph = self.ids.next_id();
        let map = self.paragraph_map(paragraph)?;
        for (key, value) in props {
            map.set(key, value)?;
        }
        Ok(paragraph)
    }

    /// A new story holding `text` and its final paragraph end.
    fn new_story(&mut self, text: &str) -> Result<EntityId, EditError> {
        let story = self.ids.next_id();
        let handle = self.crdt.stories().create(&story.to_string())?;
        handle.insert(0, text)?;
        let paragraph = self.new_paragraph(&BTreeMap::from([(
            "style".to_owned(),
            Value::from("Normal"),
        )]))?;
        handle.insert_atom(
            handle.len(),
            PARAGRAPH_END,
            &encode_binding(AtomKind::ParagraphEnd, paragraph),
        )?;
        Ok(story)
    }

    fn new_cell(&mut self) -> Result<EntityId, EditError> {
        let cell = self.ids.next_id();
        let story = self.new_story("")?;
        let map = self.registry(registry::CELLS)?.create(&cell.to_string())?;
        map.set("w", &Value::Int(2_000))?;
        map.set(registry::STORY, &Value::Str(story.to_string()))?;
        Ok(cell)
    }

    fn new_row(&mut self, cells: usize) -> Result<EntityId, EditError> {
        let row = self.ids.next_id();
        let list = self
            .registry(registry::ROWS)?
            .create(&row.to_string())?
            .id_list(registry::CELLS_KEY)?;
        for index in 0..cells {
            let cell = self.new_cell()?;
            list.insert(index, &cell.to_string())?;
        }
        Ok(row)
    }

    fn rows_list(&self, table: EntityId) -> Result<bayan_crdt::IdList, EditError> {
        self.registry(registry::TABLES)?
            .get(&table.to_string())
            .and_then(|map| map.get_id_list(registry::ROWS_KEY))
            .ok_or(EditError::NoSuchTable(table))
    }

    /// The cell list of a stored row, if the row exists and has one.
    fn cells_list(&self, row: EntityId) -> Option<bayan_crdt::IdList> {
        self.registry(registry::ROWS)
            .ok()?
            .get(&row.to_string())
            .and_then(|map| map.get_id_list(registry::CELLS_KEY))
    }

    fn table_row_ids(&self, table: EntityId) -> Result<Vec<EntityId>, EditError> {
        Ok(self
            .rows_list(table)?
            .items()
            .iter()
            .filter_map(|item| item.as_str().and_then(EntityId::parse))
            .collect())
    }

    /// The cell lists of the stored rows of `table`, each row once, where it first appears. The stored list of rows can hold a row twice (two replicas delete the same row and both undo), which the view shows once (N7); an operation that changed every entry would change that row twice, and a deletion could fail halfway.
    fn cell_lists(&self, table: EntityId) -> Result<Vec<bayan_crdt::IdList>, EditError> {
        let mut seen = BTreeSet::new();
        Ok(self
            .table_row_ids(table)?
            .into_iter()
            .filter(|row| seen.insert(*row))
            .filter_map(|row| self.cells_list(row))
            .collect())
    }

    /// The entities of the atoms of `kind` in `range` of a story.
    fn atoms_in(
        &self,
        handle: &Story,
        range: &Range<usize>,
        kind: AtomKind,
    ) -> Result<Vec<EntityId>, EditError> {
        let mut ids = Vec::new();
        for run in handle.runs_in(range.clone())? {
            if !run.text.contains(kind.placeholder()) {
                continue;
            }
            if let Some((bound, id)) = run
                .marks
                .get(ATOM_KEY)
                .and_then(Value::as_str)
                .and_then(decode_binding)
                && bound == kind
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// The paragraph that position `pos` belongs to: the first paragraph end at or after it.
    fn next_paragraph(&self, handle: &Story, pos: usize) -> Option<EntityId> {
        let len = handle.len();
        let text: Vec<char> = handle.text().chars().collect();
        let end = (pos..len).find(|at| text.get(*at) == Some(&PARAGRAPH_END))?;
        self.atoms_in(handle, &(end..end + 1), AtomKind::ParagraphEnd)
            .ok()?
            .first()
            .copied()
    }

    /// Refuses moves that would put content inside itself, which the view could then never reach: a table, or a comment reference, into a story inside it (a story of one of the table's cells, the comment's own story, or any story nested inside those, through tables and comment references). Also refuses a comment reference into a comment's story (comments do not hold comments).
    fn check_move_target(
        &self,
        runs: &[bayan_crdt::Run],
        target: EntityId,
    ) -> Result<(), EditError> {
        let moved: Vec<(AtomKind, EntityId)> = runs.iter().filter_map(holder_binding).collect();
        if moved.is_empty() {
            return Ok(());
        }
        let raw = self.raw();
        let moves_comment = moved
            .iter()
            .any(|(kind, _)| *kind == AtomKind::CommentReference);
        if moves_comment
            && raw
                .comments
                .values()
                .any(|comment| story_of(comment) == Some(target))
        {
            return Err(EditError::WouldCreateCycle);
        }
        if stories_inside(&raw, moved).contains(&target) {
            return Err(EditError::WouldCreateCycle);
        }
        Ok(())
    }
}

/// The table or comment reference that a run's binding names, if the run holds its placeholder: the atoms that hold stories.
fn holder_binding(run: &Run) -> Option<(AtomKind, EntityId)> {
    run.marks
        .get(ATOM_KEY)
        .and_then(Value::as_str)
        .and_then(decode_binding)
        .filter(|(kind, _)| {
            matches!(kind, AtomKind::TableBlock | AtomKind::CommentReference)
                && run.text.contains(kind.placeholder())
        })
}

/// The story that a cell's or a comment's properties name.
fn story_of(props: &Props) -> Option<EntityId> {
    props
        .get(registry::STORY)
        .and_then(Value::as_str)
        .and_then(EntityId::parse)
}

/// Every story inside the given tables and comment references, at any depth: the stories of the tables' cells and the comments' own stories, then, for every table and comment reference in those stories, theirs, and so on.
fn stories_inside(raw: &RawDocument, holders: Vec<(AtomKind, EntityId)>) -> BTreeSet<EntityId> {
    let mut pending = holders;
    let mut visited = BTreeSet::new();
    let mut inside = BTreeSet::new();
    while let Some((kind, id)) = pending.pop() {
        if !visited.insert((kind, id)) {
            continue;
        }
        let stories: Vec<EntityId> = match kind {
            AtomKind::TableBlock => raw
                .tables
                .get(&id)
                .map(|table| {
                    table
                        .rows
                        .iter()
                        .filter_map(|item| item.as_str().and_then(EntityId::parse))
                        .filter_map(|row| raw.rows.get(&row))
                        .flat_map(|row| {
                            row.cells
                                .iter()
                                .filter_map(|item| item.as_str().and_then(EntityId::parse))
                        })
                        .filter_map(|cell| raw.cells.get(&cell).and_then(story_of))
                        .collect()
                })
                .unwrap_or_default(),
            AtomKind::CommentReference => raw
                .comments
                .get(&id)
                .and_then(story_of)
                .into_iter()
                .collect(),
            _ => Vec::new(),
        };
        for story in stories {
            if inside.insert(story)
                && let Some(runs) = raw.stories.get(&story)
            {
                pending.extend(runs.iter().filter_map(holder_binding));
            }
        }
    }
    inside
}

/// Whether a story may need materialization: it does not end with a paragraph end, or a table follows something other than a paragraph end or another table. A cheap check of the stored characters; the normalization decides.
fn needs_materialization(handle: &Story) -> bool {
    let mut previous = None;
    for character in handle.text().chars() {
        if character == TABLE_BLOCK && !matches!(previous, None | Some(PARAGRAPH_END | TABLE_BLOCK))
        {
            return true;
        }
        previous = Some(character);
    }
    previous != Some(PARAGRAPH_END)
}

/// Whether `pos` is a block position: the start of the story, or right after a paragraph end or a table.
fn at_block_position(handle: &Story, pos: usize) -> bool {
    pos == 0 || matches!(handle.char_at(pos - 1), Some(PARAGRAPH_END | TABLE_BLOCK))
}

/// Whether `pos` is the block position right before a table, where typing would put text before the table in the same paragraph.
fn before_block_at_block_position(handle: &Story, pos: usize) -> bool {
    handle.char_at(pos) == Some(TABLE_BLOCK) && at_block_position(handle, pos)
}

/// Whether deleting `range` would leave the atom after it, a table, at a position that is not a block position.
fn would_break_blocks(handle: &Story, range: &Range<usize>) -> bool {
    handle.char_at(range.end) == Some(TABLE_BLOCK) && !at_block_position(handle, range.start)
}

/// Whether `pos` is the block position right before a table in the story as it is once `cut` (a range of the same story, if any) is removed.
fn before_block_after_cut(handle: &Story, pos: usize, cut: Option<&Range<usize>>) -> bool {
    handle.char_at(position_before_cut(pos, cut)) == Some(TABLE_BLOCK)
        && at_block_position_after_cut(handle, pos, cut)
}

/// Whether `pos` is a block position in the story as it is once `cut` (a range of the same story, if any) is removed.
fn at_block_position_after_cut(handle: &Story, pos: usize, cut: Option<&Range<usize>>) -> bool {
    pos == 0
        || matches!(
            handle.char_at(position_before_cut(pos - 1, cut)),
            Some(PARAGRAPH_END | TABLE_BLOCK)
        )
}

/// The position in the story before `cut` was removed of position `at` in the story after.
fn position_before_cut(at: usize, cut: Option<&Range<usize>>) -> usize {
    match cut {
        Some(cut) if at >= cut.start => at + cut.len(),
        _ => at,
    }
}

/// Refuses a value the adapter cannot store before anything is written (writing it would fail, possibly after other writes of the same operation).
fn check_storable(value: &Value) -> Result<(), EditError> {
    if value.is_storable() {
        Ok(())
    } else {
        Err(EditError::Crdt(CrdtError::InvalidValue(
            "unsupported value",
        )))
    }
}

/// Refuses the atom binding, which only the adapter writes, and keys that name no mark of the model: a key is `family:name`, with a name, in a family of the model whose expansion every replica configures, or `link` itself, the one key of the hyperlink family (document model §5). A bare family name such as `r` would be a mark that no rule of the model handles (a run property is stopped before a closing paragraph end only when its key starts with `r:`).
fn check_mark_key(key: &str) -> Result<(), EditError> {
    let valid = match key.split_once(':') {
        Some((family, name)) => {
            !name.is_empty()
                && family != marks::LINK
                && marks::FAMILIES.iter().any(|known| known.name == family)
        }
        None => key == marks::LINK,
    };
    if valid && key != ATOM_KEY {
        Ok(())
    } else {
        Err(EditError::InvalidKey)
    }
}

/// Refuses to cut `range` out of a story when it holds some but not all of the delimiters of a field (begin, separator, end), or one end of a range without the other: what is left would be repaired by normalization (N2, N3) into something nobody wrote, such as a field code shown as text.
///
/// Only the first delimiter of each role and entity in the story counts, because that is the one the view keeps: when replicas moved the same field or range concurrently, the story holds a second copy, which normalization drops (N7), so either copy may be deleted or moved whole, and the second copy's delimiters never refuse a cut. (N7 also drops a copy when an earlier story in document order holds one; this check looks at one story, so it may refuse a partial cut of such a copy, which the view does not show: a refusal, never a broken field.)
fn check_whole_markers(handle: &Story, range: &Range<usize>) -> Result<(), EditError> {
    if range.is_empty() || !holds_markers(&handle.runs_in(range.clone())?) {
        return Ok(());
    }
    let mut first = BTreeSet::new();
    let mut inside: BTreeMap<EntityId, usize> = BTreeMap::new();
    let mut everywhere: BTreeMap<EntityId, usize> = BTreeMap::new();
    let mut pos = 0;
    for run in handle.runs() {
        let start = pos;
        pos += run.len();
        let Some((kind, id)) = marker_binding(&run) else {
            continue;
        };
        for (offset, character) in run.text.chars().enumerate() {
            if character == kind.placeholder() && first.insert((kind, id)) {
                *everywhere.entry(id).or_insert(0) += 1;
                if range.contains(&(start + offset)) {
                    *inside.entry(id).or_insert(0) += 1;
                }
            }
        }
    }
    if inside
        .iter()
        .any(|(entity, count)| everywhere.get(entity) != Some(count))
    {
        return Err(EditError::WouldUnbalance);
    }
    Ok(())
}

/// Whether the runs hold a field or range delimiter.
fn holds_markers(runs: &[Run]) -> bool {
    runs.iter().any(|run| {
        marker_binding(run).is_some_and(|(kind, _)| run.text.contains(kind.placeholder()))
    })
}

/// The field or range delimiter that a run's binding names, if it names one.
fn marker_binding(run: &Run) -> Option<(AtomKind, EntityId)> {
    run.marks
        .get(ATOM_KEY)
        .and_then(Value::as_str)
        .and_then(decode_binding)
        .filter(|(kind, _)| {
            matches!(
                kind,
                AtomKind::FieldBegin
                    | AtomKind::FieldSeparator
                    | AtomKind::FieldEnd
                    | AtomKind::RangeStart
                    | AtomKind::RangeEnd
            )
        })
}

/// How positions that a caller computed on a story before its materialization map onto the story after it, and the checks of those positions. A refused position is reported as the caller gave it, with the length of the story the caller saw.
///
/// Materialization inserts paragraph ends. A position between two characters (where something is inserted, or where a range ends) keeps its place before a paragraph end inserted there; the position of a character (where a range starts, or a paragraph end to merge) follows the character.
#[derive(Debug, Clone)]
struct Positions {
    /// Where materialization inserted paragraph ends, in the story before it, in increasing order.
    inserted: Vec<usize>,
    /// The length of the story before materialization, which is what the caller saw.
    len: usize,
}

impl Positions {
    /// The error for a position the caller gave.
    const fn invalid(&self, pos: usize) -> EditError {
        EditError::InvalidPosition { pos, len: self.len }
    }

    /// The position between two characters, after materialization.
    fn boundary(&self, pos: usize) -> Result<usize, EditError> {
        if pos > self.len {
            return Err(self.invalid(pos));
        }
        Ok(pos + self.inserted.iter().filter(|at| **at < pos).count())
    }

    /// The position of the character at `pos`, after materialization.
    fn character(&self, pos: usize) -> Result<usize, EditError> {
        if pos >= self.len {
            return Err(self.invalid(pos));
        }
        Ok(pos + self.inserted.iter().filter(|at| **at <= pos).count())
    }

    /// A position where something may be inserted: anywhere before the final paragraph end.
    fn insertion_point(&self, handle: &Story, pos: usize) -> Result<usize, EditError> {
        let at = self.boundary(pos)?;
        if at >= handle.len() {
            return Err(self.invalid(pos));
        }
        Ok(at)
    }

    /// A range inside the story.
    fn range(&self, handle: &Story, range: &Range<usize>) -> Result<Range<usize>, EditError> {
        if range.start > range.end {
            return Err(self.invalid(range.start));
        }
        let end = self.boundary(range.end)?;
        let start = if range.is_empty() {
            end
        } else {
            self.character(range.start)?
        };
        if end > handle.len() {
            return Err(self.invalid(range.end));
        }
        Ok(start..end)
    }

    /// A range that ends before the final paragraph end: one that may be deleted, or after which an atom may be inserted.
    fn inner_range(&self, handle: &Story, range: &Range<usize>) -> Result<Range<usize>, EditError> {
        let mapped = self.range(handle, range)?;
        if mapped.end >= handle.len() {
            return Err(self.invalid(range.end));
        }
        Ok(mapped)
    }
}
