//! The import fuzzer of the brief: untrusted Loro updates and snapshots, imported through the adapter with resource limits, must never crash the engine, and whatever they leave behind must still normalize into a valid view.
//!
//! It is a structure-aware mutation fuzzer that needs no third-party code (coverage-guided fuzzing with cargo-fuzz needs libfuzzer-sys, whose licence the owner has yet to approve; see the CORE-004 report). It starts from real blobs: snapshots and incremental updates produced by random editing, including concurrent edits by a second replica. Each input is a blob mutated a few times (bit flips, interesting byte values, inserted, deleted, duplicated and spliced ranges, truncation, changed integers and header modes). Loro checks an xxHash32 checksum over each blob's body, so most inputs get a recomputed checksum, which lets them reach the decoders; some keep the wrong one, to exercise the check.
//!
//! Every input is imported into a freshly loaded copy of its base document, so it depends only on the seed, the thread and its number. A panic is recorded with its source location (inputs are grouped by location), its input is written to the output folder under a name that says which base document it belongs to, and the document is leaked rather than dropped. Panics that the adapter contains (`ImportError::Panicked`) are reported but are not failures; any other panic, and any view that breaks an invariant, is. A watchdog reports inputs that take longer than a time limit, and each thread writes its current input to disk before importing it, so that even an input that aborts the process is kept. `minimize` shrinks a finding into a minimal reproduction.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bayan_crdt::{ImportError, ImportLimits, VersionVector};
use bayan_model::simulation::{Rng, random_edit};
use bayan_model::{Document, EditError, check_invariants};

/// The seed of Loro's xxHash32 checksum: the bytes "LORO" read as a little-endian number.
const LORO_SEED: u32 = u32::from_le_bytes(*b"LORO");

/// How long one input may take before the watchdog reports it.
const SLOW: Duration = Duration::from_secs(10);

/// The stage of an input during which the adapter contained a panic.
const CONTAINED: &str = "import, contained by the adapter";

/// One starting blob.
struct Seed {
    /// The blob.
    bytes: Vec<u8>,
    /// The snapshot of the document to import it into.
    base: usize,
    /// Whether it is a snapshot (imported with snapshot limits).
    snapshot: bool,
}

/// One input: a blob to import into a base document.
#[derive(Clone)]
struct Case {
    /// Whether the base documents are the small ones (see [`seeds`]).
    small: bool,
    /// The base document (an index into the bases that [`seeds`] builds).
    base: usize,
    /// Whether the blob is imported as a snapshot, with snapshot limits, rather than as an update.
    snapshot: bool,
    /// The starting blob it was mutated from (an index into the seeds that [`seeds`] builds).
    seed: usize,
    /// The blob.
    bytes: Vec<u8>,
}

impl Case {
    /// The file name of a finding: its kind, its base document (`b` and its number, or `t` for a small one), `s` for a snapshot or `u` for an update, its starting blob, and a hash of its bytes.
    fn file_name(&self, kind: &str) -> String {
        format!(
            "{kind}-{}{}-{}-s{}-{}",
            if self.small { "t" } else { "b" },
            self.base,
            if self.snapshot { "s" } else { "u" },
            self.seed,
            hex(&self.bytes)
        )
    }

    /// Reads the base document, the kind of blob and the starting blob back from a name written by [`Case::file_name`].
    fn from_file_name(name: &str, bytes: Vec<u8>) -> Option<Self> {
        let mut parts = name.split('-').skip(1);
        let base = parts.next()?;
        let (small, base) = match base.strip_prefix('t') {
            Some(number) => (true, number),
            None => (false, base.strip_prefix('b')?),
        };
        let snapshot = match parts.next()? {
            "s" => true,
            "u" => false,
            _ => return None,
        };
        let seed = parts.next()?.strip_prefix('s')?.parse().ok()?;
        Some(Self {
            small,
            base: base.parse().ok()?,
            snapshot,
            seed,
            bytes,
        })
    }

    /// The limits an engine applies to such a blob from someone else: an update as received from another replica, and a snapshot as received when a document is shared, with its values inspected (only this device's own snapshots are trusted enough to skip that).
    fn limits(&self) -> ImportLimits {
        if self.snapshot {
            ImportLimits {
                max_bytes: 16 << 20,
                inspect_values: true,
                ..ImportLimits::LOCAL_SNAPSHOT
            }
        } else {
            ImportLimits::UPDATE
        }
    }
}

/// What importing one input did.
enum Outcome {
    /// Imported, and the document still normalizes into a valid view.
    Imported,
    /// Refused, with the kind of refusal.
    Refused(String),
    /// Imported, but the view breaks invariants: a bug in the model rather than in the CRDT.
    Invariants(String),
    /// Something panicked.
    Panicked(Panic),
}

