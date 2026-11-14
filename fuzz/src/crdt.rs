//! The harness of `crdt_import` (work package CORE-010): CRDT updates and snapshots from other people, imported through bayan-crdt's [`Doc::import`] with both of its presets (ADR-0008, decision 4).
//!
//! **An input** is one selector byte and a blob. Bit 0 of the selector chooses the limits, [`ImportLimits::UPDATE`] (an update from another replica, values inspected) or [`ImportLimits::LOCAL_SNAPSHOT`] (a snapshot this device wrote itself); the other bits choose the document the blob is imported into, a fresh copy of one of [`Corpus::bases`]: a new, empty document, the base document of CORE-004's crafted update (`crates/bayan-crdt/tests/fixtures/loro-panic-base.bin`), or one of the documents that [`Corpus::new`] makes by editing. The replica that imports holds what a live replica holds: handles to its stories, registries and maps, and an undo manager, which subscribes to the document.
//!
//! **Seeds** come from real editing, made here rather than written by `ci.py`, because only Loro can write its blobs: [`Corpus::new`] edits each document as replica 1 (text in several scripts, atoms, marks of the document model's families, stories, registries with property maps and movable lists, undo and redo), takes its snapshot as a base, loads it as replica 2, lets both edit concurrently, and keeps the updates each made since the base, the whole history as one update, the merged snapshot and a shallow snapshot. [`mutate`], libFuzzer's custom mutator for this target, starts from them (an input too short to be a blob is replaced by one) and mutates them with the CORE-004 spike's structure-aware mutations, ported from `spikes/crdt-model/src/fuzz.rs` (which stays as it is): bit flips, interesting bytes, inserted, deleted, duplicated and spliced ranges, truncation, rewritten integers and header modes, and, now and then, libFuzzer's own mutations, which use the comparisons the code makes; nine blobs in ten then get the checksum that Loro checks (xxHash32 of the body), so that they reach the decoders.
//!
//! **Properties** ([`import_case`]): no panic in BayanDocs code. A panic of Loro's may be contained by `import`, which the owner accepted (ADR-0008, amendment of 2026-10-08): the hook of [`install_panic_hook`] lets only Loro's panics unwind, and counts them ([`loro_panics`]), and `import` must then report [`ImportError::Panicked`], after which the document is poisoned and it and every handle to it, those obtained before the import included, behave as `doc.rs` documents. Every other result must be consistent with the limits, and only [`ImportError::Panicked`] and [`ImportError::ValueTooDeep`] may poison the document. Anything else fails the run: any other panic, an abort, running out of memory or time.

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use bayan_crdt::{
    CrdtError, Doc, DocStats, Expand, IdList, ImportError, ImportLimits, MarkFamily, PeerId,
    PropertyMap, Registry, Stories, Story, UndoManager, Value, Version, VersionVector,
};

use crate::loro_panics::{self, Lockfile, Owner, Package};

/// The mark families of the document model (`crates/bayan-model/src/marks.rs`): run properties continue at the end of a run, links, comments and revisions never expand.
pub const FAMILIES: [MarkFamily; 4] = [
    MarkFamily {
        name: "r",
        expand: Expand::After,
    },
    MarkFamily {
        name: "link",
        expand: Expand::None,
    },
    MarkFamily {
        name: "cmt",
        expand: Expand::None,
    },
    MarkFamily {
        name: "rev",
        expand: Expand::None,
    },
];

/// The replica that imports: none of the replicas that wrote the seeds.
const PEER: PeerId = PeerId(9);

/// The seed of the xxHash32 checksum that Loro writes into every blob: the bytes "LORO" read as a little-endian number.
const LORO_SEED: u32 = u32::from_le_bytes(*b"LORO");

/// Where a blob's body starts: after its magic, its other header fields and its checksum (bytes 16 to 19), and its two mode bytes (20 and 21).
const BODY: usize = 22;

/// The base document and update of CORE-004's crafted blob, seeds of this target: a document made by random editing, the update with which Loro 1.16.2 panicked, and its valid original.
const PANIC_BASE: &[u8] =
    include_bytes!("../../crates/bayan-crdt/tests/fixtures/loro-panic-base.bin");
const PANIC_UPDATE: &[u8] =
    include_bytes!("../../crates/bayan-crdt/tests/fixtures/loro-panic-update.bin");
const PANIC_CRAFTED: &[u8] =
    include_bytes!("../../crates/bayan-crdt/tests/fixtures/loro-panic-crafted.bin");

