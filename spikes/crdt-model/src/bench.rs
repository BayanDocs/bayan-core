//! The performance measurements of the brief, natively and (compiled for wasm32-wasip1) in WebAssembly under Node.js.
//!
//! `generate` builds a synthetic document and writes its snapshot; `bench` loads that snapshot in a fresh process, so that load time and memory are not distorted by the build. Every measurement is printed as `metric <name> <value> <unit>`.

use std::path::Path;
use std::time::Instant;

use bayan_crdt::{Doc, ImportLimits, PeerId, UndoManager, Value};
use bayan_model::{
    AtomKind, Document, EntityId, RawDocument, check_invariants, marks, normalize, registry,
};
use crdt_workload::{
    EDIT_SEED, Edit, Resolved, Rng, Shape, canonical_text, edit_script, fnv1a64, metric, millis,
    paragraphs, peak_resident_bytes, resident_bytes, wasm_linear_memory_bytes,
};

use crate::synthetic;

/// Which document a benchmark uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The brief's full document in the BayanDocs model.
    Full,
    /// The text-with-marks-and-block-markers subset shared with the Automerge comparison.
    Subset,
}

const MAIN: EntityId = EntityId::MAIN_STORY;

fn memory(prefix: &str, baseline: Option<u64>) {
    if let (Some(now), Some(before)) = (resident_bytes(), baseline) {
        metric(
            &format!("{prefix}_rss_delta"),
            now.saturating_sub(before),
            "bytes",
        );
    }
    if let Some(linear) = wasm_linear_memory_bytes() {
        metric(&format!("{prefix}_wasm_linear"), linear, "bytes");
    }
}

/// Builds the document of `mode` and writes its snapshot to `out` (and its shallow snapshot, the state with only the latest version's history, to `out` + `.shallow`).
///
/// # Errors
///
/// A description of what failed.
pub fn generate(mode: Mode, shape: &Shape, out: &Path) -> Result<(), String> {
    let started = Instant::now();
    let (doc, contents) = match mode {
        Mode::Full => synthetic::full(shape)?,
        Mode::Subset => synthetic::subset(shape)?,
    };
    metric("build_ms", millis(started.elapsed()), "ms");
    let started = Instant::now();
    let snapshot = doc.export_snapshot().map_err(|error| error.to_string())?;
    metric("export_snapshot_ms", millis(started.elapsed()), "ms");
    metric("snapshot_bytes", snapshot.len(), "bytes");
    let shallow = doc
        .export_shallow_snapshot(&doc.version())
        .map_err(|error| error.to_string())?;
    metric("shallow_snapshot_bytes", shallow.len(), "bytes");
    let stats = doc.stats();
    metric("ops", stats.ops, "count");
    metric("changes", stats.changes, "count");
    metric("main_len", contents.main_len, "count");
    metric("paragraphs", contents.paragraphs, "count");
    metric("tables", contents.tables, "count");
    metric("cells", contents.cells, "count");
    metric("comments", contents.comments, "count");
    metric("fields", contents.fields, "count");
    metric("objects", contents.objects, "count");
    metric("bookmarks", contents.bookmarks, "count");
    match mode {
        Mode::Subset => {
            let stored = fnv1a64(synthetic::subset_text(&doc).as_bytes());
            let generated = fnv1a64(
                canonical_text(&paragraphs(
                    &mut Rng::new(crdt_workload::DOCUMENT_SEED),
                    shape,
                ))
                .as_bytes(),
            );
            metric("text_fnv1a64", format!("{stored:016x}"), "hash");
            metric("generator_fnv1a64", format!("{generated:016x}"), "hash");
            if stored != generated {
                return Err("the stored text differs from the generated text".to_owned());
            }
        }
        Mode::Full => {
            let (view, report) = normalize(&RawDocument::read(&doc));
            let violations = check_invariants(&view);
            metric("view_violations", violations.len(), "count");
            metric(
                "view_repairs",
                report.n1 + report.n2 + report.n3 + report.n4 + report.n5 + report.n6 + report.n7,
                "count",
            );
            metric("view_paragraphs", view.paragraphs.len(), "count");
            metric("view_stories", view.stories.len(), "count");
            if !violations.is_empty() {
                return Err(format!(
                    "the synthetic document breaks invariants: {violations:?}"
                ));
            }
        }
    }
    std::fs::write(out, &snapshot)
        .map_err(|error| format!("cannot write {}: {error}", out.display()))?;
    let mut shallow_path = out.as_os_str().to_owned();
    shallow_path.push(".shallow");
    std::fs::write(&shallow_path, &shallow)
        .map_err(|error| format!("cannot write the shallow snapshot: {error}"))?;
    Ok(())
}

