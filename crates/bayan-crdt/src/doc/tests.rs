//! Tests of the containment of a panic inside the CRDT library during [`Doc::import`] that depend on no bug of the library (follow-up FU-16 of the dependency session DEP-CORE-2026-11). They make the panic themselves, in two ways:
//!
//! - **Out of the library's own import.** [`inject_a_panic_into_an_import`] subscribes a callback of the test's own to the library's document, which panics when the library delivers the events of an import. The library delivers them inside its import (its documentation of `LoroDoc::subscribe_root` says so) and lets the panic out of its import, so the panic reaches [`Doc::import`] from inside the library's code, as a panic of the library's own would. What the panic leaves behind in the library differs between versions of Loro (1.16.2 leaves the lock of its transaction poisoned, 1.17.1 does not), so the test relies on none of it.
//! - **Inside the containment itself.** [`poison_through_the_containment`] hands [`Doc::contain`], the containment of [`Doc::import`], a closure of the test's own that panics. The library takes no part in that panic and stays usable, so the test can watch it while every method of the poisoned document and its handles runs: an edit waits in the library for the next commit and must still be waiting, the library's state and history must not change, and while the library shows an older version, the document must answer that it does not and must not bring it back. Every place that a handle reads holds something, so a method that asked the library instead of answering as for an empty document would answer differently. A handle or undo manager that the poisoned document hands out must not be attached to the library's document at all, which the test asks it, and dropping the undo manager obtained before must leak the library's undo manager instead of freeing it.
//!
//! The code that ships never subscribes such a callback and hands [`Doc::contain`] only the real import. Besides these, the tests use only the accessors `is_attached` of [`Story`](crate::Story), [`Stories`](crate::Stories) and [`UndoManager`], and [`UndoManager::hold_while_alive`], which exist in test builds only: no other condition, no feature and no hook.
//!
//! The panic hook that these tests install stays installed for the rest of the unit-test program, which bayan-crdt's other unit tests share (today those of `value.rs`): once either test has started, a failing assertion elsewhere reports only its location, and no other unit test may install a hook of its own, which would replace this one.
//!
//! Since Loro 1.17.1 these tests carry the guarantee alone: the fixture tests in `tests/import_panic.rs`, which checked the same with a real panic of Loro 1.16.2 on CORE-004's crafted update, needed that version's bug and were deleted when Loro moved (DEP-CORE-2026-11-LORO). The crafted update stays a seed of the fuzz target `crdt_import`.
//!
//! Host only (see the declaration in `doc.rs`). In a file of its own, declared with `#[cfg(test)] mod tests;`, as AGENTS.md asks of tests: the compiler does not read it for the code that ships, every line of which the gate's outside-world check reads.

use std::cell::Cell;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Once};

use loro::event::DiffEvent;
use loro::{EventTriggerKind, Frontiers};

use super::{Doc, DocStats, MAIN_STORY, Version};
use crate::{
    CrdtError, Expand, ImportError, ImportLimits, MarkFamily, PeerId, UndoManager, Value,
    VersionVector,
};

const FAMILIES: [MarkFamily; 1] = [MarkFamily {
    name: "r",
    expand: Expand::After,
}];

thread_local! {
    /// How many panics the current thread had while [`count_panics`] runs on it, and `None` otherwise.
    static COUNTED: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Installs, once, a panic hook that never prints or records a panic's message, whatever code raised it, in the pattern a host must follow before it imports untrusted blobs (see [`Doc::import`]). It counts the panics of a thread while [`count_panics`] runs on it, which the tests provoke, and reports any other panic, such as a failing assertion, by its location only.
fn withhold_panic_messages() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        panic::set_hook(Box::new(|info| {
            let counted = COUNTED
                .try_with(|count| count.get().map(|seen| count.set(Some(seen + 1))).is_some())
                .unwrap_or(false);
            if counted {
                return;
            }
            match info.location() {
                Some(location) => eprintln!(
                    "panicked at {}:{} (the message is withheld)",
                    location.file(),
                    location.line()
                ),
                None => eprintln!("panicked (the message is withheld)"),
            }
        }));
    });
}

/// Runs `work`, which must not let a panic escape, and returns its result with the number of panics on this thread meanwhile, caught ones included.
fn count_panics<R>(work: impl FnOnce() -> R) -> (R, usize) {
    withhold_panic_messages();
    COUNTED.with(|count| count.set(Some(0)));
    let result = work();
    (result, COUNTED.with(Cell::take).unwrap_or(0))
}

