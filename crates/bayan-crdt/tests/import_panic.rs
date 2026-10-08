//! A panic of the CRDT library during an import is contained and poisons the document, and the poisoned document and every handle to it refuse to work instead of panicking again.
//!
//! A test program of its own, because it installs a panic hook for the whole process: Loro's panic messages can quote document text, so the hook prints nothing for a panic raised inside Loro and hands every other panic (such as a failing assertion of these tests) to the default hook. That is what a host must do before importing untrusted blobs (see `Doc::import`).
//!
//! Host only: in WebAssembly (wasm32-wasip1, where the gate also runs the tests) a panic aborts the whole test program instead of unwinding, so the adapter cannot contain it there; the engine's host restarts an engine that aborts (CORE-004 report).
#![cfg(not(target_arch = "wasm32"))]

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use bayan_crdt::{
    CrdtError, Doc, Expand, ImportError, ImportLimits, MarkFamily, PeerId, UndoManager, Value,
    VersionVector,
};

const FAMILIES: [MarkFamily; 1] = [MarkFamily {
    name: "r",
    expand: Expand::After,
}];

/// Found by the CORE-004 fuzzer and minimized with `crdt-model minimize`: a valid update for the document in the base snapshot, and the same update with one byte changed (and its checksum recomputed, which anyone can do: it is not a signature). Loro 1.16.2 panics while importing the crafted one.
const PANIC_BASE: &[u8] = include_bytes!("fixtures/loro-panic-base.bin");
const PANIC_UPDATE: &[u8] = include_bytes!("fixtures/loro-panic-update.bin");
const PANIC_CRAFTED: &[u8] = include_bytes!("fixtures/loro-panic-crafted.bin");

/// Panics that the hook kept quiet: raised inside the CRDT library.
static SILENCED: AtomicUsize = AtomicUsize::new(0);

/// Installs, once, a hook that prints nothing for a panic raised in the CRDT library's code and keeps the default behaviour for every other panic.
fn silence_library_panics() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let in_library = info
                .location()
                .is_some_and(|location| is_library_file(location.file()));
            if in_library {
                SILENCED.fetch_add(1, Ordering::Relaxed);
            } else {
                default(info);
            }
        }));
    });
}

/// Whether a panic's source file belongs to one of Loro's crates, such as `…/index.crates.io-…/loro-internal-1.16.2/src/oplog.rs`; on Windows the separators are backslashes.
fn is_library_file(file: &str) -> bool {
    file.split(['/', '\\']).any(|part| part.starts_with("loro"))
}

#[test]
fn recognizes_the_library_s_files_with_either_separator() {
    assert!(is_library_file(
        "/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/loro-internal-1.16.2/src/container/richtext/richtext_state.rs"
    ));
    assert!(is_library_file(
        r"C:\Users\runneradmin\.cargo\registry\src\index.crates.io-1949cf8c6b5b557f\loro-internal-1.16.2\src\container\richtext\richtext_state.rs"
    ));
    assert!(!is_library_file("crates/bayan-crdt/tests/import_panic.rs"));
    assert!(!is_library_file(r"crates\bayan-crdt\tests\import_panic.rs"));
}

fn load() -> Doc {
    Doc::load(
        PANIC_BASE,
        PeerId(9),
        &FAMILIES,
        &ImportLimits::LOCAL_SNAPSHOT,
    )
    .expect("the base document")
}

#[test]
fn a_panic_during_an_import_poisons_the_document_instead_of_crashing() {
    silence_library_panics();
    let valid = load();
    valid
        .import(PANIC_UPDATE, &ImportLimits::UPDATE)
        .expect("the valid update");
    let changed: Vec<usize> = (0..PANIC_UPDATE.len())
        .filter(|at| !(16..20).contains(at) && PANIC_UPDATE.get(*at) != PANIC_CRAFTED.get(*at))
        .collect();
    assert_eq!(
        (PANIC_CRAFTED.len(), changed),
        (PANIC_UPDATE.len(), vec![74])
    );

    let doc = load();
    let undo = UndoManager::new(&doc);
    let silenced = SILENCED.load(Ordering::Relaxed);
    // When Loro stops panicking on this blob, replace the fixtures with a case that still panics (the fuzzer finds them), so that this test keeps exercising the containment.
    assert_eq!(
        doc.import(PANIC_CRAFTED, &ImportLimits::UPDATE),
        Err(ImportError::Panicked)
    );
    // The panic happened in the library, and the hook kept its message out of the output.
    assert!(SILENCED.load(Ordering::Relaxed) > silenced);
    assert!(doc.is_poisoned());
    assert_eq!(
        doc.import(PANIC_UPDATE, &ImportLimits::UPDATE),
        Err(ImportError::Poisoned)
    );
    assert_eq!(doc.export_snapshot(), Err(CrdtError::Poisoned));
    // Freeing the document or its undo manager after such a panic can panic again, which during unwinding aborts the process; both are leaked instead, so dropping them returns normally.
    drop(undo);
    drop(doc);
}