/// Loads the snapshot at `path` and measures (see the module documentation and the report).
///
/// # Errors
///
/// A description of what failed.
pub fn bench(mode: Mode, path: &Path, edits: usize) -> Result<(), String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    metric("snapshot_bytes", bytes.len(), "bytes");
    let baseline = resident_bytes();
    match mode {
        Mode::Full => bench_full(&bytes, baseline, edits),
        Mode::Subset => bench_subset(&bytes, baseline, edits),
    }?;
    if let Some(peak) = peak_resident_bytes() {
        metric("peak_rss", peak, "bytes");
    }
    if let Some(linear) = wasm_linear_memory_bytes() {
        metric("wasm_linear_memory_end", linear, "bytes");
    }
    Ok(())
}

fn bench_full(bytes: &[u8], baseline: Option<u64>, edits: usize) -> Result<(), String> {
    let err = |error: bayan_model::EditError| error.to_string();
    let started = Instant::now();
    let mut a = Document::load(bytes, 2, 2, &ImportLimits::LOCAL_SNAPSHOT).map_err(err)?;
    let load_ms = started.elapsed();
    metric("load_ms", millis(load_ms), "ms");
    let started = Instant::now();
    let raw = RawDocument::read(a.crdt());
    let raw_ms = started.elapsed();
    let started = Instant::now();
    let (view, _) = normalize(&raw);
    let normalize_ms = started.elapsed();
    metric("raw_read_ms", millis(raw_ms), "ms");
    metric("normalize_ms", millis(normalize_ms), "ms");
    metric("read_ms", millis(raw_ms + normalize_ms), "ms");
    // Loro decodes a snapshot's containers when they are first read, so a document is ready to show only after the first read: this is the time the targets for loading apply to.
    metric(
        "load_and_read_ms",
        millis(load_ms + raw_ms + normalize_ms),
        "ms",
    );
    drop(raw);
    metric("view_main_items", view.main().len(), "count");
    metric("view_violations", check_invariants(&view).len(), "count");
    memory("memory_after_load", baseline);
    drop(view);

    let mut b = Document::load(bytes, 3, 3, &ImportLimits::LOCAL_SNAPSHOT).map_err(err)?;
    // The second replica is shown once before it is edited, as an editor would, so that the edits are not charged with decoding the snapshot (already measured as read_ms).
    drop(b.view());
    let script = edit_script(EDIT_SEED, edits);
    let mut local = std::time::Duration::ZERO;
    let mut per_edit_bytes = 0;
    let mut refused = 0;
    let mut previous = b.version_vector();
    for edit in &script {
        let len = b.story_len(MAIN).unwrap_or(1);
        let started = Instant::now();
        let result = match edit.resolve(len) {
            Resolved::Insert { at, text } => b.insert_text(MAIN, at, text),
            Resolved::Delete { range } => b.delete(MAIN, range),
            Resolved::Bold { range } => b.format(MAIN, range, marks::BOLD, &Value::Bool(true)),
            Resolved::Split { at } => b.split_paragraph(MAIN, at).map(|_| ()),
        };
        local += started.elapsed();
        if result.is_err() {
            refused += 1;
            continue;
        }
        let update = b.export_updates(&previous).map_err(err)?;
        per_edit_bytes += update.len();
        previous = b.version_vector();
    }
    let applied = script.len() - refused;
    metric("local_edits_ms", millis(local), "ms");
    metric("edits_applied", applied, "count");
    metric("edits_refused", refused, "count");
    metric(
        "update_bytes_per_edit_avg",
        per_edit_bytes / applied.max(1),
        "bytes",
    );

    let update = b.export_updates(&a.version_vector()).map_err(err)?;
    metric("update_bytes", update.len(), "bytes");
    let started = Instant::now();
    a.import(&update, &ImportLimits::UPDATE).map_err(err)?;
    metric("apply_update_ms", millis(started.elapsed()), "ms");
    let started = Instant::now();
    let view_a = a.view();
    metric("read_after_update_ms", millis(started.elapsed()), "ms");
    let equal = view_a == b.view();
    metric("replicas_equal", equal, "bool");
    metric(
        "view_violations_after_update",
        check_invariants(&view_a).len(),
        "count",
    );
    drop(view_a);
    if !equal {
        return Err("the replicas differ after the update".to_owned());
    }

    // Undo latency on the replica that made the edits.
    let started = Instant::now();
    let mut undone = 0_u32;
    for _ in 0..20 {
        if b.undo().map_err(err)? {
            undone += 1;
        }
    }
    let elapsed = started.elapsed();
    metric("undo_steps", undone, "count");
    metric("undo_ms_avg", millis(elapsed / undone.max(1)), "ms");

    let started = Instant::now();
    let snapshot = a.export_snapshot().map_err(err)?;
    metric(
        "export_snapshot_after_edits_ms",
        millis(started.elapsed()),
        "ms",
    );
    metric("snapshot_after_edits_bytes", snapshot.len(), "bytes");
    let shallow = a
        .crdt()
        .export_shallow_snapshot(&a.version())
        .map_err(|error| error.to_string())?;
    metric("shallow_snapshot_after_edits_bytes", shallow.len(), "bytes");

    // Later updates of the same size. The first update after loading also pays for decoding the containers it touches and loading the history it needs; these show the cost once the document has been open for a while.
    let mut later = std::time::Duration::ZERO;
    for round in 1..=LATER_UPDATES {
        let before = a.version_vector();
        for edit in &edit_script(EDIT_SEED ^ (u64::from(round) << 8), edits) {
            let len = b.story_len(MAIN).unwrap_or(1);
            let _ = match edit.resolve(len) {
                Resolved::Insert { at, text } => b.insert_text(MAIN, at, text),
                Resolved::Delete { range } => b.delete(MAIN, range),
                Resolved::Bold { range } => b.format(MAIN, range, marks::BOLD, &Value::Bool(true)),
                Resolved::Split { at } => b.split_paragraph(MAIN, at).map(|_| ()),
            };
        }
        let update = b.export_updates(&before).map_err(err)?;
        let started = Instant::now();
        a.import(&update, &ImportLimits::UPDATE).map_err(err)?;
        later += started.elapsed();
    }
    metric(
        "apply_later_update_ms_avg",
        millis(later / LATER_UPDATES),
        "ms",
    );
    concurrent_full(&mut a, &mut b)
}