/// A panic, as the panic hook saw it.
struct Panic {
    /// When it happened: loading the base, importing ([`CONTAINED`] when the adapter caught it), reading the result, or freeing it.
    stage: &'static str,
    /// Its source location, `file:line`, with the file relative to the crate registry.
    location: String,
    /// Its message, shortened. Loro's messages can contain document text (harmless here, the documents being synthetic), which is one more reason why the engine must never pass panic messages on.
    message: String,
}

/// All inputs that panicked at one location.
struct Site {
    count: u64,
    stage: &'static str,
    message: String,
    /// The file holding the first such input.
    example: String,
}

/// What the fuzzer found.
#[derive(Default)]
struct Findings {
    executions: u64,
    imported: u64,
    refused: BTreeMap<String, u64>,
    /// By source location.
    panics: BTreeMap<String, Site>,
    invariant_failures: Vec<String>,
    slowest: Duration,
}

impl Findings {
    fn merge(&mut self, other: Self) {
        self.executions += other.executions;
        self.imported += other.imported;
        for (kind, count) in other.refused {
            *self.refused.entry(kind).or_insert(0) += count;
        }
        for (location, site) in other.panics {
            match self.panics.get_mut(&location) {
                Some(known) => known.count += site.count,
                None => {
                    self.panics.insert(location, site);
                }
            }
        }
        self.invariant_failures.extend(other.invariant_failures);
        if other.slowest > self.slowest {
            self.slowest = other.slowest;
        }
    }
}

thread_local! {
    /// The location and message of the latest panic on this thread, recorded by the hook that [`with_panic_hook`] installs.
    static LAST_PANIC: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
}

/// Runs `work` with a panic hook that records each panic's location and message for [`run_case`] instead of printing them.
fn with_panic_hook<T>(work: impl FnOnce() -> T) -> T {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map_or_else(
            || "unknown".to_owned(),
            |at| format!("{}:{}", registry_relative(at.file()), at.line()),
        );
        let message: String = info
            .payload_as_str()
            .unwrap_or_default()
            .chars()
            .take(240)
            .collect();
        LAST_PANIC.with(|last| *last.borrow_mut() = Some((location, message)));
    }));
    let result = work();
    drop(std::panic::take_hook());
    result
}

/// Shortens a source path in the crate registry to the crate and the file, such as `loro-internal-1.16.2/src/oplog.rs`.
fn registry_relative(file: &str) -> &str {
    file.split_once("index.crates.io")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map_or(file, |(_, relative)| relative)
}

/// Runs `work` on a thread with a stack of `stack_mib` MiB, as the fuzzing workers do, so that a replay meets the stack limit the finding met. A thread that could not be started, or that ended with a panic `work` did not contain, counts as a panic outside the import.
fn on_stack(stack_mib: usize, work: impl FnOnce() -> Outcome + Send) -> Outcome {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(stack_mib << 20)
            .spawn_scoped(scope, work)
            .ok()
            .and_then(|thread| thread.join().ok())
            .unwrap_or(Outcome::Panicked(Panic {
                stage: "thread",
                location: String::new(),
                message: String::new(),
            }))
    })
}

/// Imports `case` into a freshly loaded copy of its base document and checks the result.
fn run_case(bases: &[Vec<u8>], case: &Case) -> Outcome {
    LAST_PANIC.with(|last| last.borrow_mut().take());
    let stage = Cell::new("load");
    let result = catch_unwind(AssertUnwindSafe(|| {
        let Some(base) = bases.get(case.base) else {
            return Outcome::Refused("no such base document".to_owned());
        };
        // Not dropped when a panic unwinds: after a panic inside Loro, freeing the document can panic again, and a panic during unwinding aborts the process. The document is leaked instead.
        let mut target = match Document::load(base, 900, 900, &ImportLimits::LOCAL_SNAPSHOT) {
            Ok(target) => ManuallyDrop::new(target),
            Err(error) => return Outcome::Refused(format!("base: {error}")),
        };
        stage.set("import");
        let imported = target.import(&case.bytes, &case.limits());
        stage.set("read");
        let outcome = match imported {
            Ok(_) => {
                let violations = check_invariants(&target.view());
                if violations.is_empty() {
                    Outcome::Imported
                } else {
                    Outcome::Invariants(format!("{violations:?}"))
                }
            }
            Err(EditError::Import(ImportError::Panicked)) => Outcome::Panicked(Panic {
                stage: CONTAINED,
                location: String::new(),
                message: String::new(),
            }),
            Err(error) => Outcome::Refused(refusal_kind(&error)),
        };
        stage.set("drop");
        drop(ManuallyDrop::into_inner(target));
        outcome
    }));
    let (location, message) = LAST_PANIC
        .with(|last| last.borrow_mut().take())
        .unwrap_or_default();
    match result {
        Ok(Outcome::Panicked(panic)) => Outcome::Panicked(Panic {
            location,
            message,
            ..panic
        }),
        Ok(outcome) => outcome,
        Err(_) => Outcome::Panicked(Panic {
            stage: stage.get(),
            location,
            message,
        }),
    }
}

