//! The import fuzzer of the brief: untrusted Loro updates and snapshots, imported through the adapter with resource limits, must never crash the engine, and whatever they leave behind must still normalize into a valid view.
//!
//! It is a structure-aware mutation fuzzer that needs no third-party code (coverage-guided fuzzing with cargo-fuzz needs libfuzzer-sys, whose licence the owner has yet to approve; see the CORE-004 report). It starts from real blobs: snapshots and incremental updates produced by random editing, including concurrent edits by a second replica. Each input is a blob mutated a few times (bit flips, interesting byte values, inserted, deleted, duplicated and spliced ranges, truncation, changed integers and header modes). Loro checks an xxHash32 checksum over each blob's body, so most inputs get a recomputed checksum, which lets them reach the decoders; some keep the wrong one, to exercise the check.
//!
//! Every input is imported into a freshly loaded document, so it depends only on the seed, the thread and its number, and any finding can be reproduced from the line the driver prints. A panic is caught, recorded with its input, and the document is leaked rather than dropped. A watchdog reports inputs that take longer than a time limit.

use std::collections::BTreeMap;
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

/// One starting blob.
struct Seed {
    /// The blob.
    bytes: Vec<u8>,
    /// The snapshot of the document to import it into.
    base: usize,
    /// Whether it is a snapshot (imported with snapshot limits).
    snapshot: bool,
}

