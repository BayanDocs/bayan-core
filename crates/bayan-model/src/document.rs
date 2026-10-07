//! A document stored in the CRDT, and the operations of the CORE-004 brief.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;

use bayan_crdt::{
    ATOM_KEY, CrdtError, Doc, ImportError, ImportLimits, ImportReport, PeerId, PropertyMap, Story,
    UndoManager, Value, Version, VersionVector,
};

use crate::atoms::{decode_binding, encode_binding, is_text_character};
use crate::raw::{RawDocument, registry};
use crate::view::View;
use crate::{AtomKind, EntityId, IdGenerator, Props, SECTION_DEFAULTS, marks, normalize};

const PARAGRAPH_END: char = '\u{0D}';
const TABLE_BLOCK: char = '\u{07}';

/// Why an operation was refused. A refused operation changes nothing that the next successful one would not also have written (see [`Document`]).
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
/// **Materialization** (document model §14): when an operation edits a story whose view contains structure that only normalization added (a final paragraph end, or a paragraph end before a table found mid-paragraph), the operation first writes that structure into the CRDT, with the identifiers and properties the view already shows. The view therefore does not change by materialization, and concurrent materializations by several replicas merge into the same entities. Materialization is committed as its own change and is not an undo step: undoing the operation returns the view to the state before the operation without removing the materialized structure, which the view showed anyway.
#[derive(Debug)]
pub struct Document {
    crdt: Doc,
    ids: IdGenerator,
    undo: UndoManager,
    /// Stories known not to need materialization since the last import or undo.
    clean: BTreeSet<EntityId>,
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
        let body = document.crdt.root_map(registry::BODY);
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
        }
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

    /// The stored characters of a story, placeholders included, if it exists.
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
        self.crdt
            .registry(registry::TABLES)
            .ids()
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
        self.edit(story, |document, handle, shift| {
            let pos = shift(pos);
            check_inner_position(handle, pos)?;
            if before_block_at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            handle.insert(pos, text)?;
            document.note_structure(story, handle, pos + text.chars().count());
            Ok(())
        })
    }

    /// Deletes `range` of `story`, which may not include the final paragraph end and may not leave a table in the middle of a paragraph. Entities whose atoms are deleted stay stored (undo may restore their reference) but leave the view.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::WouldBreakBlockStructure`], or the errors of the CRDT.
    pub fn delete(&mut self, story: EntityId, range: Range<usize>) -> Result<(), EditError> {
        self.edit(story, |_, handle, shift| {
            let range = shift(range.start)..shift(range.end);
            check_deletable(handle, &range)?;
            if would_break_blocks(handle, &range) {
                return Err(EditError::WouldBreakBlockStructure);
            }
            handle.delete(range)?;
            Ok(())
        })
    }

    /// Sets the mark `key` to `value` on `range` of `story`. A run property (`r:…`) is applied to the characters, stopping before a paragraph end that closes the range, and to the paragraph-mark formatting (`rPr.…` properties) of every paragraph end in the range, because run-property marks expand after their end and would otherwise spread into the next paragraph.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn format(
        &mut self,
        story: EntityId,
        range: Range<usize>,
        key: &str,
        value: &Value,
    ) -> Result<(), EditError> {
        self.edit(story, |document, handle, shift| {
            let range = shift(range.start)..shift(range.end);
            check_range(handle, &range)?;
            if marks::is_run_property(key) {
                let property = format!("rPr.{}", key.trim_start_matches("r:"));
                for paragraph in document.atoms_in(handle, &range, AtomKind::ParagraphEnd)? {
                    document.paragraph_map(paragraph)?.set(&property, value)?;
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
        self.edit(story, |document, handle, shift| {
            let range = shift(range.start)..shift(range.end);
            check_range(handle, &range)?;
            if marks::is_run_property(key) {
                let property = format!("rPr.{}", key.trim_start_matches("r:"));
                for paragraph in document.atoms_in(handle, &range, AtomKind::ParagraphEnd)? {
                    document.paragraph_map(paragraph)?.remove(&property)?;
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
        self.edit(story, |document, handle, shift| {
            let pos = shift(pos);
            check_inner_position(handle, pos)?;
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
        self.edit(story, |_, handle, shift| {
            let pos = shift(pos);
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
        let map = self
            .crdt
            .registry(registry::PARAGRAPHS)
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
        self.edit(story, |document, handle, shift| {
            let pos = shift(pos);
            check_inner_position(handle, pos)?;
            if !at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            let table = document.ids.next_id();
            let map = document
                .crdt
                .registry(registry::TABLES)
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
            .and_then(|row| self.cells_list(row).ok())
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

    /// Inserts a column before column `index` of `table`: a new cell in every row, at `index` or at the end of a shorter row.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], or the errors of the CRDT.
    pub fn insert_column(&mut self, table: EntityId, index: usize) -> Result<(), EditError> {
        self.check_attached()?;
        let mut new_cells = Vec::new();
        for row in self.table_row_ids(table)? {
            let Ok(cells) = self.cells_list(row) else {
                continue;
            };
            new_cells.push((cells, self.new_cell()?));
        }
        self.crdt.commit_without_undo();
        for (cells, cell) in new_cells {
            cells.insert(index.min(cells.len()), &cell.to_string())?;
        }
        self.crdt.commit();
        Ok(())
    }

    /// Deletes column `index` of `table`: the cell at `index` of every row that has one and keeps at least one cell. Refused when no row could lose a cell.
    ///
    /// # Errors
    ///
    /// [`EditError::NoSuchTable`], [`EditError::LastRowOrColumn`], or the errors of the CRDT.
    pub fn delete_column(&mut self, table: EntityId, index: usize) -> Result<(), EditError> {
        self.check_attached()?;
        let lists: Vec<_> = self
            .table_row_ids(table)?
            .into_iter()
            .filter_map(|row| self.cells_list(row).ok())
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

    /// Adds a comment by `author` with text `text` over `range` of `story`: a comment entity with its own story, the highlight mark `cmt:<id>` on the range (it never expands), and a comment reference atom right after the range. Returns its identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::InvalidText`], or the errors of the CRDT.
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
        self.edit(story, |document, handle, shift| {
            let range = shift(range.start)..shift(range.end);
            check_inner_position(handle, range.end)?;
            if range.is_empty() || range.start > range.end {
                return Err(EditError::InvalidPosition {
                    pos: range.start,
                    len: handle.len(),
                });
            }
            let comment = document.ids.next_id();
            let comment_story = document.new_story(text)?;
            let map = document
                .crdt
                .registry(registry::COMMENTS)
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
        self.edit(story, |document, handle, shift| {
            let mut pos = shift(pos);
            check_inner_position(handle, pos)?;
            if before_block_at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            let field = document.ids.next_id();
            document
                .crdt
                .registry(registry::FIELDS)
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
        self.edit(story, |document, handle, shift| {
            let pos = shift(pos);
            check_inner_position(handle, pos)?;
            if before_block_at_block_position(handle, pos) {
                return Err(EditError::NotABlockPosition);
            }
            let object = document.ids.next_id();
            let map = document
                .crdt
                .registry(registry::OBJECTS)
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

    /// Inserts a bookmark named `name` around `range` of `story`: a range start before it and a range end after it. Returns the range's identifier.
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], or the errors of the CRDT.
    pub fn insert_bookmark(
        &mut self,
        story: EntityId,
        range: Range<usize>,
        name: &str,
    ) -> Result<EntityId, EditError> {
        self.edit(story, |document, handle, shift| {
            let range = shift(range.start)..shift(range.end);
            check_inner_position(handle, range.end)?;
            if range.start > range.end {
                return Err(EditError::InvalidPosition {
                    pos: range.start,
                    len: handle.len(),
                });
            }
            let bookmark = document.ids.next_id();
            let map = document
                .crdt
                .registry(registry::RANGES)
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
            document.note_structure(story, handle, range.end + 2);
            Ok(bookmark)
        })
    }

    /// Moves `range` of story `from` to position `to_pos` of story `to` (cut and paste of the same content): text keeps its marks, and atoms keep their entities, so a table, object, field or comment moves with its identity. A move may not put a table inside itself or a comment reference into a comment; replicas that move the same content concurrently can still duplicate a reference, which normalization resolves (N7).
    ///
    /// # Errors
    ///
    /// [`EditError::InvalidPosition`], [`EditError::WouldCreateCycle`], [`EditError::WouldBreakBlockStructure`], or the errors of the CRDT.
    pub fn move_range(
        &mut self,
        from: EntityId,
        range: Range<usize>,
        to: EntityId,
        to_pos: usize,
    ) -> Result<(), EditError> {
        if from != to {
            // Materialize the target first, as its own preparation, so that both stories are clean before the move.
            self.edit(to, |_, _, _| Ok(()))?;
        }
        self.edit(from, |document, source, shift| {
            let range = shift(range.start)..shift(range.end);
            check_deletable(source, &range)?;
            if range.is_empty() {
                return Err(EditError::InvalidPosition {
                    pos: range.start,
                    len: source.len(),
                });
            }
            let target = if from == to {
                source.clone()
            } else {
                document.story(to)?
            };
            let same = from == to;
            let to_pos = if same { shift(to_pos) } else { to_pos };
            if same && to_pos > range.start && to_pos < range.end {
                return Err(EditError::InvalidPosition {
                    pos: to_pos,
                    len: source.len(),
                });
            }
            let runs = source.runs_in(range.clone())?;
            document.check_move_target(&runs, to)?;
            if would_break_blocks(source, &range) {
                return Err(EditError::WouldBreakBlockStructure);
            }
            source.delete(range.clone())?;
            let mut pos = if same && to_pos >= range.end {
                to_pos - range.len()
            } else {
                to_pos
            };
            check_inner_position(&target, pos)?;
            let start = pos;
            for run in &runs {
                let len = run.len();
                target.insert(pos, &run.text)?;
                // The pasted characters keep exactly their own marks: marks they inherited by expansion are removed.
                for inherited in target.runs_in(pos..pos + len)? {
                    for key in inherited.marks.keys() {
                        if !run.marks.contains_key(key) {
                            target.unmark(pos..pos + len, key)?;
                        }
                    }
                }
                for (key, value) in &run.marks {
                    target.mark(pos..pos + len, key, value)?;
                }
                pos += len;
            }
            document.note_structure(from, source, range.start);
            document.note_structure(to, &target, start);
            document.note_structure(to, &target, pos);
            Ok(())
        })
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

    // ----- Materialization -----

    /// Writes into the CRDT the structure that the view of `story` shows but the story does not store (N1 and N4 paragraph ends), so that later edits act on stored structure. Returns the positions, in the story before materialization, where paragraph ends were inserted. Runs as part of the next commit; operations call it themselves.
    ///
    /// # Errors
    ///
    /// The errors of the CRDT.
    pub fn materialize(&mut self, story: EntityId) -> Result<Vec<usize>, EditError> {
        let handle = self.story(story)?;
        let (_, report) = normalize(&self.raw());
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

    // ----- Internals -----

    /// Runs one operation on `story` as one transaction: materializes the story if it may need it, gives the operation a function that maps positions from before the materialization to after it, and commits if the operation succeeds.
    fn edit<T>(
        &mut self,
        story: EntityId,
        operation: impl FnOnce(&mut Self, &Story, &dyn Fn(usize) -> usize) -> Result<T, EditError>,
    ) -> Result<T, EditError> {
        self.check_attached()?;
        let handle = self.story(story)?;
        let mut inserted = Vec::new();
        if !self.clean.contains(&story) {
            if needs_materialization(&handle) {
                inserted = self.materialize(story)?;
                // Materialization does not change the view, so it is not an undo step.
                self.crdt.commit_without_undo();
            }
            self.clean.insert(story);
        }
        let shift = |pos: usize| pos + inserted.iter().filter(|at| **at < pos).count();
        let result = operation(self, &handle, &shift)?;
        self.crdt.commit();
        Ok(result)
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

    fn paragraph_map(&self, paragraph: EntityId) -> Result<PropertyMap, EditError> {
        Ok(self
            .crdt
            .registry(registry::PARAGRAPHS)
            .create(&paragraph.to_string())?)
    }

    fn paragraph_props(&self, paragraph: EntityId) -> Props {
        self.crdt
            .registry(registry::PARAGRAPHS)
            .get(&paragraph.to_string())
            .map(|map| map.entries())
            .unwrap_or_default()
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
        let map = self
            .crdt
            .registry(registry::CELLS)
            .create(&cell.to_string())?;
        map.set("w", &Value::Int(2_000))?;
        map.set(registry::STORY, &Value::Str(story.to_string()))?;
        Ok(cell)
    }

    fn new_row(&mut self, cells: usize) -> Result<EntityId, EditError> {
        let row = self.ids.next_id();
        let list = self
            .crdt
            .registry(registry::ROWS)
            .create(&row.to_string())?
            .id_list(registry::CELLS_KEY)?;
        for index in 0..cells {
            let cell = self.new_cell()?;
            list.insert(index, &cell.to_string())?;
        }
        Ok(row)
    }

    fn rows_list(&self, table: EntityId) -> Result<bayan_crdt::IdList, EditError> {
        self.crdt
            .registry(registry::TABLES)
            .get(&table.to_string())
            .and_then(|map| map.get_id_list(registry::ROWS_KEY))
            .ok_or(EditError::NoSuchTable(table))
    }

    fn cells_list(&self, row: EntityId) -> Result<bayan_crdt::IdList, EditError> {
        self.crdt
            .registry(registry::ROWS)
            .get(&row.to_string())
            .and_then(|map| map.get_id_list(registry::CELLS_KEY))
            .ok_or(EditError::NoSuchTable(row))
    }

    fn table_row_ids(&self, table: EntityId) -> Result<Vec<EntityId>, EditError> {
        Ok(self
            .rows_list(table)?
            .items()
            .iter()
            .filter_map(|item| item.as_str().and_then(EntityId::parse))
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

    /// Refuses moves that would put a table inside itself, or a comment reference into a comment.
    fn check_move_target(
        &self,
        runs: &[bayan_crdt::Run],
        target: EntityId,
    ) -> Result<(), EditError> {
        let bound = |kind: AtomKind| -> Vec<EntityId> {
            runs.iter()
                .filter(|run| run.text.contains(kind.placeholder()))
                .filter_map(|run| {
                    run.marks
                        .get(ATOM_KEY)
                        .and_then(Value::as_str)
                        .and_then(decode_binding)
                })
                .filter(|(bound, _)| *bound == kind)
                .map(|(_, id)| id)
                .collect()
        };
        let comments = bound(AtomKind::CommentReference);
        let tables = bound(AtomKind::TableBlock);
        if comments.is_empty() && tables.is_empty() {
            return Ok(());
        }
        let raw = self.raw();
        if !comments.is_empty()
            && raw.comments.values().any(|comment| {
                comment
                    .get(registry::STORY)
                    .and_then(Value::as_str)
                    .and_then(EntityId::parse)
                    == Some(target)
            })
        {
            return Err(EditError::WouldCreateCycle);
        }
        // Every story inside the moved tables, at any depth.
        let mut pending = tables;
        let mut inside: BTreeSet<EntityId> = BTreeSet::new();
        let mut visited: BTreeSet<EntityId> = BTreeSet::new();
        while let Some(table) = pending.pop() {
            if !visited.insert(table) {
                continue;
            }
            let Some(entry) = raw.tables.get(&table) else {
                continue;
            };
            for row in entry
                .rows
                .iter()
                .filter_map(|item| item.as_str().and_then(EntityId::parse))
            {
                let Some(row) = raw.rows.get(&row) else {
                    continue;
                };
                for cell in row
                    .cells
                    .iter()
                    .filter_map(|item| item.as_str().and_then(EntityId::parse))
                {
                    let Some(story) = raw.cells.get(&cell).and_then(|cell| {
                        cell.get(registry::STORY)
                            .and_then(Value::as_str)
                            .and_then(EntityId::parse)
                    }) else {
                        continue;
                    };
                    if inside.insert(story)
                        && let Some(runs) = raw.stories.get(&story)
                    {
                        for run in runs.iter().filter(|run| run.text.contains(TABLE_BLOCK)) {
                            if let Some((AtomKind::TableBlock, nested)) = run
                                .marks
                                .get(ATOM_KEY)
                                .and_then(Value::as_str)
                                .and_then(decode_binding)
                            {
                                pending.push(nested);
                            }
                        }
                    }
                }
            }
        }
        if inside.contains(&target) {
            return Err(EditError::WouldCreateCycle);
        }
        Ok(())
    }
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

/// A position where something may be inserted: anywhere before the final paragraph end.
fn check_inner_position(handle: &Story, pos: usize) -> Result<(), EditError> {
    let len = handle.len();
    if pos >= len {
        return Err(EditError::InvalidPosition { pos, len });
    }
    Ok(())
}

/// A range inside the story.
fn check_range(handle: &Story, range: &Range<usize>) -> Result<(), EditError> {
    let len = handle.len();
    if range.start > range.end || range.end > len {
        return Err(EditError::InvalidPosition {
            pos: range.end,
            len,
        });
    }
    Ok(())
}

/// A range that may be deleted: inside the story and before its final paragraph end.
fn check_deletable(handle: &Story, range: &Range<usize>) -> Result<(), EditError> {
    let len = handle.len();
    if range.start > range.end || range.end >= len {
        return Err(EditError::InvalidPosition {
            pos: range.end,
            len,
        });
    }
    Ok(())
}