fn refusal_kind(error: &EditError) -> String {
    match error {
        EditError::Import(ImportError::Rejected(_)) => "rejected by Loro".to_owned(),
        EditError::Import(error) => format!("{error:?}")
            .split(['(', ' ', '{'])
            .next()
            .unwrap_or("")
            .to_owned(),
        other => other.to_string(),
    }
}

/// xxHash32 (the non-cryptographic checksum Loro puts in every blob's header), written out so that the fuzzer can recompute it after mutating a blob.
#[must_use]
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
    // The total length modulo 2^32, also where `usize` is 32 bits wide.
    let len_low_bits = u64::try_from(len).unwrap_or(0) & 0xFFFF_FFFF;
    hash = hash.wrapping_add(u32::try_from(len_low_bits).unwrap_or(0));
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

/// Writes the checksum a blob's body (everything after byte 20) should have into its header (bytes 16 to 19).
pub fn fix_checksum(blob: &mut [u8]) {
    if blob.len() >= 22 {
        let checksum = xxh32(&blob[20..], LORO_SEED);
        blob[16..20].copy_from_slice(&checksum.to_le_bytes());
    }
}

/// Builds the starting blobs: for each of a number of documents, its snapshot at a checkpoint (the base), updates made after it by two replicas concurrently, and snapshots of the result. `small` makes the documents tiny (a few edits each), so that findings make short reproductions.
fn seeds(seed: u64, small: bool) -> Result<(Vec<Vec<u8>>, Vec<Seed>), String> {
    let err = |error: EditError| error.to_string();
    let mut rng = Rng::new(seed);
    let mut bases = Vec::new();
    let mut seeds = Vec::new();
    for index in 0..16_u64 {
        let mut a = Document::new(1, seed ^ index).map_err(err)?;
        let edits = if small {
            1 + rng.below(4)
        } else {
            20 + rng.below(80)
        };
        for _ in 0..edits {
            random_edit(&mut a, &mut rng);
        }
        let base = a.export_snapshot().map_err(err)?;
        let mut b = Document::load(&base, 2, seed ^ index ^ 2, &ImportLimits::LOCAL_SNAPSHOT)
            .map_err(err)?;
        let checkpoint: VersionVector = a.version_vector();
        let edits = if small {
            1 + rng.below(3)
        } else {
            10 + rng.below(60)
        };
        for _ in 0..edits {
            random_edit(&mut a, &mut rng);
            random_edit(&mut b, &mut rng);
        }
        let base_index = bases.len();
        bases.push(base);
        for document in [&a, &b] {
            seeds.push(Seed {
                bytes: document.export_updates(&checkpoint).map_err(err)?,
                base: base_index,
                snapshot: false,
            });
        }
        seeds.push(Seed {
            bytes: a.export_snapshot().map_err(err)?,
            base: base_index,
            snapshot: true,
        });
    }
    Ok((bases, seeds))
}