/// How many later updates the benchmarks apply after the first (see `apply_later_update_ms_avg`).
const LATER_UPDATES: u32 = 3;

/// Collaboration, where both replicas edit before they exchange changes: a keystroke on each replica, imported by the other, eleven times (the first import is reported separately, because the library builds a cache on the first concurrent change); then a session of 1,000 edits on one replica while the other makes 50, imported at once.
fn concurrent_full(a: &mut Document, b: &mut Document) -> Result<(), String> {
    let err = |error: bayan_model::EditError| error.to_string();
    let exchange = |from: &Document, to: &mut Document| -> Result<std::time::Duration, String> {
        let update = from.export_updates(&to.version_vector()).map_err(err)?;
        let started = Instant::now();
        to.import(&update, &ImportLimits::UPDATE).map_err(err)?;
        Ok(started.elapsed())
    };
    exchange(a, b)?;
    exchange(b, a)?;
    let mut first = std::time::Duration::ZERO;
    let mut warm = std::time::Duration::ZERO;
    for round in 0..11_usize {
        type_somewhere(a, round)?;
        type_somewhere(b, round + 101)?;
        let elapsed = exchange(b, a)?;
        if round == 0 {
            first = elapsed;
        } else {
            warm += elapsed;
        }
        exchange(a, b)?;
    }
    metric("concurrent_keystroke_first_ms", millis(first), "ms");
    metric("concurrent_keystroke_ms_avg", millis(warm / 10), "ms");
    let script = edit_script(EDIT_SEED ^ 2, 1_000);
    for (index, edit) in script.iter().enumerate() {
        let len = b.story_len(MAIN).unwrap_or(1);
        let _ = match edit.resolve(len) {
            Resolved::Insert { at, text } => b.insert_text(MAIN, at, text),
            Resolved::Delete { range } => b.delete(MAIN, range),
            Resolved::Bold { range } => b.format(MAIN, range, marks::BOLD, &Value::Bool(true)),
            Resolved::Split { at } => b.split_paragraph(MAIN, at).map(|_| ()),
        };
        if index % 20 == 0 {
            type_somewhere(a, index)?;
        }
    }
    let update = b.export_updates(&a.version_vector()).map_err(err)?;
    metric("concurrent_update_bytes", update.len(), "bytes");
    let started = Instant::now();
    a.import(&update, &ImportLimits::UPDATE).map_err(err)?;
    metric(
        "concurrent_apply_update_ms",
        millis(started.elapsed()),
        "ms",
    );
    exchange(a, b)?;
    let equal = a.view() == b.view();
    metric("concurrent_replicas_equal", equal, "bool");
    if equal {
        Ok(())
    } else {
        Err("the replicas differ after concurrent editing".to_owned())
    }
}