/// The index in [`Corpus::bases`] of CORE-004's base document.
const PANIC_BASE_INDEX: u8 = 1;

/// How many documents [`Corpus::new`] makes by editing.
const EDITED: u64 = 6;

/// Panics of Loro's that the hook let unwind, since the process started.
static LORO_PANICS: AtomicU64 = AtomicU64::new(0);

/// The number of Loro's panics so far (see [`install_panic_hook`]).
pub fn loro_panics() -> u64 {
    LORO_PANICS.load(Ordering::SeqCst)
}

/// Replaces the panic hook that is installed, libfuzzer-sys's in the target, with one that lets a panic unwind only when it is Loro's (see [`crate::loro_panics`]), counting it and printing nothing, and hands every other panic to the hook it replaces, after printing why it is not Loro's and the whole backtrace. libfuzzer-sys's hook prints the panic with Rust's default hook and aborts, which fails the run. Everything goes to standard error, which libFuzzer's log keeps, and `ci.py` never shows (README.md, "When a run fails").
///
/// # Errors
///
/// When the lockfile compiled into the target cannot be read, or does not name Loro's packages; the target then stops before it fuzzes anything.
pub fn install_panic_hook() -> Result<(), String> {
    let lockfile = Lockfile::parse(crate::LOCKFILE)?;
    let loro = loro_panics::loro_packages(&lockfile, "bayan-fuzz")?;
    install_panic_hook_for(lockfile, loro);
    Ok(())
}

/// The hook of [`install_panic_hook`], which lets the panics of the packages `loro` unwind: Loro's packages, or, in this crate's tests, a package that stands in for them, because no input may make Loro panic (`tests.rs`).
pub(crate) fn install_panic_hook_for(lockfile: Lockfile, loro: BTreeSet<Package>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let owner = loro_panics::frames(&format!("{backtrace:?}"))
            .map(|frames| loro_panics::owner(&frames, &lockfile, &loro));
        match owner {
            Ok(Owner::Loro(_)) => {
                LORO_PANICS.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Owner::NotLoro(reason)) | Err(reason) => {
                eprintln!(
                    "crdt_import: a panic that is not Loro's fails the run ({reason}). The backtrace at the panic:\n{backtrace:#}"
                );
                previous(info);
                // libfuzzer-sys's hook aborts by itself; this is in case another hook was installed before.
                std::process::abort();
            }
        }
    }));
}

/// A small deterministic pseudo-random generator (xorshift64*), so that the seeds are the same on every run and the mutator follows libFuzzer's seed.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number below `bound` (0 for a bound of 0).
    pub fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        usize::try_from(self.next_u64() % u64::try_from(bound).unwrap_or(u64::MAX)).unwrap_or(0)
    }

    /// True `numerator` times in `denominator`.
    pub fn chance(&mut self, numerator: usize, denominator: usize) -> bool {
        self.below(denominator) < numerator
    }

    pub fn byte(&mut self) -> u8 {
        self.next_u64().to_le_bytes()[0]
    }
}

/// xxHash32, the checksum Loro puts into every blob's header, ported from the CORE-004 spike so that mutated blobs can carry the right one.
pub fn xxh32(input: &[u8], seed: u32) -> u32 {
    const PRIME_1: u32 = 0x9E37_79B1;
    const PRIME_2: u32 = 0x85EB_CA77;
    const PRIME_3: u32 = 0xC2B2_AE3D;
    const PRIME_4: u32 = 0x27D4_EB2F;
    const PRIME_5: u32 = 0x1656_67B1;
    let read =
        |at: usize| u32::from_le_bytes([input[at], input[at + 1], input[at + 2], input[at + 3]]);
    let round = |accumulator: u32, lane: u32| {
        accumulator
            .wrapping_add(lane.wrapping_mul(PRIME_2))
            .rotate_left(13)
            .wrapping_mul(PRIME_1)
    };
    let len = input.len();
    let mut at = 0;
    let mut hash = if len >= 16 {
        let mut lanes = [
            seed.wrapping_add(PRIME_1).wrapping_add(PRIME_2),
            seed.wrapping_add(PRIME_2),
            seed,
            seed.wrapping_sub(PRIME_1),
        ];
        while at + 16 <= len {
            for (index, lane) in lanes.iter_mut().enumerate() {
                *lane = round(*lane, read(at + 4 * index));
            }
            at += 16;
        }
        lanes[0]
            .rotate_left(1)
            .wrapping_add(lanes[1].rotate_left(7))
            .wrapping_add(lanes[2].rotate_left(12))
            .wrapping_add(lanes[3].rotate_left(18))
    } else {
        seed.wrapping_add(PRIME_5)
    };
    // The total length modulo 2^32.
    hash = hash
        .wrapping_add(u32::try_from(u64::try_from(len).unwrap_or(0) & 0xFFFF_FFFF).unwrap_or(0));
    while at + 4 <= len {
        hash = hash
            .wrapping_add(read(at).wrapping_mul(PRIME_3))
            .rotate_left(17)
            .wrapping_mul(PRIME_4);
        at += 4;
    }
    while at < len {
        hash = hash
            .wrapping_add(u32::from(input[at]).wrapping_mul(PRIME_5))
            .rotate_left(11)
            .wrapping_mul(PRIME_1);
        at += 1;
    }
    hash ^= hash >> 15;
    hash = hash.wrapping_mul(PRIME_2);
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(PRIME_3);
    hash ^ (hash >> 16)
}

