//! Mark keys and how they expand (document model §5).
//!
//! A mark key is `family:name`, and its family (the text before the first colon) decides how the mark expands when text is typed at its boundary. That is how the CRDT library resolves the expansion of dynamically named keys such as one per comment, so a family is configured once (see `bayan_crdt::MarkFamily`).

use bayan_crdt::{Expand, MarkFamily};

use crate::EntityId;

/// Run properties, one key per `w:rPr` property (`r:b`, `r:sz`, `r:rFonts.ascii`, `r:w14.ligatures`…), and unknown run properties (`r:preserved`). Typing at the end of a run continues its formatting.
pub const RUN: &str = "r";

/// Hyperlinks (`link`). Typing at a link's boundary does not extend it.
pub const LINK: &str = "link";

/// Comment highlights, one key per comment (`cmt:<CommentId>`), so comments overlap freely. They never expand.
pub const COMMENT: &str = "cmt";

/// Revisions (`rev:ins`, `rev:del`, `rev:moveFrom`, `rev:moveTo`, `rev:rPrChange`). They never expand; the editing layer marks new text explicitly when tracking is on.
pub const REVISION: &str = "rev";

/// The mark families of the document model and how each expands. Keys of any other family do not expand.
pub const FAMILIES: [MarkFamily; 4] = [
    MarkFamily {
        name: RUN,
        expand: Expand::After,
    },
    MarkFamily {
        name: LINK,
        expand: Expand::None,
    },
    MarkFamily {
        name: COMMENT,
        expand: Expand::None,
    },
    MarkFamily {
        name: REVISION,
        expand: Expand::None,
    },
];

/// Bold (`w:b`).
pub const BOLD: &str = "r:b";
/// Italic (`w:i`).
pub const ITALIC: &str = "r:i";
/// Font size in half-points (`w:sz`).
pub const SIZE: &str = "r:sz";
/// Text color (`w:color/@w:val`).
pub const COLOR: &str = "r:color";
/// A tracked insertion (`w:ins`).
pub const INSERTED: &str = "rev:ins";

/// The key of the highlight of comment `id`.
#[must_use]
pub fn comment_key(id: EntityId) -> String {
    format!("{COMMENT}:{id}")
}

/// The comment whose highlight `key` is, if it is one.
#[must_use]
pub fn comment_of_key(key: &str) -> Option<EntityId> {
    key.strip_prefix("cmt:").and_then(EntityId::parse)
}

/// Whether `key` is a run property (family [`RUN`]).
#[must_use]
pub fn is_run_property(key: &str) -> bool {
    key.starts_with("r:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_keys_round_trip() {
        let id = EntityId(0xabc);
        assert_eq!(comment_of_key(&comment_key(id)), Some(id));
        assert_eq!(comment_of_key("cmt:xyz"), None);
        assert_eq!(comment_of_key(BOLD), None);
        assert!(is_run_property(BOLD) && !is_run_property(LINK));
    }
}