/// Types one character somewhere in the middle of the main story (trying a few positions, since a few are refused, such as right before a table).
fn type_somewhere(document: &mut Document, salt: usize) -> Result<(), String> {
    let len = document.story_len(MAIN).unwrap_or(1).max(2);
    for attempt in 0..32 {
        let pos = (len / 3 + salt * 7_919 + attempt) % (len - 1);
        if document.insert_text(MAIN, pos, "k").is_ok() {
            return Ok(());
        }
    }
    Err("found no position to type at".to_owned())
}

fn bench_subset(bytes: &[u8], baseline: Option<u64>, edits: usize) -> Result<(), String> {
    let err = |error: bayan_crdt::CrdtError| error.to_string();
    let started = Instant::now();
    let a = Doc::load(
        bytes,
        PeerId(2),
        &marks::FAMILIES,
        &ImportLimits::LOCAL_SNAPSHOT,
    )
    .map_err(err)?;
    let load_ms = started.elapsed();
    metric("load_ms", millis(load_ms), "ms");
    let started = Instant::now();
    let (text, marks_read, blocks, entities) = read_subset(&a);
    let read_ms = started.elapsed();
    metric("read_ms", millis(read_ms), "ms");
    metric("load_and_read_ms", millis(load_ms + read_ms), "ms");
    metric(
        "text_fnv1a64",
        format!("{:016x}", fnv1a64(text.as_bytes())),
        "hash",
    );
    metric("marks_read", marks_read, "count");
    metric("blocks_read", blocks, "count");
    metric("paragraph_entities_read", entities, "count");
    memory("memory_after_load", baseline);

    let b = Doc::load(
        bytes,
        PeerId(3),
        &marks::FAMILIES,
        &ImportLimits::LOCAL_SNAPSHOT,
    )
    .map_err(err)?;
    // Shown once before it is edited, as in bench_full.
    drop(read_subset(&b));
    let mut undo = UndoManager::new(&b);
    let main = b.main_story();
    let paragraphs_registry = b.registry(registry::PARAGRAPHS);
    let mut ids = Rng::new(EDIT_SEED ^ 1);
    let script = edit_script(EDIT_SEED, edits);
    let mut local = std::time::Duration::ZERO;
    let mut per_edit_bytes = 0;
    let mut previous = b.version_vector();
    let mut len = main.len();
    for edit in &script {
        let resolved = edit.resolve(len);
        let started = Instant::now();
        match &resolved {
            Resolved::Insert { at, text } => main.insert(*at, text).map_err(err)?,
            Resolved::Delete { range } => main.delete(range.clone()).map_err(err)?,
            Resolved::Bold { range } => main
                .mark(range.clone(), marks::BOLD, &Value::Bool(true))
                .map_err(err)?,
            Resolved::Split { at } => {
                let id = ids.hex_id();
                paragraphs_registry
                    .create(&id)
                    .and_then(|map| map.set("type", &Value::from("paragraph")))
                    .map_err(err)?;
                main.insert_atom(
                    *at,
                    AtomKind::ParagraphEnd.placeholder(),
                    &format!("p:{id}"),
                )
                .map_err(err)?;
            }
        }
        b.commit();
        local += started.elapsed();
        len = Edit::new_len(&resolved, len);
        let update = b.export_updates(&previous).map_err(err)?;
        per_edit_bytes += update.len();
        previous = b.version_vector();
    }
    metric("local_edits_ms", millis(local), "ms");
    metric(
        "update_bytes_per_edit_avg",
        per_edit_bytes / script.len().max(1),
        "bytes",
    );
    let update = b.export_updates(&a.version_vector()).map_err(err)?;
    metric("update_bytes", update.len(), "bytes");
    let started = Instant::now();
    a.import(&update, &ImportLimits::UPDATE)
        .map_err(|error| error.to_string())?;
    metric("apply_update_ms", millis(started.elapsed()), "ms");
    let equal = a.main_story().runs() == main.runs();
    metric("replicas_equal", equal, "bool");
    if !equal {
        return Err("the replicas differ after the update".to_owned());
    }
    let started = Instant::now();
    let (text, _, _, _) = read_subset(&a);
    metric("read_after_update_ms", millis(started.elapsed()), "ms");
    metric(
        "final_text_fnv1a64",
        format!("{:016x}", fnv1a64(text.as_bytes())),
        "hash",
    );
    metric("final_sequence_len", a.main_story().len(), "count");
    let started = Instant::now();
    let mut undone = 0_u32;
    for _ in 0..20 {
        if undo.undo().map_err(err)? {
            undone += 1;
        }
    }
    metric("undo_steps", undone, "count");
    metric(
        "undo_ms_avg",
        millis(started.elapsed() / undone.max(1)),
        "ms",
    );

    // Later updates of the same size, as in bench_full.
    let mut later = std::time::Duration::ZERO;
    for round in 1..=LATER_UPDATES {
        let before = a.version_vector();
        let mut len = main.len();
        for edit in &edit_script(EDIT_SEED ^ (u64::from(round) << 8), edits) {
            let resolved = edit.resolve(len);
            match &resolved {
                Resolved::Insert { at, text } => main.insert(*at, text).map_err(err)?,
                Resolved::Delete { range } => main.delete(range.clone()).map_err(err)?,
                Resolved::Bold { range } => main
                    .mark(range.clone(), marks::BOLD, &Value::Bool(true))
                    .map_err(err)?,
                Resolved::Split { at } => main
                    .insert_atom(
                        *at,
                        AtomKind::ParagraphEnd.placeholder(),
                        "p:00000000000000000000000000000002",
                    )
                    .map_err(err)?,
            }
            b.commit();
            len = Edit::new_len(&resolved, len);
        }
        let update = b.export_updates(&before).map_err(err)?;
        let started = Instant::now();
        a.import(&update, &ImportLimits::UPDATE)
            .map_err(|error| error.to_string())?;
        later += started.elapsed();
    }
    metric(
        "apply_later_update_ms_avg",
        millis(later / LATER_UPDATES),
        "ms",
    );
    concurrent_subset(&a, &b)
}