/// Writes the checksum that a blob's body (everything after byte 20) should have into its header (bytes 16 to 19).
pub fn fix_checksum(blob: &mut [u8]) {
    if blob.len() >= BODY {
        let checksum = xxh32(&blob[20..], LORO_SEED);
        blob[16..20].copy_from_slice(&checksum.to_le_bytes());
    }
}

/// The selector byte of an input: the base document's index and the limits.
pub fn selector(base: u8, snapshot_limits: bool) -> u8 {
    base << 1 | u8::from(snapshot_limits)
}

/// The documents that inputs are imported into, and the seeds.
#[derive(Debug)]
pub struct Corpus {
    /// Snapshots of the documents that inputs are imported into, by index; `None` is a new, empty document.
    pub bases: Vec<Option<Vec<u8>>>,
    /// Inputs (a selector byte and a blob) from real editing.
    pub seeds: Vec<Vec<u8>>,
}

/// The corpus, made once per process.
pub fn corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(Corpus::new)
}

impl Corpus {
    /// Makes the bases and the seeds (see the module's documentation). Panics if the adapter refuses an edit, which would be a bug of this harness or of the adapter.
    pub fn new() -> Self {
        let mut bases = vec![None, Some(PANIC_BASE.to_vec())];
        let mut seeds = vec![
            [&[selector(PANIC_BASE_INDEX, false)], PANIC_UPDATE].concat(),
            [&[selector(PANIC_BASE_INDEX, false)], PANIC_CRAFTED].concat(),
            [&[selector(0, true)], PANIC_BASE].concat(),
        ];
        for number in 0..EDITED {
            let mut rng = Rng::new(0xC0DE_0010_0000_0000 | number);
            let a = Doc::new(PeerId(1), &FAMILIES).expect("replica 1");
            let mut undo_a = UndoManager::new(&a);
            for _ in 0..10 + rng.below(40) {
                edit(&a, &mut undo_a, &mut rng);
            }
            let base = a.export_snapshot().expect("the base's snapshot");
            let b = Doc::load(&base, PeerId(2), &FAMILIES, &ImportLimits::LOCAL_SNAPSHOT)
                .expect("replica 2");
            let mut undo_b = UndoManager::new(&b);
            let checkpoint = a.version_vector();
            let index = u8::try_from(bases.len()).expect("fewer than 128 bases");
            for _ in 0..5 + rng.below(25) {
                edit(&a, &mut undo_a, &mut rng);
                edit(&b, &mut undo_b, &mut rng);
            }
            let from_a = a.export_updates(&checkpoint).expect("replica 1's updates");
            let from_b = b.export_updates(&checkpoint).expect("replica 2's updates");
            seeds.push([&[selector(index, false)], from_a.as_slice()].concat());
            seeds.push([&[selector(index, false)], from_b.as_slice()].concat());
            seeds.push(
                [
                    &[selector(0, false)],
                    a.export_updates(&VersionVector::new())
                        .expect("the whole history")
                        .as_slice(),
                ]
                .concat(),
            );
            a.import(&from_b, &ImportLimits::UPDATE)
                .expect("replica 2's updates merge");
            seeds.push(
                [
                    &[selector(index, true)],
                    a.export_snapshot().expect("the merged snapshot").as_slice(),
                ]
                .concat(),
            );
            seeds.push(
                [
                    &[selector(0, true)],
                    a.export_shallow_snapshot(&a.version())
                        .expect("a shallow snapshot")
                        .as_slice(),
                ]
                .concat(),
            );
            // An update offered with the snapshot limits, and a snapshot with the update limits, which refuse it.
            seeds.push([&[selector(index, true)], from_b.as_slice()].concat());
            seeds.push([&[selector(index, false)], base.as_slice()].concat());
            bases.push(Some(base));
        }
        Self { bases, seeds }
    }
}

