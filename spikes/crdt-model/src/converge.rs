//! The brief's full convergence run: many seeded runs of the simulation, in parallel.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bayan_model::simulation::{Config, Failure, REFUSAL_REASONS, Stats, plan, run, shrink};

/// The outcome of one run.
struct Outcome {
    index: usize,
    seed: u64,
    result: Result<Stats, Failure>,
    elapsed: Duration,
}

/// Runs `runs` simulations of `config`, seeds `seed`, `seed + 1`, …, on `threads` threads. Prints a line per run and a summary; returns an error if any run failed (after shrinking it).
///
/// # Errors
///
/// A summary of the failed runs.
#[expect(
    clippy::print_stdout,
    reason = "the driver reports its progress and results on standard output"
)]
pub fn converge(runs: usize, threads: usize, seed: u64, config: &Config) -> Result<(), String> {
    let started = Instant::now();
    let next = AtomicUsize::new(0);
    let outcomes: Mutex<Vec<Outcome>> = Mutex::new(Vec::with_capacity(runs));
    println!(
        "converge: {runs} runs of {} operations, {} replicas, {threads} threads, seeds from {seed:#x}",
        config.operations, config.replicas
    );
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= runs {
                        break;
                    }
                    let run_seed = seed.wrapping_add(u64::try_from(index).unwrap_or(0));
                    let actions = plan(run_seed, config);
                    let run_started = Instant::now();
                    let result = run(run_seed, &actions, config);
                    let elapsed = run_started.elapsed();
                    match &result {
                        Ok(stats) => println!(
                            "run {index} seed {run_seed:#x}: ok in {} ms (applied {}, refused {} [{}], skipped {}, targets shown {} hidden {}, delivered {}, undo/redo {}, materializations {}, repairs N1-N7 {:?} of which highlights {}, main {} atoms, view fingerprint {:016x})",
                            elapsed.as_millis(),
                            stats.applied,
                            stats.refused,
                            refusals(&stats.refusals),
                            stats.skipped,
                            stats.visible_targets,
                            stats.hidden_targets,
                            stats.delivered,
                            stats.undone,
                            stats.materializations,
                            stats.repairs,
                            stats.highlights,
                            stats.main_len,
                            stats.fingerprint
                        ),
                        Err(failure) => println!("run {index} seed {run_seed:#x}: FAILED {failure}"),
                    }
                    if let Ok(mut outcomes) = outcomes.lock() {
                        outcomes.push(Outcome {
                            index,
                            seed: run_seed,
                            result,
                            elapsed,
                        });
                    }
                }
            });
        }
    });
    let mut outcomes = outcomes
        .into_inner()
        .map_err(|_| "a worker thread panicked".to_owned())?;
    outcomes.sort_by_key(|outcome| outcome.index);
    let mut totals = Stats::default();
    let mut times: Vec<Duration> = Vec::new();
    let mut failures = Vec::new();
    for outcome in &outcomes {
        times.push(outcome.elapsed);
        match &outcome.result {
            Ok(stats) => {
                totals.applied += stats.applied;
                totals.refused += stats.refused;
                for (total, count) in totals.refusals.iter_mut().zip(stats.refusals) {
                    *total += count;
                }
                totals.skipped += stats.skipped;
                totals.visible_targets += stats.visible_targets;
                totals.hidden_targets += stats.hidden_targets;
                totals.delivered += stats.delivered;
                totals.undone += stats.undone;
                totals.materializations += stats.materializations;
                for (total, count) in totals.repairs.iter_mut().zip(stats.repairs) {
                    *total += count;
                }
                totals.highlights += stats.highlights;
            }
            Err(failure) => failures.push((outcome.seed, failure.clone())),
        }
    }
    times.sort_unstable();
    let median = times.get(times.len() / 2).copied().unwrap_or_default();
    let slowest = times.last().copied().unwrap_or_default();
    println!(
        "summary: {} of {} runs converged to identical views satisfying I1-I7, also on a replica loaded from the final snapshot, with idempotent normalization and no materialization that changed a view; {} operations applied, {} refused [{}], {} skipped; targets the view showed {}, hidden {}; {} messages delivered, {} undo/redo steps, {} materializations; repairs needed in final views N1-N7 {:?}, of which comment highlights {}; run time median {} ms, slowest {} ms; total {} s",
        outcomes.len() - failures.len(),
        outcomes.len(),
        totals.applied,
        totals.refused,
        refusals(&totals.refusals),
        totals.skipped,
        totals.visible_targets,
        totals.hidden_targets,
        totals.delivered,
        totals.undone,
        totals.materializations,
        totals.repairs,
        totals.highlights,
        median.as_millis(),
        slowest.as_millis(),
        started.elapsed().as_secs()
    );
    if failures.is_empty() {
        return Ok(());
    }
    for (failed_seed, failure) in &failures {
        let actions = plan(*failed_seed, config);
        let shrunk = shrink(*failed_seed, &actions, config, 500);
        println!(
            "failure seed {failed_seed:#x}: {failure}\n  shrunk from {} to {} actions: {shrunk:?}",
            actions.len(),
            shrunk.len()
        );
    }
    Err(format!("{} runs failed", failures.len()))
}

/// The refusal counts that are not zero, with their reasons.
fn refusals(counts: &[usize; REFUSAL_REASONS.len()]) -> String {
    REFUSAL_REASONS
        .iter()
        .zip(counts)
        .filter(|(_, count)| **count > 0)
        .map(|(reason, count)| format!("{reason} {count}"))
        .collect::<Vec<_>>()
        .join(", ")
}