/// The subset's version of [`concurrent_full`], with the adapter's operations.
fn concurrent_subset(a: &Doc, b: &Doc) -> Result<(), String> {
    let err = |error: bayan_crdt::CrdtError| error.to_string();
    let exchange = |from: &Doc, to: &Doc| -> Result<std::time::Duration, String> {
        let update = from.export_updates(&to.version_vector()).map_err(err)?;
        let started = Instant::now();
        to.import(&update, &ImportLimits::UPDATE)
            .map_err(|error| error.to_string())?;
        Ok(started.elapsed())
    };
    let type_at = |doc: &Doc, salt: usize| -> Result<(), String> {
        let main = doc.main_story();
        let len = main.len().max(2);
        main.insert((len / 3 + salt * 7_919) % (len - 1), "k")
            .map_err(err)?;
        doc.commit();
        Ok(())
    };
    exchange(a, b)?;
    exchange(b, a)?;
    let mut first = std::time::Duration::ZERO;
    let mut warm = std::time::Duration::ZERO;
    for round in 0..11_usize {
        type_at(a, round)?;
        type_at(b, round + 101)?;
        let elapsed = exchange(b, a)?;
        if round == 0 {
            first = elapsed;
        } else {
            warm += elapsed;
        }
        exchange(a, b)?;
    }
    metric("concurrent_keystroke_first_ms", millis(first), "ms");
    metric("concurrent_keystroke_ms_avg", millis(warm / 10), "ms");
    let main = b.main_story();
    let mut len = main.len();
    for (index, edit) in edit_script(EDIT_SEED ^ 2, 1_000).iter().enumerate() {
        let resolved = edit.resolve(len);
        match &resolved {
            Resolved::Insert { at, text } => main.insert(*at, text).map_err(err)?,
            Resolved::Delete { range } => main.delete(range.clone()).map_err(err)?,
            Resolved::Bold { range } => main
                .mark(range.clone(), marks::BOLD, &Value::Bool(true))
                .map_err(err)?,
            Resolved::Split { at } => main
                .insert_atom(
                    *at,
                    AtomKind::ParagraphEnd.placeholder(),
                    "p:00000000000000000000000000000001",
                )
                .map_err(err)?,
        }
        b.commit();
        len = Edit::new_len(&resolved, len);
        if index % 20 == 0 {
            type_at(a, index)?;
        }
    }
    let update = b.export_updates(&a.version_vector()).map_err(err)?;
    metric("concurrent_update_bytes", update.len(), "bytes");
    let started = Instant::now();
    a.import(&update, &ImportLimits::UPDATE)
        .map_err(|error| error.to_string())?;
    metric(
        "concurrent_apply_update_ms",
        millis(started.elapsed()),
        "ms",
    );
    exchange(a, b)?;
    let equal = a.main_story().runs() == b.main_story().runs();
    metric("concurrent_replicas_equal", equal, "bool");
    if equal {
        Ok(())
    } else {
        Err("the replicas differ after concurrent editing".to_owned())
    }
}

/// Reads the subset completely: every run with its marks, and every paragraph entity. Returns the canonical text, the number of marks (a key and value starting on a run where it was not already set), the number of paragraph ends and the number of paragraph entities.
fn read_subset(doc: &Doc) -> (String, usize, usize, usize) {
    let runs = doc.main_story().runs();
    let mut text = String::new();
    let mut blocks = 0;
    let mut open: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
    let mut marks_read = 0;
    for run in &runs {
        for (key, value) in &run.marks {
            if key != bayan_crdt::ATOM_KEY && open.get(key) != Some(value) {
                marks_read += 1;
            }
        }
        open = run
            .marks
            .iter()
            .filter(|(key, _)| key.as_str() != bayan_crdt::ATOM_KEY)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        for character in run.text.chars() {
            if character == AtomKind::ParagraphEnd.placeholder() {
                text.push('\n');
                blocks += 1;
            } else {
                text.push(character);
            }
        }
    }
    let entities = doc
        .registry(registry::PARAGRAPHS)
        .entries()
        .values()
        .filter(|map| !map.entries().is_empty())
        .count();
    // The text after the last paragraph end is empty; drop the final line feed to match the canonical text.
    text.pop();
    (text, marks_read, blocks, entities)
}