impl Default for Corpus {
    fn default() -> Self {
        Self::new()
    }
}

/// Text in several scripts and with characters that need care: combining marks, right-to-left text, CJK, an emoji outside the Basic Multilingual Plane, a soft hyphen and a no-break space.
const TEXTS: [&str; 10] = [
    "Hello",
    " world",
    "Grüße",
    "שלום",
    "مرحبا",
    "日本語",
    "😀",
    "e\u{301}",
    "\u{AD}\u{A0}",
    "\u{200F}x",
];

/// One random edit of the kinds a document model makes, then, now and then, a commit (an undo step or not), an undo or a redo.
fn edit(doc: &Doc, undo: &mut UndoManager, rng: &mut Rng) {
    let story = if rng.chance(1, 4) {
        let id = format!("s{}", rng.below(3));
        doc.stories().get(&id).map_or_else(
            || doc.stories().create(&id).expect("a story"),
            |story| story,
        )
    } else {
        doc.main_story()
    };
    let len = story.len();
    match rng.below(9) {
        0 | 1 => story
            .insert(rng.below(len + 1), TEXTS[rng.below(TEXTS.len())])
            .expect("insert"),
        2 if len > 0 => {
            let start = rng.below(len);
            story
                .delete(start..start + 1 + rng.below((len - start).min(8)))
                .expect("delete");
        }
        3 if len > 0 => {
            let start = rng.below(len);
            let end = start + 1 + rng.below(len - start);
            let (key, value) = match rng.below(6) {
                0 => ("r:b".to_owned(), Value::Bool(true)),
                1 => (
                    "r:sz".to_owned(),
                    Value::Int(i64::try_from(rng.below(96)).unwrap_or(0) + 8),
                ),
                2 => ("r:color".to_owned(), Value::Str("2F5496".to_owned())),
                3 => (
                    "link".to_owned(),
                    Value::Str("https://example.com/".to_owned()),
                ),
                4 => (format!("cmt:{}", rng.below(4)), Value::Str("c".to_owned())),
                _ => (
                    "rev:ins".to_owned(),
                    Value::Map(
                        [
                            ("author".to_owned(), Value::Str("A".to_owned())),
                            ("id".to_owned(), Value::Int(7)),
                        ]
                        .into(),
                    ),
                ),
            };
            story.mark(start..end, &key, &value).expect("mark");
        }
        4 if len > 0 => {
            let start = rng.below(len);
            story
                .unmark(
                    start..start + 1 + rng.below(len - start),
                    ["r:b", "link", "rev:ins"][rng.below(3)],
                )
                .expect("unmark");
        }
        5 => story
            .insert_atom(
                rng.below(len + 1),
                ['\u{0D}', '\u{FFFC}', '\u{0C}'][rng.below(3)],
                &format!("p:{}", rng.below(16)),
            )
            .expect("an atom"),
        6 => {
            let registry = doc.registry("paragraphs").expect("the registry");
            let id = format!("p{}", rng.below(8));
            let entry = registry
                .get(&id)
                .map_or_else(|| registry.create(&id).expect("an entry"), |entry| entry);
            let value = match rng.below(5) {
                0 => Value::Str(["Heading1", "Normal", "ListParagraph"][rng.below(3)].to_owned()),
                1 => Value::Int(i64::try_from(rng.below(14_400)).unwrap_or(0) - 720),
                2 => Value::List(vec![
                    Value::Int(720),
                    Value::Int(1440),
                    Value::Str("dot".to_owned()),
                ]),
                3 => Value::Map(
                    [
                        ("val".to_owned(), Value::Bool(rng.chance(1, 2))),
                        ("w".to_owned(), Value::Bytes(vec![0, 1, 254])),
                    ]
                    .into(),
                ),
                _ => Value::Null,
            };
            entry
                .set(
                    ["style", "ind.left", "tabs", "rPr", "keepNext"][rng.below(5)],
                    &value,
                )
                .expect("a property");
        }
        7 => {
            let registry = doc.registry("tables").expect("the registry");
            let table = registry
                .get("t0")
                .map_or_else(|| registry.create("t0").expect("a table"), |table| table);
            let rows = table.id_list("rows").expect("the rows");
            let count = rows.len();
            match rng.below(3) {
                0 => rows
                    .insert(rng.below(count + 1), &format!("r{}", rng.below(32)))
                    .expect("a row"),
                1 if count > 0 => rows.delete(rng.below(count)).expect("delete a row"),
                _ if count > 1 => rows
                    .move_item(rng.below(count), rng.below(count))
                    .expect("move a row"),
                _ => {}
            }
        }
        _ => {
            let body = doc.root_map("body").expect("the body");
            body.set(
                ["pgSz.w", "pgSz.h", "cols"][rng.below(3)],
                &Value::Int(i64::try_from(rng.below(20_000)).unwrap_or(0)),
            )
            .expect("a body property");
        }
    }
    match rng.below(8) {
        0..=3 => doc.commit(),
        4 => doc.commit_without_undo(),
        5 => {
            doc.commit();
            undo.undo().expect("undo");
        }
        6 => {
            doc.commit();
            undo.redo().expect("redo");
        }
        _ => {}
    }
}