/// A valid update from another replica.
fn remote_update() -> Vec<u8> {
    let other = Doc::new(PeerId(2), &FAMILIES).unwrap();
    other.main_story().insert(0, "remote").unwrap();
    other.commit();
    other.export_updates(&VersionVector::new()).unwrap()
}

/// Imports `update` into `doc` while a callback of the test's own, subscribed to the library's document, panics as soon as the library delivers the import's events, and checks that [`Doc::import`] contained exactly that panic and poisoned the document.
fn inject_a_panic_into_an_import(doc: &Doc, update: &[u8]) {
    let subscription = doc.loro.subscribe_root(Arc::new(|event: DiffEvent<'_>| {
        if event.triggered_by == EventTriggerKind::Import {
            panic!("a panic injected into the CRDT library's import, for a test");
        }
    }));
    let (outcome, panics) = count_panics(|| {
        panic::catch_unwind(AssertUnwindSafe(|| {
            doc.import(update, &ImportLimits::UPDATE)
        }))
    });
    if !doc.is_poisoned() {
        // The containment failed. Leak the library's document, so that the test fails at the assertions below instead of aborting: with Loro 1.16.2, freeing it while the failure unwinds would panic again, on the lock that the injected panic poisoned.
        std::mem::forget(doc.loro.clone());
    }
    // `Some`: the panic did not escape `Doc::import`. If the import returns normally instead after an upgrade of Loro, the library no longer lets a subscriber's panic out of its import: inject the panic through another callback that the library runs inside its import, and never with a blob that makes the library panic, which the project neither searches for nor commits.
    assert_eq!(outcome.ok(), Some(Err(ImportError::Panicked)));
    // At least the injected panic: how many times the hook fires while the library hands that panic on is up to the library, which may raise it again as a new panic.
    assert!(panics >= 1);
    assert!(doc.is_poisoned());
    // The callback stays with the library's document, and panics again only if something imports into it.
    subscription.detach();
}

/// Poisons `doc` through [`Doc::contain`], the containment of [`Doc::import`], with a closure of the test's own that panics inside it, and checks that the containment caught exactly that panic. The library takes no part in the panic, so it stays usable.
fn poison_through_the_containment(doc: &Doc) {
    let (outcome, panics) = count_panics(|| {
        panic::catch_unwind(AssertUnwindSafe(|| {
            doc.contain(|| panic!("a panic injected into the containment of an import, for a test"))
        }))
    });
    // `Some`: the panic did not escape `Doc::contain`.
    assert_eq!(outcome.ok(), Some(Err(ImportError::Panicked)));
    assert_eq!(panics, 1);
    assert!(doc.is_poisoned());
}

/// A panic that comes out of the library's own import is contained by [`Doc::import`], and the document is poisoned: it refuses to import and to export, and dropping it returns normally, because it is leaked instead of freed (see `Doc`'s `Drop`), whatever the panic left behind in the library.
#[test]
fn a_panic_inside_the_library_during_an_import_poisons_the_document_instead_of_crashing() {
    withhold_panic_messages();
    let update = remote_update();
    let doc = Doc::new(PeerId(1), &FAMILIES).unwrap();
    // A callback that lives exactly as long as the library's document, to show whether dropping the `Doc` freed it: nothing else holds the library's document here. It counts the library's local updates, of which there are none.
    let updates = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&updates);
    doc.loro
        .subscribe_local_update(Box::new(move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            true
        }))
        .detach();

    inject_a_panic_into_an_import(&doc, &update);
    assert_eq!(
        doc.import(&update, &ImportLimits::UPDATE),
        Err(ImportError::Poisoned)
    );
    assert_eq!(doc.export_snapshot(), Err(CrdtError::Poisoned));
    drop(doc);
    // The library's document, and with it the callback, is still there: leaked, not freed. (Freeing it would also have panicked with Loro 1.16.2, on the lock that the injected panic poisoned.)
    assert_eq!(
        (Arc::strong_count(&updates), updates.load(Ordering::Relaxed)),
        (2, 0)
    );
}