/// Applies one to four mutations to `input`.
fn mutate(rng: &mut Rng, input: &mut Vec<u8>, seeds: &[Seed]) {
    const INTERESTING: [u8; 8] = [0x00, 0x01, 0x3F, 0x40, 0x7F, 0x80, 0xFE, 0xFF];
    for _ in 0..1 + rng.below(4) {
        let len = input.len();
        if len <= 23 {
            input.push(u8::try_from(rng.below(256)).unwrap_or(0));
            continue;
        }
        // Mostly the body; the header (magic, checksum, mode) now and then.
        let at = if rng.chance(1, 16) {
            rng.below(22)
        } else {
            22 + rng.below(len - 22)
        };
        match rng.below(10) {
            0 => input[at] ^= 1 << rng.below(8),
            1 => input[at] = INTERESTING[rng.below(INTERESTING.len())],
            2 => {
                let bytes: Vec<u8> = (0..1 + rng.below(8))
                    .map(|_| u8::try_from(rng.below(256)).unwrap_or(0))
                    .collect();
                input.splice(at..at, bytes);
            }
            3 => {
                let end = (at + 1 + rng.below(16)).min(len);
                input.drain(at..end);
            }
            4 => {
                let end = (at + 1 + rng.below(32)).min(len);
                let copy = input[at..end].to_vec();
                let to = 22 + rng.below(input.len() - 22);
                input.splice(to..to, copy);
            }
            5 => {
                if let Some(other) = rng.pick(seeds) {
                    let from = other
                        .bytes
                        .len()
                        .min(22 + rng.below(other.bytes.len().saturating_sub(22).max(1)));
                    input.truncate(at);
                    input.extend_from_slice(&other.bytes[from..]);
                }
            }
            6 => input.truncate(at.max(22)),
            7 => input[at] = input[at].wrapping_add(if rng.chance(1, 2) { 1 } else { 255 }),
            8 => {
                if at + 4 <= len {
                    let value = u32::try_from(rng.next_u64() & 0xFFFF_FFFF)
                        .unwrap_or(0)
                        .to_le_bytes();
                    input[at..at + 4].copy_from_slice(&value);
                }
            }
            _ => {
                // The two mode bytes after the checksum.
                input[20] = u8::try_from(rng.below(6)).unwrap_or(0);
                input[21] = 0;
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    format!("{:016x}", crdt_workload::fnv1a64(bytes))
}

/// Runs the fuzzer for `seconds` on `threads` threads (each with a large stack, so that only nesting deeper than the documented limits can exhaust it), writing inputs that panic, break an invariant or take too long into `out`. Prints a summary, with one line per panic location.
///
/// # Errors
///
/// A description of the findings when an input broke an invariant or panicked where the adapter does not contain it.
#[expect(
    clippy::print_stdout,
    reason = "the driver reports its progress and results on standard output"
)]
pub fn fuzz(
    seconds: u64,
    threads: usize,
    seed: u64,
    small: bool,
    stream: u64,
    stack_mib: usize,
    out: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(out)
        .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
    let (bases, seeds) = seeds(seed, small)?;
    println!(
        "fuzz: {} starting blobs over {} {}base documents, {threads} threads with {stack_mib} MiB stacks, {seconds} s, seed {seed:#x}",
        seeds.len(),
        bases.len(),
        if small { "small " } else { "" }
    );
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let findings = Mutex::new(Findings::default());
    let current: Vec<Mutex<Option<Case>>> = (0..threads).map(|_| Mutex::new(None)).collect();
    let started_at: Vec<AtomicU64> = (0..threads).map(|_| AtomicU64::new(0)).collect();
    let origin = Instant::now();
    let done = AtomicBool::new(false);
    with_panic_hook(|| {
        std::thread::scope(|scope| {
            let mut workers = Vec::new();
            for thread in 0..threads {
                let (bases, seeds, findings, current, started_at) =
                    (&bases, &seeds, &findings, &current, &started_at);
                let worker = std::thread::Builder::new()
                    .stack_size(stack_mib << 20)
                    .spawn_scoped(scope, move || {
                        // The mutations depend on the seed, the thread and the stream; the base documents only on the seed.
                        let mut rng = Rng::new(
                            seed ^ (u64::try_from(thread).unwrap_or(0) << 40)
                                ^ stream.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                        );
                        let mut local = Findings::default();
                        while Instant::now() < deadline {
                            // The same draw as `rng.pick(seeds)`, keeping the index.
                            let index = rng.below(seeds.len());
                            let Some(start) = seeds.get(index) else { break };
                            let mut bytes = start.bytes.clone();
                            mutate(&mut rng, &mut bytes, seeds);
                            if !rng.chance(1, 10) {
                                fix_checksum(&mut bytes);
                            }
                            let case = Case {
                                small,
                                base: start.base,
                                snapshot: start.snapshot,
                                seed: index,
                                bytes,
                            };
                            if let Ok(mut slot) = current[thread].lock() {
                                *slot = Some(case.clone());
                            }
                            // On disk before the import, so that an input that aborts the process (which no panic handler can catch) is not lost.
                            let _ =
                                std::fs::write(out.join(format!("current-{thread}")), &case.bytes);
                            let _ = std::fs::write(
                                out.join(format!("current-{thread}.name")),
                                case.file_name("current"),
                            );
                            let begun = Instant::now();
                            started_at[thread].store(
                                u64::try_from(origin.elapsed().as_millis()).unwrap_or(0) + 1,
                                Ordering::Relaxed,
                            );
                            let outcome = run_case(bases, &case);
                            started_at[thread].store(0, Ordering::Relaxed);
                            let elapsed = begun.elapsed();
                            local.executions += 1;
                            if elapsed > local.slowest {
                                local.slowest = elapsed;
                            }
                            match outcome {
                                Outcome::Imported => local.imported += 1,
                                Outcome::Refused(kind) => {
                                    *local.refused.entry(kind).or_insert(0) += 1
                                }
                                Outcome::Invariants(violations) => {
                                    let name = case.file_name("invariant");
                                    let _ = std::fs::write(out.join(&name), &case.bytes);
                                    local
                                        .invariant_failures
                                        .push(format!("{name}: {violations}"));
                                }
                                Outcome::Panicked(panic) => {
                                    let location = panic.location.clone();
                                    let site =
                                        local.panics.entry(panic.location).or_insert_with(|| {
                                            let name = case.file_name("panic");
                                            let _ = std::fs::write(out.join(&name), &case.bytes);
                                            Site {
                                                count: 0,
                                                stage: panic.stage,
                                                message: panic.message,
                                                example: name,
                                            }
                                        });
                                    site.count += 1;
                                    append_line(
                                        &out.join(format!("panics-{thread}.log")),
                                        &format!("{location}\t{}", site.stage),
                                    );
                                }
                            }
                            if local.executions % 1_000 == 0 {
                                write_totals(&out.join(format!("totals-{thread}")), &local);
                            }
                        }
                        write_totals(&out.join(format!("totals-{thread}")), &local);
                        if let Ok(mut findings) = findings.lock() {
                            findings.merge(local);
                        }
                    });
                match worker {
                    Ok(worker) => workers.push(worker),
                    Err(error) => println!("cannot start worker {thread}: {error}"),
                }
            }
            // The watchdog: reports, once each, inputs that run longer than SLOW.
            scope.spawn(|| {
                let mut reported = vec![false; threads];
                while !done.load(Ordering::Relaxed) && Instant::now() < deadline + SLOW {
                    std::thread::sleep(Duration::from_millis(500));
                    let now = u64::try_from(origin.elapsed().as_millis()).unwrap_or(0);
                    for thread in 0..threads {
                        let begun = started_at[thread].load(Ordering::Relaxed);
                        let slow = begun != 0 && now.saturating_sub(begun) > u64::try_from(SLOW.as_millis()).unwrap_or(u64::MAX);
                        if slow && !reported[thread] {
                            reported[thread] = true;
                            if let Some(case) = current[thread].lock().ok().and_then(|case| case.clone()) {
                                let name = case.file_name("slow");
                                let _ = std::fs::write(out.join(&name), &case.bytes);
                                println!("watchdog: thread {thread} has been importing {name} for more than {} s", SLOW.as_secs());
                            }
                        } else if !slow {
                            reported[thread] = false;
                        }
                    }
                }
            });
            for worker in workers {
                let _ = worker.join();
            }
            done.store(true, Ordering::Relaxed);
        });
    });
    let findings = findings
        .into_inner()
        .map_err(|_| "a worker panicked outside an input".to_owned())?;
    let cpu_seconds = seconds.saturating_mul(u64::try_from(threads).unwrap_or(0));
    let panicked: u64 = findings.panics.values().map(|site| site.count).sum();
    let uncontained: Vec<&String> = findings
        .panics
        .iter()
        .filter(|(_, site)| site.stage != CONTAINED)
        .map(|(location, _)| location)
        .collect();
    println!(
        "summary: {} inputs in {seconds} s on {threads} threads ({cpu_seconds} CPU-seconds, {} inputs per second); {} imported (all normalized into valid views), refused by kind {:?}; {panicked} panicked at {} source locations ({} of them not contained by the adapter); {} invariant failures; slowest input {} ms",
        findings.executions,
        findings.executions / seconds.max(1),
        findings.imported,
        findings.refused,
        findings.panics.len(),
        uncontained.len(),
        findings.invariant_failures.len(),
        findings.slowest.as_millis()
    );
    for (location, site) in &findings.panics {
        println!(
            "panic site: {location}: {} inputs, during {}, first in {}: {}",
            site.count, site.stage, site.example, site.message
        );
    }
    for line in &findings.invariant_failures {
        println!("invariant failure: {line}");
    }
    if uncontained.is_empty() && findings.invariant_failures.is_empty() {
        Ok(())
    } else {
        Err("the fuzzer found problems that the adapter does not contain; see above".to_owned())
    }
}

/// Appends `line` to the file at `path` (for logs that must survive the process aborting).
fn append_line(path: &Path, line: &str) {
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Writes a thread's running totals, which [`supervise`] reads even when the process aborted.
fn write_totals(path: &Path, findings: &Findings) {
    let refused: u64 = findings.refused.values().sum();
    let panicked: u64 = findings.panics.values().map(|site| site.count).sum();
    let _ = std::fs::write(
        path,
        format!(
            "executions={} imported={} refused={refused} panicked={panicked} invariants={} slowest_ms={}",
            findings.executions,
            findings.imported,
            findings.invariant_failures.len(),
            findings.slowest.as_millis()
        ),
    );
}

/// Reads a file written by [`write_totals`]: the counts in its order.
fn read_totals(path: &Path) -> [u64; 6] {
    let mut totals = [0; 6];
    if let Ok(text) = std::fs::read_to_string(path) {
        for (slot, field) in totals.iter_mut().zip(text.split_whitespace()) {
            *slot = field
                .split_once('=')
                .and_then(|(_, value)| value.parse().ok())
                .unwrap_or(0);
        }
    }
    totals
}

/// The line of a process's error output that says why it aborted, with digits removed (for example `memory allocation of  bytes failed`). Loro prints decoding errors to the error output too; they are skipped.
fn abort_signature(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let reason = text
        .lines()
        .find(|line| {
            [
                "memory allocation of",
                "overflowed its stack",
                "non-unwinding panic",
                "fatal runtime error",
            ]
            .iter()
            .any(|pattern| line.contains(pattern))
        })
        .or_else(|| {
            text.lines().rev().find(|line| {
                !line.trim().is_empty() && !line.starts_with("Column Deserialize Error")
            })
        })
        .unwrap_or("no error output");
    reason
        .chars()
        .filter(|character| !character.is_ascii_digit())
        .collect()
}

/// Whether a child process ended normally: `fuzz` and `replay` exit with 0 to 4; anything else (a signal, or the abort code on Windows) is an abort.
fn ended_normally(status: std::process::ExitStatus) -> bool {
    matches!(status.code(), Some(0..=4))
}

/// Fuzzes for `seconds` with `workers` child processes of this program, each running `fuzz` on one thread with its own stream of mutations over the same base documents. A child that aborts the process (which happens on inputs that make Loro allocate absurd amounts of memory, and which no panic handler can catch) is restarted on a new stream; the input that aborted it is kept as `abort-…` in `out`. Prints the combined totals.
///
/// # Errors
///
/// A description of what failed, or of the findings when there are any that the adapter does not contain.
#[expect(
    clippy::print_stdout,
    reason = "the driver reports its progress and results on standard output"
)]
pub fn supervise(
    seconds: u64,
    workers: usize,
    seed: u64,
    small: bool,
    stack_mib: usize,
    out: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(out)
        .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
    let program = std::env::current_exe().map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let started = Instant::now();
    let runs: Mutex<Vec<(PathBuf, Duration)>> = Mutex::new(Vec::new());
    let aborts: Mutex<Vec<String>> = Mutex::new(Vec::new());
    println!(
        "supervise: {workers} workers with {stack_mib} MiB stacks, {seconds} s, seed {seed:#x}{}",
        if small { ", small base documents" } else { "" }
    );
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let (program, runs, aborts) = (&program, &runs, &aborts);
            scope.spawn(move || {
                let mut round = 0_u64;
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
                    if remaining < 5 {
                        break;
                    }
                    let stream = (u64::try_from(worker).unwrap_or(0) << 32) | round;
                    let directory = out.join(format!("w{worker}-r{round}"));
                    let mut command = std::process::Command::new(program);
                    command.args([
                        "fuzz",
                        "--seconds",
                        &remaining.to_string(),
                        "--threads",
                        "1",
                        "--seed",
                        &format!("{seed:#x}"),
                        "--stream",
                        &stream.to_string(),
                        "--stack",
                        &stack_mib.to_string(),
                        "--out",
                    ]);
                    command.arg(&directory);
                    if small {
                        command.arg("--small");
                    }
                    let begun = Instant::now();
                    let output = command.output();
                    let elapsed = begun.elapsed();
                    if let Ok(mut runs) = runs.lock() {
                        runs.push((directory.clone(), elapsed));
                    }
                    let output = match output {
                        Ok(output) => output,
                        Err(error) => {
                            println!("worker {worker}: cannot start the fuzzer: {error}");
                            break;
                        }
                    };
                    let _ = std::fs::write(directory.join("stdout.txt"), &output.stdout);
                    if !ended_normally(output.status) {
                        let name = std::fs::read_to_string(directory.join("current-0.name"))
                            .unwrap_or_default()
                            .replacen("current", "abort", 1);
                        let kept = out.join(&name);
                        let _ = std::fs::copy(directory.join("current-0"), &kept);
                        let signature = abort_signature(&output.stderr);
                        println!(
                            "worker {worker}: round {round} aborted after {} s ({signature}); input kept as {name}",
                            elapsed.as_secs()
                        );
                        if let Ok(mut aborts) = aborts.lock() {
                            aborts.push(format!("{name}: {signature}"));
                        }
                    }
                    round += 1;
                }
            });
        }
    });
    let runs = runs
        .into_inner()
        .map_err(|_| "a worker failed".to_owned())?;
    let aborts = aborts
        .into_inner()
        .map_err(|_| "a worker failed".to_owned())?;
    let mut totals = [0_u64; 6];
    let mut sites: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut cpu = Duration::ZERO;
    for (directory, elapsed) in &runs {
        cpu += *elapsed;
        let counts = read_totals(&directory.join("totals-0"));
        for (total, count) in totals.iter_mut().zip(&counts[..5]) {
            *total += count;
        }
        totals[5] = totals[5].max(counts[5]);
        if let Ok(log) = std::fs::read_to_string(directory.join("panics-0.log")) {
            for line in log.lines() {
                let (location, stage) = line.split_once('\t').unwrap_or((line, ""));
                let site = sites
                    .entry(location.to_owned())
                    .or_insert_with(|| (0, stage.to_owned()));
                site.0 += 1;
            }
        }
    }
    let [executions, imported, refused, panicked, invariants, slowest] = totals;
    let uncontained = sites
        .values()
        .filter(|(_, stage)| stage.as_str() != CONTAINED)
        .count();
    println!(
        "summary: {executions} inputs in {} s of fuzzing ({} rounds on {workers} workers, {} s wall clock); {imported} imported (all normalized into valid views), {refused} refused; {panicked} panicked at {} source locations ({uncontained} of them not contained by the adapter); {} aborted the process; {invariants} invariant failures; slowest input {slowest} ms (inputs of a round that aborted are counted up to its last thousand)",
        cpu.as_secs(),
        runs.len(),
        started.elapsed().as_secs(),
        sites.len(),
        aborts.len()
    );
    for (location, (count, stage)) in &sites {
        println!("panic site: {location}: {count} inputs, during {stage}");
    }
    for abort in &aborts {
        println!("abort: {abort}");
    }
    if uncontained == 0 && invariants == 0 {
        Ok(())
    } else {
        Err("the fuzzer found problems that the adapter does not contain; see above".to_owned())
    }
}