/// Applies the CORE-004 spike's mutations, one to four, to a blob (`spikes/crdt-model/src/fuzz.rs`, `mutate`), with `splice` blobs to splice from.
pub fn mutate_blob(rng: &mut Rng, blob: &mut Vec<u8>, splice: &[Vec<u8>]) {
    const INTERESTING: [u8; 8] = [0x00, 0x01, 0x3F, 0x40, 0x7F, 0x80, 0xFE, 0xFF];
    for _ in 0..1 + rng.below(4) {
        let len = blob.len();
        if len <= BODY + 1 {
            blob.push(rng.byte());
            continue;
        }
        // Mostly the body; the header (magic, checksum, mode) now and then.
        let at = if rng.chance(1, 16) {
            rng.below(BODY)
        } else {
            BODY + rng.below(len - BODY)
        };
        match rng.below(10) {
            0 => blob[at] ^= 1 << rng.below(8),
            1 => blob[at] = INTERESTING[rng.below(INTERESTING.len())],
            2 => {
                let bytes: Vec<u8> = (0..1 + rng.below(8)).map(|_| rng.byte()).collect();
                blob.splice(at..at, bytes);
            }
            3 => {
                let end = (at + 1 + rng.below(16)).min(len);
                blob.drain(at..end);
            }
            4 => {
                let end = (at + 1 + rng.below(32)).min(len);
                let copy = blob[at..end].to_vec();
                let to = BODY + rng.below(blob.len() - BODY);
                blob.splice(to..to, copy);
            }
            5 => {
                if !splice.is_empty() {
                    let other = &splice[rng.below(splice.len())];
                    let from = other
                        .len()
                        .min(BODY + rng.below(other.len().saturating_sub(BODY).max(1)));
                    blob.truncate(at);
                    blob.extend_from_slice(&other[from..]);
                }
            }
            6 => blob.truncate(at.max(BODY)),
            7 => blob[at] = blob[at].wrapping_add(if rng.chance(1, 2) { 1 } else { 255 }),
            8 => {
                if at + 4 <= len {
                    let value = rng.next_u64().to_le_bytes();
                    blob[at..at + 4].copy_from_slice(&value[..4]);
                }
            }
            _ => {
                // The two mode bytes after the checksum.
                blob[20] = u8::try_from(rng.below(6)).unwrap_or(0);
                blob[21] = 0;
            }
        }
    }
}