/// Every method of a poisoned document and of its handles, those obtained before the panic and those obtained after, refuses or answers as for an empty document, without calling the library, whose locks the panic poisoned (review of CORE-004, item 3). Before, all but the import and the exports panicked here.
#[test]
fn a_poisoned_document_and_its_handles_refuse_everything() {
    silence_library_panics();
    let doc = load();
    // Handles obtained before the panic.
    let story_before = doc.main_story();
    let stories_before = doc.stories();
    let registry_before = doc.registry("paragraphs").expect("a registry");
    let map_before = doc.root_map("body").expect("a root map");
    let list_before = registry_before
        .create("t")
        .and_then(|table| table.id_list("rows"))
        .expect("a list");
    doc.commit();
    let mut undo = UndoManager::new(&doc);
    assert_eq!(
        doc.import(PANIC_CRAFTED, &ImportLimits::UPDATE),
        Err(ImportError::Panicked)
    );
    let peer = doc.peer();
    assert_eq!(peer, PeerId(9));

    // The document.
    doc.commit();
    doc.commit_without_undo();
    assert_eq!(doc.version().encode(), doc.version().encode());
    assert_eq!(doc.version_vector(), VersionVector::new());
    assert_eq!(doc.stats().ops, 0);
    assert!(doc.checkout(&doc.version()).is_err());
    doc.checkout_latest();
    assert!(!doc.is_detached());
    assert!(doc.registry("paragraphs").is_err());
    assert!(doc.root_map("body").is_err());
    assert_eq!(
        doc.export_updates(&VersionVector::new()),
        Err(CrdtError::Poisoned)
    );
    assert_eq!(
        doc.export_shallow_snapshot(&doc.version()),
        Err(CrdtError::Poisoned)
    );

    // Stories, before and after.
    for story in [story_before, doc.main_story()] {
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
    for stories in [stories_before, doc.stories()] {
        assert!(stories.ids().is_empty());
        assert!(stories.entries().is_empty());
        assert!(stories.get("s").is_none());
        assert!(stories.create("s").is_err());
    }
    // Registries, maps and lists obtained before.
    assert!(registry_before.ids().is_empty());
    assert!(registry_before.entries().is_empty());
    assert_eq!(registry_before.len(), 0);
    assert!(registry_before.is_empty());
    assert!(registry_before.get("t").is_none());
    assert!(registry_before.create("u").is_err());
    assert_eq!(map_before.get("pgSz.w"), None);
    assert!(map_before.entries().is_empty());
    assert_eq!(
        map_before.set("k", &Value::Int(1)),
        Err(CrdtError::Poisoned)
    );
    assert_eq!(map_before.remove("k"), Err(CrdtError::Poisoned));
    assert!(map_before.id_list("rows").is_err());
    assert!(map_before.get_id_list("rows").is_none());
    assert_eq!(list_before.len(), 0);
    assert!(list_before.is_empty());
    assert!(list_before.items().is_empty());
    assert_eq!(list_before.insert(0, "r"), Err(CrdtError::Poisoned));
    assert_eq!(list_before.delete(0), Err(CrdtError::Poisoned));
    assert_eq!(list_before.move_item(0, 0), Err(CrdtError::Poisoned));

    // The undo manager, and a new one.
    assert_eq!(undo.undo(), Err(CrdtError::Poisoned));
    assert_eq!(undo.redo(), Err(CrdtError::Poisoned));
    assert!(!undo.can_undo());
    assert!(!undo.can_redo());
    assert_eq!(undo.undo_count(), 0);
    let mut fresh = UndoManager::new(&doc);
    assert_eq!(fresh.undo(), Err(CrdtError::Poisoned));
    assert!(!fresh.can_undo());
}
