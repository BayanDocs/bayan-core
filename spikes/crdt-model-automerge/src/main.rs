//! # crdt-model-automerge
//!
//! The Automerge side of the CORE-004 comparison: it builds the synthetic document of `crdt-workload` as one Automerge text object and measures loading, reading, local editing and merging, natively and in WebAssembly, on exactly the workload that the Loro benchmark (`spikes/crdt-model`) runs.
//!
//! **Document shape.** The root map holds one text object under the key `main`. Every paragraph starts with an Automerge block marker (`split_block`) whose map is `{"type": "paragraph", "id": <32 hexadecimal digits>}`. Bold and italic are the marks `r:b` and `r:i` (value `true`, expanding after), and each comment is a mark `cmt:<id>` (value `true`, expanding neither way).
//!
//! **Positions.** The documents are created and loaded with `TextEncoding::UnicodeCodePoint`, so Automerge counts one position per Unicode scalar value and one per block marker, which is exactly a *sequence position* of `crdt-workload`. (It is also Automerge's default when neither its `utf8-indexing` nor its `utf16-indexing` feature is enabled, but the benchmark sets it explicitly rather than relying on feature unification.)
//!
//! **Commands.**
//! - `generate --out <file> [--small]` builds the document (the 500-page shape, or ten pages with `--small`), prints its metrics and writes the bytes of `save()` to the file.
//! - `bench --snapshot <file> [--edits N]` loads the snapshot twice, reads the first copy, applies the edit script to the second copy one change per edit, merges all the edits into the first copy as one update and checks that both copies agree.
//!
//! Every measurement is one line `metric <name> <value> <unit>` on standard output (`crdt_workload::metric`).
//!
//! **Status:** spike code (CORE-004).

#![forbid(unsafe_code)]

use std::ops::Range;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use automerge::marks::{ExpandMark, Mark};
use automerge::transaction::Transactable;
use automerge::{
    ActorId, AutoCommit, AutomergeError, Change, ChangeHash, LoadOptions, ObjId, ObjType, ROOT,
    ReadDoc, ScalarValue, Span, TextEncoding, Value, hydrate,
};
use crdt_workload::{
    DOCUMENT_SEED, EDIT_SEED, Edit, FIVE_HUNDRED_PAGES, Resolved, Rng, Shape, TEN_PAGES,
    canonical_text, comments, edit_script, fnv1a64, metric, millis, paragraphs,
    peak_resident_bytes, resident_bytes, sequence_len, wasm_linear_memory_bytes,
};

/// The key of the text object in the root map.
const TEXT_KEY: &str = "main";

/// How Automerge counts positions: one per Unicode scalar value, and one per block marker.
const ENCODING: TextEncoding = TextEncoding::UnicodeCodePoint;

/// The character that Automerge's `text()` returns in place of each block marker.
const BLOCK_PLACEHOLDER: char = '\u{fffc}';

/// The block type of a paragraph.
const PARAGRAPH: &str = "paragraph";

/// The mark name of bold.
const BOLD: &str = "r:b";

/// The mark name of italic.
const ITALIC: &str = "r:i";

/// The generator commits after every this many paragraphs, and again after every this many comments.
const COMMIT_EVERY: usize = 100;

/// The number of edits of a `bench` run unless `--edits` says otherwise.
const DEFAULT_EDITS: usize = 1_000;

/// The actor of the generator. Fixed, so that the same command always writes the same bytes (Automerge's change timestamps are 0 unless the caller sets them).
const BUILDER_ACTOR: [u8; 16] = *b"core-004-builder";

/// The actor of the copy that applies the edit script, fixed for the same reason.
const EDITOR_ACTOR: [u8; 16] = *b"core-004-editor1";

/// Runs the command given on the command line; on failure, says why and exits with a failure status.
fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match parse(&arguments).and_then(run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            report_failure(&message);
            ExitCode::FAILURE
        }
    }
}

/// Prints why the run failed.
#[expect(
    clippy::print_stderr,
    reason = "a benchmark that cannot finish says why on standard error"
)]
fn report_failure(message: &str) {
    eprintln!("crdt-model-automerge: {message}");
    eprintln!(
        "usage: crdt-model-automerge generate --out <file> [--small] | bench --snapshot <file> [--edits N]"
    );
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Build the synthetic document and write its snapshot.
    Generate {
        /// Where to write the snapshot.
        out: String,
        /// Build the ten-page document instead of the 500-page one.
        small: bool,
    },
    /// Measure a snapshot.
    Bench {
        /// The snapshot to load.
        snapshot: String,
        /// The number of edits to apply.
        edits: usize,
    },
}