/// What the fuzzer found.
#[derive(Default)]
struct Findings {
    executions: u64,
    imported: u64,
    refused: BTreeMap<String, u64>,
    panics: Vec<String>,
    invariant_failures: Vec<String>,
    slowest: Duration,
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

/// Builds the starting blobs: for each of a number of small documents, its snapshot at a checkpoint (the base), updates made after it by two replicas concurrently, and snapshots of the result.
fn seeds(seed: u64) -> Result<(Vec<Vec<u8>>, Vec<Seed>), String> {
    let err = |error: EditError| error.to_string();
    let mut rng = Rng::new(seed);
    let mut bases = Vec::new();
    let mut seeds = Vec::new();
    for index in 0..16_u64 {
        let mut a = Document::new(1, seed ^ index).map_err(err)?;
        for _ in 0..20 + rng.below(80) {
            random_edit(&mut a, &mut rng);
        }
        let base = a.export_snapshot().map_err(err)?;
        let mut b = Document::load(&base, 2, seed ^ index ^ 2, &ImportLimits::LOCAL_SNAPSHOT)
            .map_err(err)?;
        let checkpoint: VersionVector = a.version_vector();
        for _ in 0..10 + rng.below(60) {
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

/// Runs the fuzzer for `seconds` on `threads` threads (each with a large stack, so that only nesting deeper than the documented limits can exhaust it), writing inputs that panic, break an invariant or take too long into `out`. Prints a summary.
///
/// # Errors
///
/// A description of the findings, when there are any.
#[expect(
    clippy::print_stdout,
    reason = "the driver reports its progress and results on standard output"
)]
pub fn fuzz(seconds: u64, threads: usize, seed: u64, out: &Path) -> Result<(), String> {
    std::fs::create_dir_all(out)
        .map_err(|error| format!("cannot create {}: {error}", out.display()))?;
    // Panics are caught and recorded; the default hook would print each one.
    std::panic::set_hook(Box::new(|_| {}));
    let (bases, seeds) = seeds(seed)?;
    println!(
        "fuzz: {} starting blobs over {} base documents, {threads} threads, {seconds} s, seed {seed:#x}",
        seeds.len(),
        bases.len()
    );
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let findings = Mutex::new(Findings::default());
    let current: Vec<Mutex<Vec<u8>>> = (0..threads).map(|_| Mutex::new(Vec::new())).collect();
    let started_at: Vec<AtomicU64> = (0..threads).map(|_| AtomicU64::new(0)).collect();
    let origin = Instant::now();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for thread in 0..threads {
            let (bases, seeds, findings, current, started_at) =
                (&bases, &seeds, &findings, &current, &started_at);
            let worker = std::thread::Builder::new()
                .stack_size(64 << 20)
                .spawn_scoped(scope, move || {
                    let mut rng = Rng::new(seed ^ (u64::try_from(thread).unwrap_or(0) << 40));
                    let mut iteration: u64 = 0;
                    let mut local = Findings::default();
                    while Instant::now() < deadline {
                        let Some(start) = rng.pick(seeds) else { break };
                        let mut input = start.bytes.clone();
                        mutate(&mut rng, &mut input, seeds);
                        if !rng.chance(1, 10) {
                            fix_checksum(&mut input);
                        }
                        let limits = if start.snapshot {
                            ImportLimits {
                                max_bytes: 16 << 20,
                                ..ImportLimits::LOCAL_SNAPSHOT
                            }
                        } else {
                            ImportLimits::UPDATE
                        };
                        if let Ok(mut slot) = current[thread].lock() {
                            slot.clone_from(&input);
                        }
                        let begun = Instant::now();
                        started_at[thread].store(
                            u64::try_from(origin.elapsed().as_millis()).unwrap_or(0) + 1,
                            Ordering::Relaxed,
                        );
                        let outcome = catch_unwind(AssertUnwindSafe(|| {
                            let mut target = match Document::load(
                                &bases[start.base],
                                900,
                                900,
                                &ImportLimits::LOCAL_SNAPSHOT,
                            ) {
                                Ok(target) => target,
                                Err(error) => return (Err(format!("base: {error}")), None),
                            };
                            let result = target.import(&input, &limits);
                            let violations = match &result {
                                Ok(_) => {
                                    let violations = check_invariants(&target.view());
                                    (!violations.is_empty()).then(|| format!("{violations:?}"))
                                }
                                Err(_) => None,
                            };
                            let refused = result.err().map(|error| match error {
                                EditError::Import(ImportError::Rejected(_)) => {
                                    "rejected by Loro".to_owned()
                                }
                                EditError::Import(error) => format!("{error:?}")
                                    .split(['(', ' ', '{'])
                                    .next()
                                    .unwrap_or("")
                                    .to_owned(),
                                other => other.to_string(),
                            });
                            (refused.map_or(Ok(()), Err), violations)
                        }));
                        started_at[thread].store(0, Ordering::Relaxed);
                        let elapsed = begun.elapsed();
                        local.executions += 1;
                        if elapsed > local.slowest {
                            local.slowest = elapsed;
                        }
                        match outcome {
                            Ok((Ok(()), None)) => local.imported += 1,
                            Ok((Err(kind), None)) => *local.refused.entry(kind).or_insert(0) += 1,
                            Ok((_, Some(violations))) => {
                                let name = format!("invariant-{}", hex(&input));
                                let _ = std::fs::write(out.join(&name), &input);
                                local.invariant_failures.push(format!(
                                    "{name} (thread {thread}, input {iteration}): {violations}"
                                ));
                            }
                            Err(payload) => {
                                let message = payload
                                    .downcast_ref::<&str>()
                                    .map(|text| (*text).to_owned())
                                    .or_else(|| payload.downcast_ref::<String>().cloned())
                                    .unwrap_or_default();
                                let name = format!("panic-{}", hex(&input));
                                let _ = std::fs::write(out.join(&name), &input);
                                local.panics.push(format!(
                                    "{name} (thread {thread}, input {iteration}): {message}"
                                ));
                            }
                        }
                        iteration += 1;
                    }
                    if let Ok(mut findings) = findings.lock() {
                        findings.executions += local.executions;
                        findings.imported += local.imported;
                        for (kind, count) in local.refused {
                            *findings.refused.entry(kind).or_insert(0) += count;
                        }
                        findings.panics.extend(local.panics);
                        findings.invariant_failures.extend(local.invariant_failures);
                        if local.slowest > findings.slowest {
                            findings.slowest = local.slowest;
                        }
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
                        let input = current[thread].lock().map(|input| input.clone()).unwrap_or_default();
                        let name = format!("slow-{}", hex(&input));
                        let _ = std::fs::write(out.join(&name), &input);
                        println!("watchdog: thread {thread} has been importing {name} for more than {} s", SLOW.as_secs());
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
    let _ = std::panic::take_hook();
    let findings = findings
        .into_inner()
        .map_err(|_| "a worker panicked outside an input".to_owned())?;
    let cpu_seconds = seconds.saturating_mul(u64::try_from(threads).unwrap_or(0));
    println!(
        "summary: {} inputs in {seconds} s on {threads} threads ({cpu_seconds} CPU-seconds, {} inputs per second); {} imported (all normalized into valid views), refused by kind {:?}; {} panics, {} invariant failures; slowest input {} ms",
        findings.executions,
        findings.executions / seconds.max(1),
        findings.imported,
        findings.refused,
        findings.panics.len(),
        findings.invariant_failures.len(),
        findings.slowest.as_millis()
    );
    for line in findings.panics.iter().chain(&findings.invariant_failures) {
        println!("finding: {line}");
    }
    if findings.panics.is_empty() && findings.invariant_failures.is_empty() {
        Ok(())
    } else {
        Err("the fuzzer found problems; see the findings above".to_owned())
    }
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
