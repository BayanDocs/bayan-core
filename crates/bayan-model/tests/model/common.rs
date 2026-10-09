//! Helpers shared by the integration tests.

use bayan_crdt::ImportLimits;
use bayan_model::{Document, EntityId, Item, Props, View, check_invariants};

/// The seed of every test document's identifier generator.
pub const SEED: u64 = 7;

/// A new document for replica `peer`.
pub fn document(peer: u64) -> Document {
    Document::new(peer, SEED).expect("a new document")
}

/// A replica for `peer` that has received everything `source` has.
pub fn replica_of(source: &Document, peer: u64) -> Document {
    let mut replica = Document::replica(peer, SEED).expect("an empty replica");
    sync(source, &mut replica);
    replica
}

/// Sends everything `from` has that `to` lacks.
pub fn sync(from: &Document, to: &mut Document) {
    let update = from
        .export_updates(&to.version_vector())
        .expect("exporting an update");
    to.import(&update, &ImportLimits::UPDATE)
        .expect("importing an update");
}

/// Exchanges updates in both directions.
pub fn sync_both(a: &mut Document, b: &mut Document) {
    sync(a, b);
    sync(b, a);
}

/// The plain text of the main story.
pub fn text(document: &Document) -> String {
    document.view().plain_text(EntityId::MAIN_STORY)
}

/// The view, after checking that it satisfies every invariant.
pub fn valid_view(document: &Document) -> View {
    let view = document.view();
    let violations = check_invariants(&view);
    assert!(violations.is_empty(), "invariants broken: {violations:?}");
    view
}

/// The marks at position `pos` of a story of the view (positions count one per character and one per atom).
pub fn marks_at(view: &View, story: EntityId, pos: usize) -> Props {
    let mut offset = 0;
    for item in &view.stories[&story] {
        let len = item.len();
        if pos < offset + len {
            return match item {
                Item::Text { marks, .. } | Item::Atom { marks, .. } => marks.clone(),
            };
        }
        offset += len;
    }
    panic!("position {pos} is outside story {story}");
}