/// Reads the command line (without the program name).
fn parse(arguments: &[String]) -> Result<Command, String> {
    let (command, rest) = arguments
        .split_first()
        .ok_or_else(|| "missing command".to_owned())?;
    let mut rest = rest.iter();
    match command.as_str() {
        "generate" => {
            let mut out = None;
            let mut small = false;
            while let Some(argument) = rest.next() {
                match argument.as_str() {
                    "--out" => out = Some(value_of("--out", rest.next())?),
                    "--small" => small = true,
                    other => return Err(format!("unknown argument `{other}`")),
                }
            }
            let out = out.ok_or_else(|| "generate needs --out <file>".to_owned())?;
            Ok(Command::Generate { out, small })
        }
        "bench" => {
            let mut snapshot = None;
            let mut edits = DEFAULT_EDITS;
            while let Some(argument) = rest.next() {
                match argument.as_str() {
                    "--snapshot" => snapshot = Some(value_of("--snapshot", rest.next())?),
                    "--edits" => {
                        let value = value_of("--edits", rest.next())?;
                        edits = value
                            .parse()
                            .map_err(|_| format!("--edits needs a number, not `{value}`"))?;
                    }
                    other => return Err(format!("unknown argument `{other}`")),
                }
            }
            let snapshot = snapshot.ok_or_else(|| "bench needs --snapshot <file>".to_owned())?;
            Ok(Command::Bench { snapshot, edits })
        }
        other => Err(format!("unknown command `{other}`")),
    }
}

/// The value that follows an option, or an error naming the option.
fn value_of(option: &str, value: Option<&String>) -> Result<String, String> {
    value
        .cloned()
        .ok_or_else(|| format!("{option} needs a value"))
}

/// Runs a parsed command.
fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Generate { out, small } => {
            generate(&out, if small { TEN_PAGES } else { FIVE_HUNDRED_PAGES })
        }
        Command::Bench { snapshot, edits } => bench(&snapshot, edits),
    }
}

/// Turns an Automerge error into a message.
fn automerge_error(error: AutomergeError) -> String {
    format!("Automerge: {error}")
}

/// The synthetic document built in Automerge, with what the generator knows about it.
struct Built {
    /// The document, all changes committed.
    doc: AutoCommit,
    /// The text object.
    text: ObjId,
    /// How long building and committing took.
    build: Duration,
    /// The number of paragraphs.
    paragraphs: usize,
    /// The number of marks applied (bold, italic and comments).
    marks: usize,
    /// The sequence length the generator expects.
    sequence_len: usize,
    /// The canonical text the generator expects.
    canonical_text: String,
}

/// Builds the synthetic document of `shape`: for each paragraph a block marker, its text, and its bold and italic marks (one commit per 100 paragraphs), then the comments (one commit per 100 comments).
fn build(shape: &Shape) -> Result<Built, String> {
    // The calls, in this order, that both CORE-004 benchmarks make, so that they build the same text, formatting and comment ranges.
    let mut rng = Rng::new(DOCUMENT_SEED);
    let document = paragraphs(&mut rng, shape);
    let expected_len = sequence_len(&document);
    let comment_ranges = comments(&mut rng, expected_len, shape.comments);
    let lengths: Vec<usize> = document
        .iter()
        .map(crdt_workload::Paragraph::chars)
        .collect();
    // One generator for every identifier: the paragraphs' first, then the comments'.
    let mut ids = Rng::new(DOCUMENT_SEED ^ 1);

    let start = Instant::now();
    let mut doc =
        AutoCommit::new_with_encoding(ENCODING).with_actor(ActorId::from(BUILDER_ACTOR.as_slice()));
    let text = doc
        .put_object(ROOT, TEXT_KEY, ObjType::Text)
        .map_err(automerge_error)?;
    let mut len = 0;
    let mut marks = 0;
    for (index, (paragraph, chars)) in document.iter().zip(&lengths).enumerate() {
        insert_block(&mut doc, &text, len, &mut ids)?;
        let first = len + 1;
        doc.splice_text(&text, first, 0, &paragraph.text)
            .map_err(automerge_error)?;
        len = first + chars;
        for (name, range) in [(BOLD, &paragraph.bold), (ITALIC, &paragraph.italic)] {
            if let Some(range) = range {
                let range = first + range.start..first + range.end;
                if add_mark(&mut doc, &text, name, range, ExpandMark::After)? {
                    marks += 1;
                }
            }
        }
        if (index + 1).is_multiple_of(COMMIT_EVERY) {
            doc.commit();
        }
    }
    doc.commit();
    for (index, comment) in comment_ranges.iter().enumerate() {
        let name = format!("cmt:{}", ids.hex_id());
        let range = comment.start..comment.start + comment.len;
        if add_mark(&mut doc, &text, &name, range, ExpandMark::None)? {
            marks += 1;
        }
        if (index + 1).is_multiple_of(COMMIT_EVERY) {
            doc.commit();
        }
    }
    doc.commit();
    let build = start.elapsed();

    Ok(Built {
        doc,
        text,
        build,
        paragraphs: document.len(),
        marks,
        sequence_len: expected_len,
        canonical_text: canonical_text(&document),
    })
}