/// libFuzzer's custom mutator for `crdt_import` (see the module's documentation): mutates `data[..size]` in place into at most `max_size` bytes and returns the new size. `libfuzzer` is libFuzzer's own mutator (`libfuzzer_sys::fuzzer_mutate`), passed in so that this crate's tests need no fuzzing engine.
pub fn mutate(
    data: &mut [u8],
    size: usize,
    max_size: usize,
    seed: u32,
    libfuzzer: &mut dyn FnMut(&mut [u8], usize, usize) -> usize,
) -> usize {
    let corpus = corpus();
    let mut rng = Rng::new(u64::from(seed));
    let max_size = max_size.min(data.len());
    let mut input = data[..size.min(max_size)].to_vec();
    if input.len() <= BODY || rng.chance(1, 64) {
        // Too short to be a blob: start from real editing again.
        input.clone_from(&corpus.seeds[rng.below(corpus.seeds.len())]);
    }
    let (selector_byte, blob) = input
        .split_first()
        .map_or((0, Vec::new()), |(&first, rest)| (first, rest.to_vec()));
    let mut selector_byte = selector_byte;
    let mut blob = blob;
    if rng.chance(1, 16) {
        selector_byte = rng.byte();
    }
    if rng.chance(1, 4) {
        // libFuzzer's own mutations of the blob, which use the values the code compares with.
        let room = max_size.saturating_sub(1).max(blob.len());
        let len = blob.len();
        blob.resize(room, 0);
        let new_len = libfuzzer(&mut blob, len, room);
        blob.truncate(new_len);
    } else {
        let blobs: Vec<Vec<u8>> = corpus.seeds.iter().map(|seed| seed[1..].to_vec()).collect();
        mutate_blob(&mut rng, &mut blob, &blobs);
    }
    if !rng.chance(1, 10) {
        fix_checksum(&mut blob);
    }
    let out = [&[selector_byte], blob.as_slice()].concat();
    let len = out.len().min(max_size);
    data[..len].copy_from_slice(&out[..len]);
    len
}

/// What a live replica holds besides the document: handles to its stories, registries and maps, and an undo manager, which subscribes to the document. A poisoned document must make them all refuse, also those obtained before the import.
struct Handles {
    story: Story,
    stories: Stories,
    registry: Option<Registry>,
    entry: Option<PropertyMap>,
    rows: Option<IdList>,
    body: Option<PropertyMap>,
    undo: UndoManager,
}

impl Handles {
    /// Obtains them without changing the document.
    fn new(doc: &Doc) -> Self {
        let registry = doc.registry("tables").ok();
        let entry = registry.as_ref().and_then(|registry| registry.get("t0"));
        let rows = entry.as_ref().and_then(|entry| entry.get_id_list("rows"));
        Self {
            story: doc.main_story(),
            stories: doc.stories(),
            registry,
            entry,
            rows,
            body: doc.root_map("body").ok(),
            undo: UndoManager::new(doc),
        }
    }
}

thread_local! {
    /// The poisoned documents and their handles, kept so that LeakSanitizer finds them reachable: the adapter never frees a poisoned document (`Doc`'s `Drop` keeps a reference to it forever, and so does an undo manager), because freeing it can panic again. libFuzzer runs every input on this thread. Never dropped (`ManuallyDrop`), so nothing of them is freed when the process ends either.
    static POISONED: ManuallyDrop<RefCell<Vec<Box<dyn Any>>>> = ManuallyDrop::new(RefCell::new(Vec::new()));
}

/// Imports one input (see the module's documentation) and checks its properties; panics, which fails the run, when one does not hold.
pub fn import_case(data: &[u8]) {
    let Some((&selector_byte, blob)) = data.split_first() else {
        return;
    };
    let corpus = corpus();
    let limits = if selector_byte & 1 == 0 {
        ImportLimits::UPDATE
    } else {
        ImportLimits::LOCAL_SNAPSHOT
    };
    let base = &corpus.bases[usize::from(selector_byte >> 1) % corpus.bases.len()];
    let doc = match base {
        None => Doc::new(PEER, &FAMILIES),
        Some(snapshot) => Doc::load(snapshot, PEER, &FAMILIES, &ImportLimits::LOCAL_SNAPSHOT),
    }
    .expect("a base document loads");
    let handles = Handles::new(&doc);
    let before = loro_panics();
    let result = doc.import(blob, &limits);
    let contained = loro_panics() - before;
    check_result(&doc, blob, &limits, &result, contained);
    if doc.is_poisoned() {
        let fresh = check_poisoned(&doc, &handles);
        POISONED.with(|kept| kept.borrow_mut().push(Box::new((doc, handles, fresh))));
    }
}