/// Every method of a poisoned document and of its handles, those obtained before the panic and those obtained after, refuses or answers as for an empty document, without calling the library (review of CORE-004, item 3).
#[test]
fn a_poisoned_document_and_its_handles_refuse_everything_without_calling_the_library() {
    withhold_panic_messages();
    let update = remote_update();
    let doc = Doc::new(PeerId(1), &FAMILIES).unwrap();
    let mut undo = UndoManager::new(&doc);
    // A token that a callback of the library's undo manager holds for as long as that manager lives.
    let token = Arc::new(());
    undo.hold_while_alive(Arc::clone(&token));
    // Something in every place a handle reads, written in three undo steps.
    let main = doc.main_story();
    main.insert(0, "abc").unwrap();
    main.mark(0..2, "r:b", &Value::Bool(true)).unwrap();
    doc.commit();
    let stories = doc.stories();
    let side = stories.create("s").unwrap();
    side.insert(0, "x").unwrap();
    let registry = doc.registry("paragraphs").unwrap();
    let table = registry.create("t").unwrap();
    table.set("k", &Value::Int(1)).unwrap();
    let rows = table.id_list("rows").unwrap();
    rows.insert(0, "r1").unwrap();
    rows.insert(1, "r2").unwrap();
    // An entry that `Registry::create` did not make, as a misbehaving replica can write one.
    doc.loro.get_map("paragraphs").insert("v", 1).unwrap();
    let body = doc.root_map("body").unwrap();
    body.set("w", &Value::Int(2)).unwrap();
    let cols = body.id_list("cols").unwrap();
    cols.insert(0, "c1").unwrap();
    doc.commit();
    main.insert(3, "d").unwrap();
    doc.commit();
    // A step to redo, besides the two left to undo.
    assert!(undo.undo().unwrap());
    let earlier = doc.version();

    // What the library answers before the panic, which no method may answer after it.
    assert!(main.is_attached() && stories.is_attached() && undo.is_attached());
    assert_eq!(
        (main.len(), main.text(), main.char_at(0), main.runs().len()),
        (3, "abc".to_owned(), Some('a'), 2)
    );
    assert_eq!(side.text(), "x");
    assert_eq!(stories.ids(), ["s"]);
    assert_eq!(stories.entries().len(), 1);
    assert!(stories.get("s").is_some());
    assert_eq!(registry.ids(), ["t", "v"]);
    assert_eq!(registry.unmergeable_ids(), ["v"]);
    assert_eq!((registry.entries().len(), registry.len()), (1, 2));
    assert!(registry.get("t").is_some());
    assert_eq!(
        (table.get("k"), body.get("w")),
        (Some(Value::Int(1)), Some(Value::Int(2)))
    );
    assert_eq!((table.entries().len(), body.entries().len()), (1, 1));
    assert!(table.get_id_list("rows").is_some() && body.get_id_list("cols").is_some());
    assert_eq!((rows.items().len(), cols.items().len()), (2, 1));
    assert_eq!(
        (undo.can_undo(), undo.can_redo(), undo.undo_count()),
        (true, true, 2)
    );
    assert_ne!(earlier, Version(Frontiers::default()));
    assert_ne!(doc.version_vector(), VersionVector::new());
    assert_ne!(doc.stats(), DocStats { ops: 0, changes: 0 });
    // And the token shows a free: dropping the undo manager of a document that is not poisoned frees the library's manager, and the callback with it.
    let control = Arc::new(());
    let mut healthy = UndoManager::new(&doc);
    healthy.hold_while_alive(Arc::clone(&control));
    drop(healthy);
    assert_eq!(Arc::strong_count(&control), 1);

    poison_through_the_containment(&doc);
    assert_eq!(doc.peer(), PeerId(1));
    // An edit that waits in the library for the next commit, written past the poisoned document, and what the library holds: whatever a method of the poisoned document did in the library would change it.
    doc.loro.get_text(MAIN_STORY).insert(0, "p").unwrap();
    let library = || {
        (
            doc.loro.get_pending_txn_len(),
            doc.loro.oplog_vv(),
            doc.loro.get_deep_value(),
            doc.loro.is_detached(),
        )
    };
    let unchanged = library();
    assert_eq!((unchanged.0, unchanged.3), (1, false));

    // Handles obtained after the panic, and a new undo manager: none is attached to the library's document, so obtaining them asked the library nothing.
    let main_after = doc.main_story();
    let stories_after = doc.stories();
    let mut undo_after = UndoManager::new(&doc);
    assert!(!main_after.is_attached());
    assert!(!stories_after.is_attached());
    assert!(!undo_after.is_attached());

    // The document.
    doc.commit();
    doc.commit_without_undo();
    assert!(!doc.has_pending_changes());
    assert_eq!(doc.version(), Version(Frontiers::default()));
    assert_eq!(doc.version_vector(), VersionVector::new());
    assert_eq!(doc.stats(), DocStats { ops: 0, changes: 0 });
    assert_eq!(doc.checkout(&earlier), Err(CrdtError::Poisoned));
    doc.checkout_latest();
    assert!(!doc.is_detached());
    assert_eq!(doc.registry("paragraphs").err(), Some(CrdtError::Poisoned));
    assert_eq!(doc.root_map("body").err(), Some(CrdtError::Poisoned));
    assert_eq!(doc.export_snapshot(), Err(CrdtError::Poisoned));
    assert_eq!(
        doc.export_shallow_snapshot(&earlier),
        Err(CrdtError::Poisoned)
    );
    assert_eq!(
        doc.export_updates(&VersionVector::new()),
        Err(CrdtError::Poisoned)
    );
    assert_eq!(
        doc.import(&update, &ImportLimits::UPDATE),
        Err(ImportError::Poisoned)
    );

    // Stories, before and after.
    for story in [main, side, main_after] {
        assert_eq!(story.len(), 0);
        assert!(story.is_empty());
        assert_eq!(story.text(), "");
        assert_eq!(story.char_at(0), None);
        assert!(story.runs().is_empty());
        assert_eq!(story.runs_in(0..0), Err(CrdtError::Poisoned));
        assert_eq!(story.insert(0, "x"), Err(CrdtError::Poisoned));
        assert_eq!(
            story.insert_atom(0, '\u{0D}', "p:0"),
            Err(CrdtError::Poisoned)
        );
        assert_eq!(story.delete(0..0), Err(CrdtError::Poisoned));
        assert_eq!(
            story.mark(0..0, "r:b", &Value::Bool(true)),
            Err(CrdtError::Poisoned)
        );
        assert_eq!(story.unmark(0..0, "r:b"), Err(CrdtError::Poisoned));
    }
    for stories in [stories, stories_after] {
        assert!(stories.ids().is_empty());
        assert!(stories.entries().is_empty());
        assert!(stories.get("s").is_none());
        assert_eq!(stories.create("u").err(), Some(CrdtError::Poisoned));
    }
    // The registry, maps and lists, all obtained before (after the panic, the document hands out none).
    assert!(registry.ids().is_empty());
    assert!(registry.unmergeable_ids().is_empty());
    assert!(registry.entries().is_empty());
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());
    assert!(registry.get("t").is_none());
    assert_eq!(registry.create("u").err(), Some(CrdtError::Poisoned));
    for (map, key, list) in [(&table, "k", "rows"), (&body, "w", "cols")] {
        assert_eq!(map.get(key), None);
        assert!(map.entries().is_empty());
        assert!(map.get_id_list(list).is_none());
        assert_eq!(map.set(key, &Value::Int(3)), Err(CrdtError::Poisoned));
        assert_eq!(map.remove(key), Err(CrdtError::Poisoned));
        assert_eq!(map.id_list(list).err(), Some(CrdtError::Poisoned));
    }
    for list in [rows, cols] {
        assert_eq!(list.len(), 0);
        assert!(list.is_empty());
        assert!(list.items().is_empty());
        assert_eq!(list.insert(0, "r"), Err(CrdtError::Poisoned));
        assert_eq!(list.delete(0), Err(CrdtError::Poisoned));
        assert_eq!(list.move_item(0, 1), Err(CrdtError::Poisoned));
    }
    // The undo manager from before, and the one created after.
    for manager in [&mut undo, &mut undo_after] {
        assert_eq!(manager.undo(), Err(CrdtError::Poisoned));
        assert_eq!(manager.redo(), Err(CrdtError::Poisoned));
        assert!(!manager.can_undo());
        assert!(!manager.can_redo());
        assert_eq!(manager.undo_count(), 0);
    }

    // None of it changed anything in the library: the edit still waits for the next commit.
    assert_eq!(library(), unchanged);

    // While the library shows an older version, the document answers that it does not, and does not bring the library back to the latest one.
    doc.loro.commit();
    doc.loro.checkout(&earlier.0).unwrap();
    assert!(!doc.is_detached());
    doc.checkout_latest();
    assert!(doc.loro.is_detached());

    // Dropping the undo manager from before leaks the library's manager instead of freeing it (see `UndoManager`'s `Drop`): its callback still holds the token.
    drop(undo);
    assert_eq!(Arc::strong_count(&token), 2);
}