/// Inserts a paragraph block marker at `at`, with a fresh identifier from `ids`.
fn insert_block(
    doc: &mut AutoCommit,
    text: &ObjId,
    at: usize,
    ids: &mut Rng,
) -> Result<(), String> {
    let block = doc.split_block(text, at).map_err(automerge_error)?;
    doc.put(&block, "type", PARAGRAPH)
        .map_err(automerge_error)?;
    doc.put(&block, "id", ids.hex_id())
        .map_err(automerge_error)?;
    Ok(())
}

/// Marks `range` with `name` = true. Does nothing and returns false when the range is empty.
fn add_mark(
    doc: &mut AutoCommit,
    text: &ObjId,
    name: &str,
    range: Range<usize>,
    expand: ExpandMark,
) -> Result<bool, String> {
    if range.is_empty() {
        return Ok(false);
    }
    let mark = Mark::new(name.to_owned(), true, range.start, range.end);
    doc.mark(text, mark, expand).map_err(automerge_error)?;
    Ok(true)
}

/// One block marker as a reader sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Block {
    /// Its sequence position.
    position: usize,
    /// Its `type` (always `paragraph` here).
    kind: String,
    /// Its `id`.
    id: String,
}

/// The complete content of the text object, as a reader sees it.
#[derive(Debug, Clone, PartialEq)]
struct Content {
    /// The text as Automerge returns it, with U+FFFC in place of each block marker.
    text: String,
    /// Every mark: name, value and range of sequence positions (Automerge merges touching ranges with the same name and value).
    marks: Vec<Mark>,
    /// Every block marker, in order.
    blocks: Vec<Block>,
}

impl Content {
    /// The canonical text of `crdt-workload`: the paragraphs' texts joined by line feeds. The block marker that starts the document is dropped and every other one becomes a line feed. (Text in front of the first block marker, which edits can create, counts as a paragraph of its own.)
    fn canonical_text(&self) -> String {
        let body = self
            .text
            .strip_prefix(BLOCK_PLACEHOLDER)
            .unwrap_or(&self.text);
        body.replace(BLOCK_PLACEHOLDER, "\n")
    }
}

/// Reads the complete content of the text object, the fastest way Automerge offers: one pass over `spans()` yields the text and every block marker with its map (the spans also carry the set of marks active on each run of text, which this read does not keep), and `marks()` yields every mark with its range.
fn read_content(doc: &AutoCommit, text: &ObjId) -> Result<Content, String> {
    let mut string = String::new();
    let mut blocks = Vec::new();
    let mut position = 0;
    for span in doc.spans(text).map_err(automerge_error)? {
        match span {
            Span::Text { text: run, .. } => {
                position += run.chars().count();
                string.push_str(&run);
            }
            Span::Block(map) => {
                blocks.push(Block {
                    position,
                    kind: hydrated_string(&map, "type")?,
                    id: hydrated_string(&map, "id")?,
                });
                string.push(BLOCK_PLACEHOLDER);
                position += 1;
            }
        }
    }
    let marks = doc.marks(text).map_err(automerge_error)?;
    Ok(Content {
        text: string,
        marks,
        blocks,
    })
}