/// Imports the input in `path` (a file the fuzzer wrote) once, the way the fuzzer did, and reports what happened. Returns the exit code: 0 imported or refused, 3 panicked, 4 broke an invariant. An input that aborts the process aborts this command too, which is what [`minimize`] relies on in isolated mode.
///
/// # Errors
///
/// A description of what failed.
#[expect(
    clippy::print_stdout,
    reason = "the driver reports its results on standard output"
)]
pub fn replay(path: &Path, seed: u64, stack_mib: usize) -> Result<u8, String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "the input has no file name".to_owned())?;
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let case = Case::from_file_name(name, bytes).ok_or_else(|| {
        "the file name does not say which base document and starting blob the input belongs to"
            .to_owned()
    })?;
    let (bases, _) = seeds(seed, case.small)?;
    Ok(with_panic_hook(|| {
        match on_stack(stack_mib, || run_case(&bases, &case)) {
            Outcome::Imported => {
                println!("imported");
                0
            }
            Outcome::Refused(kind) => {
                println!("refused: {kind}");
                0
            }
            Outcome::Panicked(panic) => {
                println!("panicked at {} during {}", panic.location, panic.stage);
                3
            }
            Outcome::Invariants(violations) => {
                println!("broke invariants: {violations}");
                4
            }
        }
    }))
}

