//! The brief's full convergence run: many seeded runs of the simulation, in parallel.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bayan_model::simulation::{Config, Failure, Stats, plan, run, shrink};

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
                            "run {index} seed {run_seed:#x}: ok in {} ms (applied {}, refused {}, delivered {}, undo/redo {}, repairs N1-N7 {:?}, main {} atoms)",
                            elapsed.as_millis(),
                            stats.applied,
                            stats.refused,
                            stats.delivered,
                            stats.undone,
                            stats.repairs,
                            stats.main_len
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
                totals.delivered += stats.delivered;
                totals.undone += stats.undone;
                for (total, count) in totals.repairs.iter_mut().zip(stats.repairs) {
                    *total += count;
                }
            }
            Err(failure) => failures.push((outcome.seed, failure.clone())),
        }
    }
    times.sort_unstable();
    let median = times.get(times.len() / 2).copied().unwrap_or_default();
    let slowest = times.last().copied().unwrap_or_default();
    println!(
        "summary: {} of {} runs converged to identical views satisfying I1-I7 (deterministic, idempotent); {} operations applied, {} refused, {} messages delivered, {} undo/redo steps; repairs needed in final views N1-N7 {:?}; run time median {} ms, slowest {} ms; total {} s",
        outcomes.len() - failures.len(),
        outcomes.len(),
        totals.applied,
        totals.refused,
        totals.delivered,
        totals.undone,
        totals.repairs,
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