/// Reads the same content another way, to check [`read_content`]: the text with `text()`, the marks with `marks()`, and each block marker with one `get` per U+FFFC of the text, then `get` of the map's `type` and `id`. (Slower: about 12 microseconds per block marker on the 500-page document.)
fn read_content_by_index(doc: &AutoCommit, text: &ObjId) -> Result<Content, String> {
    let string = doc.text(text).map_err(automerge_error)?;
    let marks = doc.marks(text).map_err(automerge_error)?;
    let mut blocks = Vec::new();
    for (position, character) in string.chars().enumerate() {
        if character == BLOCK_PLACEHOLDER {
            blocks.push(read_block(doc, text, position)?);
        }
    }
    Ok(Content {
        text: string,
        marks,
        blocks,
    })
}

/// Reads the block marker at `position`.
fn read_block(doc: &AutoCommit, text: &ObjId, position: usize) -> Result<Block, String> {
    match doc.get(text, position).map_err(automerge_error)? {
        Some((Value::Object(ObjType::Map), block)) => Ok(Block {
            position,
            kind: read_string(doc, &block, "type")?,
            id: read_string(doc, &block, "id")?,
        }),
        _ => Err(format!(
            "position {position} reads as U+FFFC but holds no block marker"
        )),
    }
}

/// Reads a string value of a map.
fn read_string(doc: &AutoCommit, map: &ObjId, key: &str) -> Result<String, String> {
    match doc.get(map, key).map_err(automerge_error)? {
        Some((Value::Scalar(value), _)) => match value.as_ref() {
            ScalarValue::Str(string) => Ok(string.to_string()),
            other => Err(format!("block key `{key}` holds {other:?}, not a string")),
        },
        other => Err(format!("block key `{key}` holds {other:?}, not a string")),
    }
}

/// Reads the content both ways and checks that they agree.
fn check_reads_agree(doc: &AutoCommit, text: &ObjId) -> Result<Content, String> {
    let content = read_content(doc, text)?;
    let by_index = read_content_by_index(doc, text)?;
    if content.text != by_index.text {
        return Err("the spans' text differs from text()".to_owned());
    }
    if content.blocks != by_index.blocks {
        return Err("the spans' block markers differ from those read with get".to_owned());
    }
    if content.marks != by_index.marks {
        return Err("marks() changed between two reads".to_owned());
    }
    Ok(content)
}

/// Reads a string value of a hydrated block map.
fn hydrated_string(map: &hydrate::Map, key: &str) -> Result<String, String> {
    match map.get(key) {
        Some(hydrate::Value::Scalar(ScalarValue::Str(string))) => Ok(string.to_string()),
        other => Err(format!("block key `{key}` holds {other:?}, not a string")),
    }
}

/// The text object under `main` in the root map.
fn text_object(doc: &AutoCommit) -> Result<ObjId, String> {
    match doc.get(ROOT, TEXT_KEY).map_err(automerge_error)? {
        Some((Value::Object(ObjType::Text), text)) => Ok(text),
        other => Err(format!(
            "the root key `{TEXT_KEY}` holds {other:?}, not a text"
        )),
    }
}

/// Loads a snapshot or an update with positions counted in Unicode scalar values.
fn load(bytes: &[u8]) -> Result<AutoCommit, String> {
    AutoCommit::load_with_options(bytes, LoadOptions::new().text_encoding(ENCODING))
        .map_err(automerge_error)
}

/// The FNV-1a hash of a text, as 16 hexadecimal digits.
fn text_hash(text: &str) -> String {
    format!("{:016x}", fnv1a64(text.as_bytes()))
}

/// `generate`: builds the document, prints its metrics, checks it against the generator and writes the snapshot.
fn generate(out: &str, shape: Shape) -> Result<(), String> {
    let Built {
        mut doc,
        text,
        build,
        paragraphs,
        marks,
        sequence_len,
        canonical_text,
    } = build(&shape)?;
    metric("build_ms", millis(build), "ms");

    let start = Instant::now();
    let snapshot = doc.save();
    metric("save_ms", millis(start.elapsed()), "ms");
    metric("snapshot_bytes", snapshot.len(), "bytes");

    let length = doc.length(&text);
    metric("sequence_len", length, "count");
    if length != sequence_len {
        return Err(format!(
            "the document has {length} positions, the generator expects {sequence_len}"
        ));
    }
    metric("paragraphs", paragraphs, "count");
    metric("marks", marks, "count");
    let stats = doc.stats();
    metric("ops", stats.num_ops, "count");
    metric("changes", stats.num_changes, "count");

    let content = check_reads_agree(&doc, &text)?;
    let read_back = text_hash(&content.canonical_text());
    let generated = text_hash(&canonical_text);
    metric("text_fnv1a64", &read_back, "hash");
    metric("generator_fnv1a64", &generated, "hash");
    if read_back != generated {
        return Err("the text read back differs from the generator's text".to_owned());
    }
    if content.blocks.len() != paragraphs {
        return Err(format!(
            "{} block markers read back for {paragraphs} paragraphs",
            content.blocks.len()
        ));
    }
    if content.marks.len() != marks {
        return Err(format!(
            "{} marks read back, {marks} applied",
            content.marks.len()
        ));
    }

    std::fs::write(out, &snapshot).map_err(|error| format!("cannot write {out}: {error}"))
}

