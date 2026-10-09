//! The raw state of a document as stored in the CRDT, before normalization.

use std::collections::{BTreeMap, BTreeSet};

use bayan_crdt::{Doc, Registry, Run, Value};

use crate::EntityId;

/// Properties of an entity: property name to value, in name order.
pub type Props = BTreeMap<String, Value>;

/// Registry names, one root map per entity kind (document model §16).
pub mod registry {
    /// Paragraph properties, by paragraph identifier.
    pub const PARAGRAPHS: &str = "paragraphs";
    /// Tables: properties and the movable list of row identifiers (key [`ROWS`]).
    pub const TABLES: &str = "tables";
    /// Rows: properties and the movable list of cell identifiers (key [`CELLS`]).
    pub const ROWS: &str = "rows";
    /// Cells: properties and the identifier of the cell's story (key [`STORY`]).
    pub const CELLS: &str = "cells";
    /// Objects anchored by object-anchor atoms.
    pub const OBJECTS: &str = "objects";
    /// Fields delimited by field atoms.
    pub const FIELDS: &str = "fields";
    /// Comments: properties and the identifier of the comment's story (key [`STORY`]).
    pub const COMMENTS: &str = "comments";
    /// Ranges (bookmarks and the like) delimited by range atoms.
    pub const RANGES: &str = "ranges";
    /// The root map of document-wide properties; the final section's properties are its keys (document model §6).
    pub const BODY: &str = "body";
    /// The key of a table's row list.
    pub const ROWS_KEY: &str = "rows";
    /// The key of a row's cell list.
    pub const CELLS_KEY: &str = "cells";
    /// The key of the story identifier in a cell or comment.
    pub const STORY: &str = "story";
}

/// A table as stored: its properties and its row list as stored (items are normally row identifiers).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawTable {
    /// The table's properties.
    pub props: Props,
    /// The row list.
    pub rows: Vec<Value>,
}

/// A row as stored: its properties and its cell list as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawRow {
    /// The row's properties.
    pub props: Props,
    /// The cell list.
    pub cells: Vec<Value>,
}

/// Everything the normalization reads: every story as runs, and every entity registry, exactly as stored (which concurrent edits or a misbehaving replica may have left inconsistent). Entries whose key is not an identifier are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawDocument {
    /// The main story.
    pub main: Vec<Run>,
    /// Every other story.
    pub stories: BTreeMap<EntityId, Vec<Run>>,
    /// Paragraph properties.
    pub paragraphs: BTreeMap<EntityId, Props>,
    /// Identifiers whose entry in the paragraph registry is not a property map (a value or another kind of container that a misbehaving replica stored there): no paragraph, but taken, so that normalization never derives one of them for a paragraph end that materialization would then have to write there.
    pub other_paragraph_entries: BTreeSet<EntityId>,
    /// Tables.
    pub tables: BTreeMap<EntityId, RawTable>,
    /// Rows.
    pub rows: BTreeMap<EntityId, RawRow>,
    /// Cells (their properties include the story identifier).
    pub cells: BTreeMap<EntityId, Props>,
    /// Objects.
    pub objects: BTreeMap<EntityId, Props>,
    /// Fields.
    pub fields: BTreeMap<EntityId, Props>,
    /// Comments (their properties include the story identifier).
    pub comments: BTreeMap<EntityId, Props>,
    /// Ranges.
    pub ranges: BTreeMap<EntityId, Props>,
    /// Document-wide properties, including the final section's.
    pub body: Props,
}