/// Describes how `input` differs from `start`, ignoring the checksum (bytes 16 to 19): the changed bytes when there are a few and the length is the same, otherwise the range that differs.
fn describe_changes(start: &[u8], input: &[u8]) -> String {
    let same = |at: usize| (16..20).contains(&at) || start.get(at) == input.get(at);
    if start.len() == input.len() {
        let changed: Vec<usize> = (0..input.len()).filter(|at| !same(*at)).collect();
        if changed.len() <= 16 {
            let list: Vec<String> = changed
                .iter()
                .map(|at| format!("byte {at}: {:#04x} -> {:#04x}", start[*at], input[*at]))
                .collect();
            return format!("{} changed bytes ({})", changed.len(), list.join(", "));
        }
    }
    let prefix = (0..start.len().min(input.len()))
        .take_while(|at| same(*at))
        .count();
    let suffix = start
        .iter()
        .rev()
        .zip(input.iter().rev())
        .take(start.len().min(input.len()) - prefix)
        .take_while(|(a, b)| a == b)
        .count();
    format!(
        "bytes {prefix}..{} of the starting blob ({} bytes) replaced by {} bytes; the input has {} bytes",
        start.len() - suffix,
        start.len() - suffix - prefix,
        input.len() - suffix - prefix,
        input.len()
    )
}