/// The properties of an import's result: consistent with the limits and with the panics the hook let through, and the document poisoned exactly after a contained panic or a value nested too deep.
fn check_result(
    doc: &Doc,
    blob: &[u8],
    limits: &ImportLimits,
    result: &Result<bayan_crdt::ImportReport, ImportError>,
    contained: u64,
) {
    let poisoning = matches!(
        result,
        Err(ImportError::Panicked | ImportError::ValueTooDeep)
    );
    assert_eq!(doc.is_poisoned(), poisoning, "poisoned after {result:?}");
    assert_eq!(
        contained > 0,
        matches!(result, Err(ImportError::Panicked)),
        "{contained} contained panics of Loro's, but the import gave {result:?}"
    );
    match result {
        Ok(report) => {
            assert!(blob.len() <= limits.max_bytes);
            assert!(
                report.changes <= limits.max_changes && report.ops <= limits.max_ops,
                "{report:?}"
            );
        }
        Err(ImportError::TooLarge { bytes, limit }) => {
            assert!(*bytes == blob.len() && *limit == limits.max_bytes && bytes > limit)
        }
        Err(ImportError::SnapshotNotAccepted) => assert!(!limits.accept_snapshots),
        Err(ImportError::TooManyChanges { changes, limit }) => {
            assert!(*limit == limits.max_changes && changes > limit)
        }
        Err(ImportError::TooManyOps { ops, limit }) => {
            assert!(*limit == limits.max_ops && ops > limit)
        }
        Err(ImportError::ValueTooDeep) => assert!(limits.inspect_values),
        Err(ImportError::Malformed(_) | ImportError::Rejected(_) | ImportError::Panicked) => {}
        Err(ImportError::Poisoned) => panic!("a fresh document was already poisoned"),
    }
}