impl RawDocument {
    /// Reads the raw state of `doc`.
    #[must_use]
    pub fn read(doc: &Doc) -> Self {
        let stories = doc
            .stories()
            .entries()
            .into_iter()
            .filter_map(|(key, story)| Some((EntityId::parse(&key)?, story.runs())))
            .collect();
        let tables = read_entities(doc, registry::TABLES, |map| RawTable {
            props: map.entries(),
            rows: map
                .get_id_list(registry::ROWS_KEY)
                .map(|list| list.items())
                .unwrap_or_default(),
        });
        let rows = read_entities(doc, registry::ROWS, |map| RawRow {
            props: map.entries(),
            cells: map
                .get_id_list(registry::CELLS_KEY)
                .map(|list| list.items())
                .unwrap_or_default(),
        });
        let paragraphs = read_props(doc, registry::PARAGRAPHS);
        let other_paragraph_entries = doc
            .registry(registry::PARAGRAPHS)
            .map(|registry: Registry| registry.ids())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|key| EntityId::parse(&key))
            .filter(|id| !paragraphs.contains_key(id))
            .collect();
        Self {
            main: doc.main_story().runs(),
            stories,
            paragraphs,
            other_paragraph_entries,
            tables,
            rows,
            cells: read_props(doc, registry::CELLS),
            objects: read_props(doc, registry::OBJECTS),
            fields: read_props(doc, registry::FIELDS),
            comments: read_props(doc, registry::COMMENTS),
            ranges: read_props(doc, registry::RANGES),
            body: doc
                .root_map(registry::BODY)
                .map(|body| body.entries())
                .unwrap_or_default(),
        }
    }

    /// The runs of story `id` (the main story for [`EntityId::MAIN_STORY`]).
    #[must_use]
    pub fn story(&self, id: EntityId) -> Option<&[Run]> {
        if id == EntityId::MAIN_STORY {
            Some(&self.main)
        } else {
            self.stories.get(&id).map(Vec::as_slice)
        }
    }

    /// The registry of entities that atoms of `kind` reference, if any.
    #[must_use]
    pub fn has_entity(&self, kind: crate::AtomKind, id: EntityId) -> bool {
        use crate::AtomKind;
        match kind {
            AtomKind::ParagraphEnd => self.paragraphs.contains_key(&id),
            AtomKind::Tab => false,
            AtomKind::FieldBegin | AtomKind::FieldSeparator | AtomKind::FieldEnd => {
                self.fields.contains_key(&id)
            }
            AtomKind::ObjectAnchor => self.objects.contains_key(&id),
            AtomKind::RangeStart | AtomKind::RangeEnd => self.ranges.contains_key(&id),
            AtomKind::TableBlock => self.tables.contains_key(&id),
            AtomKind::CommentReference => self.comments.contains_key(&id),
        }
    }
}

fn read_props(doc: &Doc, name: &str) -> BTreeMap<EntityId, Props> {
    read_entities(doc, name, bayan_crdt::PropertyMap::entries)
}

/// Every entity of the registry `name`, read with `read`; none when the document refuses the registry (it is poisoned; the names of [`registry`] are all valid).
fn read_entities<T>(
    doc: &Doc,
    name: &str,
    read: impl Fn(&bayan_crdt::PropertyMap) -> T,
) -> BTreeMap<EntityId, T> {
    doc.registry(name)
        .map(|registry: Registry| registry.entries())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, map)| Some((EntityId::parse(&key)?, read(&map))))
        .collect()
}

#[cfg(test)]
mod tests {
    use bayan_crdt::{Doc, PeerId};

    use super::registry;

    /// The adapter refuses root names that other replicas could not decode; the model's names are all accepted.
    #[test]
    fn every_registry_name_is_a_valid_root_name() {
        let doc = Doc::new(PeerId(1), &crate::marks::FAMILIES).expect("a document");
        for name in [
            registry::PARAGRAPHS,
            registry::TABLES,
            registry::ROWS,
            registry::CELLS,
            registry::OBJECTS,
            registry::FIELDS,
            registry::COMMENTS,
            registry::RANGES,
        ] {
            assert!(doc.registry(name).is_ok(), "{name}");
        }
        assert!(doc.root_map(registry::BODY).is_ok());
    }
}