/// `bench`: the measurements of the report, on a snapshot written by `generate`.
fn bench(snapshot: &str, edits: usize) -> Result<(), String> {
    let bytes =
        std::fs::read(snapshot).map_err(|error| format!("cannot read {snapshot}: {error}"))?;
    let rss_before = resident_bytes();

    // Copy A: load and read.
    let start = Instant::now();
    let mut a = load(&bytes)?;
    metric("load_ms", millis(start.elapsed()), "ms");
    let memory_load_only = (resident_bytes(), wasm_linear_memory_bytes());
    let a_text = text_object(&a)?;
    let start = Instant::now();
    let content = read_content(&a, &a_text)?;
    metric("read_ms", millis(start.elapsed()), "ms");
    metric("text_fnv1a64", text_hash(&content.canonical_text()), "hash");
    let blocks = content.blocks.len();
    let marks = content.marks.len();
    drop(content);
    print_memory(
        "memory_after_load",
        rss_before,
        resident_bytes(),
        wasm_linear_memory_bytes(),
    );
    print_memory(
        "memory_load_only",
        rss_before,
        memory_load_only.0,
        memory_load_only.1,
    );
    metric("blocks_read", blocks, "count");
    metric("marks_read", marks, "count");

    // Extra, for comparison (after the memory probes, so that it does not disturb them): the plain text alone, which `text()` returns without marks or block maps.
    let start = Instant::now();
    let plain = a.text(&a_text).map_err(automerge_error)?;
    metric("read_text_ms", millis(start.elapsed()), "ms");
    drop(plain);

    // Copy B: the edit script, one change per edit.
    let mut b = load(&bytes)?;
    b.set_actor(ActorId::from(EDITOR_ACTOR.as_slice()));
    let b_text = text_object(&b)?;
    let a_heads = a.get_heads();
    let script = edit_script(EDIT_SEED, edits);
    let mut ids = Rng::new(EDIT_SEED ^ 1);
    let mut len = b.length(&b_text);
    let mut editing = Duration::ZERO;
    let mut update_bytes_sum = 0_usize;
    let mut heads = b.get_heads();
    for edit in &script {
        let resolved = edit.resolve(len);
        let start = Instant::now();
        apply_edit(&mut b, &b_text, &resolved, &mut ids)?;
        b.commit();
        editing += start.elapsed();
        // Not timed: the update this edit would send to other replicas.
        update_bytes_sum += b.save_after(&heads).len();
        heads = b.get_heads();
        len = Edit::new_len(&resolved, len);
    }
    metric("local_edits_ms", millis(editing), "ms");
    metric("edits", edits, "count");
    if let Some(average) = update_bytes_sum.checked_div(edits) {
        metric("update_bytes_per_edit_avg", average, "bytes");
    }
    let b_len = b.length(&b_text);
    if b_len != len {
        return Err(format!(
            "after the edits the document has {b_len} positions, the edit script expects {len}"
        ));
    }

    // One update with every edit, applied to copy A.
    let update = b.save_after(&a_heads);
    metric("update_bytes", update.len(), "bytes");
    let changes: Vec<ChangeHash> = b.get_changes(&a_heads).iter().map(Change::hash).collect();
    metric("update_changes", changes.len(), "count");
    let bundle = b.bundle(changes).map_err(automerge_error)?;
    metric("update_bundle_bytes", bundle.bytes().len(), "bytes");
    let start = Instant::now();
    a.load_incremental(&update).map_err(automerge_error)?;
    metric("apply_update_ms", millis(start.elapsed()), "ms");

    // Both copies must now agree.
    let start = Instant::now();
    let a_content = read_content(&a, &a_text)?;
    let read_after_update = start.elapsed();
    let b_content = read_content(&b, &b_text)?;
    let a_hash = text_hash(&a_content.canonical_text());
    let b_hash = text_hash(&b_content.canonical_text());
    if a_hash != b_hash {
        return Err(format!(
            "the copies differ after the update: {a_hash} and {b_hash}"
        ));
    }
    metric("replicas_equal", true, "bool");
    metric("replicas_content_equal", a_content == b_content, "bool");
    let mut a_final_heads = a.get_heads();
    let mut b_final_heads = b.get_heads();
    a_final_heads.sort();
    b_final_heads.sort();
    metric("heads_equal", a_final_heads == b_final_heads, "bool");
    metric("read_after_update_ms", millis(read_after_update), "ms");
    metric("final_text_fnv1a64", a_hash, "hash");
    metric("final_sequence_len", a.length(&a_text), "count");
    metric("final_blocks", a_content.blocks.len(), "count");
    metric("final_marks", a_content.marks.len(), "count");

    if let Some(peak) = peak_resident_bytes() {
        metric("peak_rss", peak, "bytes");
    }
    if let Some(memory) = wasm_linear_memory_bytes() {
        metric("wasm_linear_memory_end", memory, "bytes");
    }

    // Later updates of the same size, as for Loro (`apply_later_update_ms_avg`): the first update after loading may pay for work that later ones do not.
    let mut later = Duration::ZERO;
    for round in 1..=LATER_UPDATES {
        let before = b.get_heads();
        let mut len = b.length(&b_text);
        for edit in &edit_script(EDIT_SEED ^ (u64::from(round) << 8), edits) {
            let resolved = edit.resolve(len);
            apply_edit(&mut b, &b_text, &resolved, &mut ids)?;
            b.commit();
            len = Edit::new_len(&resolved, len);
        }
        let update = b.save_after(&before);
        let start = Instant::now();
        a.load_incremental(&update).map_err(automerge_error)?;
        later += start.elapsed();
    }
    metric(
        "apply_later_update_ms_avg",
        millis(later / LATER_UPDATES),
        "ms",
    );

    // Warm keystrokes: one remote keystroke at a time, applied to a copy that is already up to date, as for Loro (`apply_keystroke_ms_avg`).
    let mut keystrokes = Duration::ZERO;
    for round in 0..KEYSTROKES {
        let before = b.get_heads();
        let len = b.length(&b_text);
        let at = (len / 3 + usize::try_from(round).unwrap_or(0) * 7_919) % len.max(1);
        apply_edit(
            &mut b,
            &b_text,
            &Resolved::Insert { at, text: "k" },
            &mut ids,
        )?;
        b.commit();
        let update = b.save_after(&before);
        let start = Instant::now();
        a.load_incremental(&update).map_err(automerge_error)?;
        keystrokes += start.elapsed();
    }
    metric(
        "apply_keystroke_ms_avg",
        millis(keystrokes / KEYSTROKES),
        "ms",
    );

    // Extra, after the measurements above: what one remote keystroke costs. A fresh copy (C) receives only the first edit's change. Applying an update walks every operation of the text object, so this is the fixed cost that every update pays, however small.
    let first_change = b
        .get_changes(&a_heads)
        .first()
        .map(|change| change.raw_bytes().to_vec())
        .ok_or_else(|| "the edit script made no change".to_owned())?;
    drop((a, b, a_content, b_content, update, bundle));
    let mut c = load(&bytes)?;
    let start = Instant::now();
    c.load_incremental(&first_change).map_err(automerge_error)?;
    metric("apply_first_change_ms", millis(start.elapsed()), "ms");
    metric("first_change_bytes", first_change.len(), "bytes");
    Ok(())
}