/// A poisoned document and every handle to it refuse, or answer as for an empty document, without calling Loro, whose locks a panic left poisoned (`doc.rs`); a call that reached Loro would panic outside the containment and fail the run. Returns the undo manager it made, which is kept with the document.
fn check_poisoned(doc: &Doc, handles: &Handles) -> UndoManager {
    static EMPTY: OnceLock<Vec<u8>> = OnceLock::new();
    let empty = EMPTY.get_or_init(|| {
        Doc::new(PeerId(10), &FAMILIES)
            .expect("an empty document")
            .version()
            .encode()
    });
    assert_eq!(
        doc.import(b"", &ImportLimits::UPDATE),
        Err(ImportError::Poisoned)
    );
    assert_eq!(doc.export_snapshot(), Err(CrdtError::Poisoned));
    assert_eq!(
        doc.export_updates(&VersionVector::new()),
        Err(CrdtError::Poisoned)
    );
    assert_eq!(
        doc.export_shallow_snapshot(&doc.version()),
        Err(CrdtError::Poisoned)
    );
    assert_eq!(&doc.version().encode(), empty);
    assert_eq!(doc.version_vector(), VersionVector::new());
    assert_eq!(doc.stats(), DocStats { ops: 0, changes: 0 });
    assert!(
        doc.checkout(&Version::decode(empty).expect("the empty version"))
            .is_err()
    );
    doc.checkout_latest();
    doc.commit();
    doc.commit_without_undo();
    assert!(!doc.is_detached() && !doc.has_pending_changes());
    assert_eq!(doc.registry("tables").err(), Some(CrdtError::Poisoned));
    assert_eq!(doc.root_map("body").err(), Some(CrdtError::Poisoned));
    for story in [&handles.story, &doc.main_story()] {
        assert!(
            story.is_empty()
                && story.text().is_empty()
                && story.runs().is_empty()
                && story.char_at(0).is_none()
        );
        assert_eq!(story.insert(0, "x"), Err(CrdtError::Poisoned));
        assert_eq!(
            story.insert_atom(0, '\u{0D}', "p:0"),
            Err(CrdtError::Poisoned)
        );
        assert_eq!(
            story.mark(0..0, "r:b", &Value::Bool(true)),
            Err(CrdtError::Poisoned)
        );
        assert_eq!(story.unmark(0..0, "r:b"), Err(CrdtError::Poisoned));
        assert_eq!(story.delete(0..0), Err(CrdtError::Poisoned));
        assert_eq!(story.runs_in(0..0), Err(CrdtError::Poisoned));
    }
    for stories in [&handles.stories, &doc.stories()] {
        assert!(
            stories.ids().is_empty() && stories.entries().is_empty() && stories.get("s0").is_none()
        );
        assert!(stories.create("s9").is_err());
    }
    if let Some(registry) = &handles.registry {
        assert!(
            registry.ids().is_empty()
                && registry.entries().is_empty()
                && registry.is_empty()
                && registry.get("t0").is_none()
        );
        assert!(registry.create("t9").is_err());
    }
    for map in [&handles.entry, &handles.body].into_iter().flatten() {
        assert!(
            map.get("rows").is_none()
                && map.entries().is_empty()
                && map.get_id_list("rows").is_none()
        );
        assert_eq!(map.set("k", &Value::Int(1)), Err(CrdtError::Poisoned));
        assert_eq!(map.remove("k"), Err(CrdtError::Poisoned));
        assert!(map.id_list("rows").is_err());
    }
    if let Some(rows) = &handles.rows {
        assert!(rows.is_empty() && rows.items().is_empty());
        assert_eq!(rows.insert(0, "r"), Err(CrdtError::Poisoned));
        assert_eq!(rows.delete(0), Err(CrdtError::Poisoned));
        assert_eq!(rows.move_item(0, 0), Err(CrdtError::Poisoned));
    }
    assert!(!handles.undo.can_undo() && !handles.undo.can_redo() && handles.undo.undo_count() == 0);
    let mut fresh = UndoManager::new(doc);
    assert_eq!(fresh.undo(), Err(CrdtError::Poisoned));
    assert_eq!(fresh.redo(), Err(CrdtError::Poisoned));
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_checksum_is_loro_s() {
        // Every blob Loro writes carries the checksum fix_checksum computes.
        for seed in &corpus().seeds {
            let mut blob = seed[1..].to_vec();
            let written = blob[16..20].to_vec();
            fix_checksum(&mut blob);
            assert_eq!(blob[16..20], written[..]);
        }
        // Reference values of xxHash32 (seed 0) for the empty input and for "abc".
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
    }

    #[test]
    fn the_seeds_are_real_blobs_of_every_kind_for_every_base() {
        let corpus = corpus();
        assert_eq!(corpus.bases.len(), 2 + usize::try_from(EDITED).unwrap());
        assert_eq!(corpus.seeds.len(), 3 + 7 * usize::try_from(EDITED).unwrap());
        for seed in &corpus.seeds {
            assert!(seed.len() > 1 + BODY, "{} bytes", seed.len());
            assert_eq!(&seed[1..5], b"loro", "a Loro blob starts with its magic");
        }
        // Every seed but CORE-004's crafted one imports into its base (or is refused for the limits it is offered with), and none panics: Loro 1.17.1 refuses the crafted one as malformed, where 1.16.2 panicked.
        for seed in &corpus.seeds {
            let doc = Doc::new(PeerId(11), &FAMILIES).unwrap();
            let base = &corpus.bases[usize::from(seed[0] >> 1) % corpus.bases.len()];
            if let Some(base) = base {
                doc.import(base, &ImportLimits::LOCAL_SNAPSHOT).unwrap();
            }
            let limits = if seed[0] & 1 == 0 {
                ImportLimits::UPDATE
            } else {
                ImportLimits::LOCAL_SNAPSHOT
            };
            let result = doc.import(&seed[1..], &limits);
            if seed[1..] == *PANIC_CRAFTED {
                assert!(
                    matches!(result, Err(ImportError::Malformed(_))),
                    "{result:?}"
                );
                continue;
            }
            match result {
                Ok(_) | Err(ImportError::SnapshotNotAccepted) => {}
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn the_seeds_pass() {
        // The target's own checks (`import_case`, `check_result`) on every seed, so that they run in CI while `crdt_import` is parked. No seed panics in Loro or poisons its document, so `check_poisoned` runs only when the target is fuzzed.
        for seed in &corpus().seeds {
            import_case(seed);
        }
    }

    #[test]
    fn the_seeds_are_the_same_every_time() {
        let again = Corpus::new();
        assert_eq!(again.bases, corpus().bases);
        assert_eq!(again.seeds, corpus().seeds);
    }

    #[test]
    fn the_mutator_keeps_within_its_size_and_starts_from_seeds() {
        let mut libfuzzer = |data: &mut [u8], size: usize, _max: usize| {
            if size > 0 {
                data[0] ^= 1;
            }
            size
        };
        let mut buffer = vec![0; 4096];
        // From nothing, it makes a real blob.
        let len = mutate(&mut buffer, 0, 4096, 7, &mut libfuzzer);
        assert!(len > 1 + BODY);
        assert_eq!(&buffer[2..5], b"oro");
        for seed in 0..2_000 {
            let len = mutate(&mut buffer, len.min(4096), 512, seed, &mut libfuzzer);
            assert!(len <= 512);
        }
    }
}