/// How an input fails, for [`minimize`]: a panic at a source location, or an abort with the first line of the process's error output (digits removed).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    Panic(String),
    Abort(String),
}

/// Shrinks the input in `path`, a file the fuzzer wrote (its name says which base document and starting blob it belongs to), into a simpler one that fails in the same way (a panic at the same source location, or the same kind of abort): first by setting changed bytes back to their value in the starting blob, then by removing ever smaller ranges of bytes (which Loro's column encoding rarely survives). With `isolated`, every attempt runs in a child process (`replay`), which is slower but also works for inputs that abort the process. Writes the result next to the input with `.min` appended, and the snapshot of its base document and the starting blob as `base-<n>.bin` and `seed-<n>.bin`: together a reproduction that needs only Loro. Prints the bytes in which the result differs from the starting blob. `seed` must be the seed of the fuzzing run.
///
/// # Errors
///
/// A description of what failed, for example when the input does not fail.
#[expect(
    clippy::print_stdout,
    reason = "the driver reports its results on standard output"
)]
pub fn minimize(path: &Path, seed: u64, isolated: bool, stack_mib: usize) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "the input has no file name".to_owned())?;
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let case = Case::from_file_name(name, bytes).ok_or_else(|| {
        "the file name does not say which base document and starting blob the input belongs to"
            .to_owned()
    })?;
    let (bases, seeds) = seeds(seed, case.small)?;
    let base = bases
        .get(case.base)
        .ok_or_else(|| format!("there is no base document {}", case.base))?;
    let start = seeds
        .get(case.seed)
        .ok_or_else(|| format!("there is no starting blob {}", case.seed))?;
    let program = std::env::current_exe().map_err(|error| error.to_string())?;
    let probe_path = path.with_file_name(format!(
        "probe-{}{}-{}-s{}-candidate",
        if case.small { "t" } else { "b" },
        case.base,
        if case.snapshot { "s" } else { "u" },
        case.seed
    ));
    let classify = |bytes: &[u8]| -> Option<Failure> {
        if isolated {
            std::fs::write(&probe_path, bytes).ok()?;
            let output = std::process::Command::new(&program)
                .args([
                    "replay",
                    "--seed",
                    &format!("{seed:#x}"),
                    "--stack",
                    &stack_mib.to_string(),
                    "--input",
                ])
                .arg(&probe_path)
                .output()
                .ok()?;
            if !ended_normally(output.status) {
                Some(Failure::Abort(abort_signature(&output.stderr)))
            } else if output.status.code() == Some(3) {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let location = stdout
                    .strip_prefix("panicked at ")
                    .and_then(|rest| rest.split_once(" during "))
                    .map(|(location, _)| location.to_owned())?;
                Some(Failure::Panic(location))
            } else {
                None
            }
        } else {
            let probe = Case {
                bytes: bytes.to_vec(),
                ..case.clone()
            };
            match on_stack(stack_mib, || run_case(&bases, &probe)) {
                Outcome::Panicked(panic) => Some(Failure::Panic(panic.location)),
                _ => None,
            }
        }
    };
    with_panic_hook(|| {
        let original = classify(&case.bytes).ok_or_else(|| {
            "the input does not fail (in isolated mode, abort or panic)".to_owned()
        })?;
        let checksum_was_fixed = {
            let mut copy = case.bytes.clone();
            fix_checksum(&mut copy);
            copy == case.bytes
        };
        let mut tests = 0_u32;
        let mut still_fails = |mut bytes: Vec<u8>| -> Option<Vec<u8>> {
            if checksum_was_fixed {
                fix_checksum(&mut bytes);
            }
            tests += 1;
            (classify(&bytes).as_ref() == Some(&original)).then_some(bytes)
        };
        let mut smallest = case.bytes.clone();
        // Bytes 16 to 19 hold the checksum, which is recomputed.
        let differs =
            |bytes: &[u8], at: usize| !(16..20).contains(&at) && bytes[at] != start.bytes[at];
        if smallest.len() == start.bytes.len() {
            let mut reverted = true;
            while reverted {
                reverted = false;
                for at in 0..smallest.len() {
                    if differs(&smallest, at) {
                        let mut candidate = smallest.clone();
                        candidate[at] = start.bytes[at];
                        if let Some(simpler) = still_fails(candidate) {
                            smallest = simpler;
                            reverted = true;
                        }
                    }
                }
            }
        }
        // The header (magic number, checksum and mode, 22 bytes) stays.
        let mut chunk = smallest.len().saturating_sub(22).max(1);
        loop {
            let mut at = 22;
            let mut removed = false;
            while at < smallest.len() {
                let end = (at + chunk).min(smallest.len());
                let mut candidate = smallest.clone();
                candidate.drain(at..end);
                if let Some(smaller) = still_fails(candidate) {
                    smallest = smaller;
                    removed = true;
                } else {
                    at = end;
                }
            }
            if !removed {
                if chunk == 1 {
                    break;
                }
                chunk = chunk.div_ceil(2);
            }
        }
        let _ = std::fs::remove_file(&probe_path);
        let changes = describe_changes(&start.bytes, &smallest);
        let mut min_path = path.as_os_str().to_owned();
        min_path.push(".min");
        std::fs::write(&min_path, &smallest)
            .map_err(|error| format!("cannot write the minimized input: {error}"))?;
        let prefix = if case.small { "t" } else { "b" };
        let base_path = path.with_file_name(format!("base-{prefix}{}.bin", case.base));
        std::fs::write(&base_path, base)
            .map_err(|error| format!("cannot write the base document: {error}"))?;
        let seed_path = path.with_file_name(format!("seed-{prefix}{}.bin", case.seed));
        std::fs::write(&seed_path, &start.bytes)
            .map_err(|error| format!("cannot write the starting blob: {error}"))?;
        println!(
            "minimized {name} in {tests} tests: {changes}, checksum {}; it still fails with {original:?}; base document {} ({} bytes), starting blob {} ({} bytes)",
            if checksum_was_fixed {
                "recomputed"
            } else {
                "kept"
            },
            base_path.display(),
            base.len(),
            seed_path.display(),
            start.bytes.len()
        );
        Ok(())
    })
}

/// Where a finding's input is written.
#[must_use]
pub fn default_output() -> PathBuf {
    PathBuf::from("target/crdt-model-fuzz")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xxh32_matches_published_vectors() {
        assert_eq!(xxh32(b"", 0), 0x02CC_5D05);
        assert_eq!(xxh32(b"a", 0), 0x550D_7456);
        assert_eq!(xxh32(b"abc", 0), 0x32D1_53FF);
        assert_eq!(
            xxh32(b"Nobody inspects the spammish repetition", 0),
            0xE229_3B2F
        );
    }

    #[test]
    fn recomputed_checksums_match_loro_blobs() {
        let document = Document::new(1, 1).expect("a document");
        let blob = document.export_snapshot().expect("a snapshot");
        let mut copy = blob.clone();
        copy[16..20].fill(0);
        fix_checksum(&mut copy);
        assert_eq!(copy, blob);
    }
}