/// How many later updates of the edit script's size the benchmark applies after the first (as in the Loro benchmark).
const LATER_UPDATES: u32 = 3;

/// How many single keystrokes the benchmark applies to an up-to-date copy.
const KEYSTROKES: u32 = 10;

/// Applies one resolved edit to the text object (without committing).
fn apply_edit(
    doc: &mut AutoCommit,
    text: &ObjId,
    edit: &Resolved<'_>,
    ids: &mut Rng,
) -> Result<(), String> {
    match edit {
        Resolved::Insert { at, text: typed } => doc
            .splice_text(text, *at, 0, typed)
            .map_err(automerge_error),
        Resolved::Delete { range } => {
            if range.is_empty() {
                return Ok(());
            }
            // A plain deletion also removes block markers in the range, which merges their paragraphs (what `join_block` does).
            let count = isize::try_from(range.len())
                .map_err(|_| format!("cannot delete {} positions", range.len()))?;
            doc.splice_text(text, range.start, count, "")
                .map_err(automerge_error)
        }
        Resolved::Bold { range } => {
            add_mark(doc, text, BOLD, range.clone(), ExpandMark::After).map(|_| ())
        }
        Resolved::Split { at } => insert_block(doc, text, *at, ids),
    }
}

/// Prints how much memory the loaded document takes: the growth of the resident set since `before` (native), and the WebAssembly linear memory (WebAssembly).
fn print_memory(name: &str, before: Option<u64>, after: Option<u64>, linear: Option<u64>) {
    if let (Some(before), Some(after)) = (before, after) {
        metric(
            &format!("{name}_rss_delta"),
            after.saturating_sub(before),
            "bytes",
        );
    }
    if let Some(linear) = linear {
        metric(&format!("{name}_wasm_linear"), linear, "bytes");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_small_document_round_trips_and_merges() {
        let mut built = build(&TEN_PAGES).unwrap();
        assert_eq!(built.doc.length(&built.text), built.sequence_len);
        let snapshot = built.doc.save();

        let a = load(&snapshot).unwrap();
        let text = text_object(&a).unwrap();
        let content = check_reads_agree(&a, &text).unwrap();
        assert_eq!(content.canonical_text(), built.canonical_text);
        assert_eq!(content.blocks.len(), TEN_PAGES.paragraphs);
        assert_eq!(content.marks.len(), built.marks);
        assert!(content.blocks.iter().all(|block| block.kind == PARAGRAPH));

        // Two hundred edits on a second copy, one change each, merged back as one update.
        let mut a = a;
        let mut b = load(&snapshot).unwrap();
        b.set_actor(ActorId::from(EDITOR_ACTOR.as_slice()));
        let b_text = text_object(&b).unwrap();
        let heads = a.get_heads();
        let mut len = b.length(&b_text);
        let mut ids = Rng::new(1);
        for edit in edit_script(EDIT_SEED, 200) {
            let resolved = edit.resolve(len);
            apply_edit(&mut b, &b_text, &resolved, &mut ids).unwrap();
            b.commit();
            len = Edit::new_len(&resolved, len);
            assert_eq!(b.length(&b_text), len);
        }
        assert_eq!(b.get_changes(&heads).len(), 200);
        a.load_incremental(&b.save_after(&heads)).unwrap();
        assert_eq!(
            check_reads_agree(&a, &text).unwrap(),
            check_reads_agree(&b, &b_text).unwrap()
        );
    }

    #[test]
    fn positions_count_scalar_values_and_block_markers() {
        let mut doc = AutoCommit::new_with_encoding(ENCODING);
        let text = doc.put_object(ROOT, TEXT_KEY, ObjType::Text).unwrap();
        let mut ids = Rng::new(3);
        insert_block(&mut doc, &text, 0, &mut ids).unwrap();
        // Two Arabic letters (two bytes each in UTF-8) and one Latin letter.
        doc.splice_text(&text, 1, 0, "\u{0643}\u{062A}a").unwrap();
        insert_block(&mut doc, &text, 3, &mut ids).unwrap();
        assert_eq!(doc.length(&text), 5);
        let content = check_reads_agree(&doc, &text).unwrap();
        assert_eq!(content.canonical_text(), "\u{0643}\u{062A}\na");
        let positions: Vec<usize> = content.blocks.iter().map(|block| block.position).collect();
        assert_eq!(positions, [0, 3]);
        // Deleting a block marker merges the paragraphs.
        doc.splice_text(&text, 3, 1, "").unwrap();
        let content = check_reads_agree(&doc, &text).unwrap();
        assert_eq!(content.canonical_text(), "\u{0643}\u{062A}a");
    }

    #[test]
    fn parses_the_command_line() {
        let arguments = |line: &str| line.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            parse(&arguments("generate --out x.bin --small")),
            Ok(Command::Generate {
                out: "x.bin".to_owned(),
                small: true
            })
        );
        assert_eq!(
            parse(&arguments("bench --snapshot x.bin")),
            Ok(Command::Bench {
                snapshot: "x.bin".to_owned(),
                edits: DEFAULT_EDITS
            })
        );
        assert!(parse(&arguments("bench --edits ten")).is_err());
    }
}
