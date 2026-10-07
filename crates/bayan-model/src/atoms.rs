//! Special atoms and their placeholder characters (document model §4).

use crate::EntityId;

/// The kinds of special atom this prototype models: the subset the CORE-004 brief names, plus the comment reference that anchors a comment (document model §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AtomKind {
    /// The end of a paragraph; its entity holds the paragraph's properties.
    ParagraphEnd,
    /// A tab character (no entity).
    Tab,
    /// The start of a field; the field code follows.
    FieldBegin,
    /// The end of a field's code and the start of its result.
    FieldSeparator,
    /// The end of a field.
    FieldEnd,
    /// An inline or anchored object (picture, shape, chart…).
    ObjectAnchor,
    /// The start of a range (bookmark, permission, content control…).
    RangeStart,
    /// The end of a range.
    RangeEnd,
    /// A table, which is a block-level atom.
    TableBlock,
    /// Where a comment is anchored; the comment's text is a story of its own.
    CommentReference,
}

impl AtomKind {
    /// Every kind.
    pub const ALL: [Self; 10] = [
        Self::ParagraphEnd,
        Self::Tab,
        Self::FieldBegin,
        Self::FieldSeparator,
        Self::FieldEnd,
        Self::ObjectAnchor,
        Self::RangeStart,
        Self::RangeEnd,
        Self::TableBlock,
        Self::CommentReference,
    ];

    /// The placeholder code point (document model §4): a C0 control character, which the importer and every input path keep out of document text. Where Word's binary format has a convention, the placeholder mirrors it.
    #[must_use]
    pub const fn placeholder(self) -> char {
        match self {
            Self::ParagraphEnd => '\u{0D}',
            Self::Tab => '\u{09}',
            Self::FieldBegin => '\u{13}',
            Self::FieldSeparator => '\u{14}',
            Self::FieldEnd => '\u{15}',
            Self::ObjectAnchor => '\u{08}',
            Self::RangeStart => '\u{11}',
            Self::RangeEnd => '\u{12}',
            Self::TableBlock => '\u{07}',
            Self::CommentReference => '\u{05}',
        }
    }

    /// The kind whose placeholder is `character`, if any.
    #[must_use]
    pub fn from_placeholder(character: char) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.placeholder() == character)
    }

    /// The short code that names the kind in an atom binding.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ParagraphEnd => "p",
            Self::Tab => "tab",
            Self::FieldBegin => "fb",
            Self::FieldSeparator => "fs",
            Self::FieldEnd => "fe",
            Self::ObjectAnchor => "obj",
            Self::RangeStart => "rs",
            Self::RangeEnd => "re",
            Self::TableBlock => "tbl",
            Self::CommentReference => "cref",
        }
    }

    /// Whether atoms of this kind reference an entity (all except the tab).
    #[must_use]
    pub const fn has_entity(self) -> bool {
        !matches!(self, Self::Tab)
    }

    /// Whether the atom is block-level: it may stand only at the start of a story or right after a paragraph end or another block-level atom (document model §4, invariant I4).
    #[must_use]
    pub const fn is_block(self) -> bool {
        matches!(self, Self::TableBlock)
    }

    /// Whether the view keeps the marks of atoms of this kind. Paragraph ends carry no marks in the view: all formatting of the paragraph mark, including its revisions, lives in the paragraph's properties, as `w:pPr/w:rPr` does in OOXML. Range delimiters and table blocks have no glyph to format.
    #[must_use]
    pub const fn keeps_marks(self) -> bool {
        !matches!(
            self,
            Self::ParagraphEnd | Self::RangeStart | Self::RangeEnd | Self::TableBlock
        )
    }
}

/// Whether `character` may appear as text: every C0 control character is reserved for placeholders (document model §4) and is removed from user input and from the view.
#[must_use]
pub const fn is_text_character(character: char) -> bool {
    !matches!(character, '\u{00}'..='\u{1F}')
}

/// The value of the `atom` mark that binds a placeholder to its entity: the kind's code and the entity identifier, such as `p:0123…`.
#[must_use]
pub fn encode_binding(kind: AtomKind, id: EntityId) -> String {
    format!("{}:{id}", kind.code())
}

/// Reads a binding written by [`encode_binding`]; `None` for anything else.
#[must_use]
pub fn decode_binding(binding: &str) -> Option<(AtomKind, EntityId)> {
    let (code, id) = binding.split_once(':')?;
    let kind = AtomKind::ALL.into_iter().find(|kind| kind.code() == code)?;
    Some((kind, EntityId::parse(id)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_distinct_c0_controls() {
        let mut seen = Vec::new();
        for kind in AtomKind::ALL {
            let placeholder = kind.placeholder();
            assert!(!is_text_character(placeholder));
            assert_eq!(AtomKind::from_placeholder(placeholder), Some(kind));
            assert!(!seen.contains(&placeholder));
            seen.push(placeholder);
        }
        assert_eq!(AtomKind::from_placeholder('a'), None);
        assert!(is_text_character(' '));
        assert!(is_text_character('\u{7F}'));
        assert!(is_text_character('\u{FFFC}'));
    }

    #[test]
    fn bindings_round_trip_and_reject_garbage() {
        let id = EntityId(7);
        for kind in AtomKind::ALL {
            let binding = encode_binding(kind, id);
            assert_eq!(decode_binding(&binding), Some((kind, id)));
        }
        assert_eq!(decode_binding("p"), None);
        assert_eq!(decode_binding("zz:00000000000000000000000000000007"), None);
        assert_eq!(decode_binding("p:7"), None);
    }
}
