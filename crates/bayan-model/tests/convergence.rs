//! Randomized convergence (CORE-004 AC-1): three replicas apply random operations (including concurrent structural edits), exchange updates in random orders with partitions, undo and redo, and must end with identical views that satisfy I1–I7, with normalization deterministic and idempotent.
//!
//! The default size suits CI. The brief's full run (1,000 runs of 10,000 operations) uses the spike's driver, `cargo run --release -p crdt-model -- converge` (spikes/crdt-model/REPORT.md); this test accepts `BAYAN_CONVERGENCE_RUNS` and `BAYAN_CONVERGENCE_OPERATIONS` for intermediate sizes. A failing run is shrunk before it is reported, with its seed.

use bayan_model::simulation::{Config, plan, run, shrink};

fn setting(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// The repairs that only concurrency can make necessary, now that local operations keep fields and ranges whole: N2, N3, N4, N6 and N7 (of the rules N1–N7 in `Stats::repairs`).
fn concurrent_repairs(repairs: &[usize; 7]) -> usize {
    repairs[1] + repairs[2] + repairs[3] + repairs[5] + repairs[6]
}

#[test]
fn replicas_converge_to_identical_valid_views() {
    let runs = setting("BAYAN_CONVERGENCE_RUNS", 6);
    let config = Config {
        replicas: 3,
        operations: setting("BAYAN_CONVERGENCE_OPERATIONS", 400),
        check_every: 100,
    };
    // The same plans on one replica alone: what local operations need repaired without any concurrency.
    let alone = Config {
        replicas: 1,
        ..config
    };
    let (mut concurrent, mut baseline) = (0_usize, 0_usize);
    for index in 0..runs {
        let seed = 0xC0DE_0004_0000_0000 + u64::try_from(index).unwrap_or(0);
        let actions = plan(seed, &config);
        match run(seed, &actions, &config) {
            Ok(stats) => {
                println!("seed {seed:#x}: {stats:?}");
                concurrent += concurrent_repairs(&stats.repairs);
                assert!(
                    stats.applied > config.operations / 3,
                    "seed {seed:#x}: only {} operations applied",
                    stats.applied
                );
                assert_eq!(stats.undo_errors, 0, "seed {seed:#x}");
            }
            Err(failure) => {
                let shrunk = shrink(seed, &actions, &config, 2_000);
                let again = run(seed, &shrunk, &config);
                panic!(
                    "seed {seed:#x}: {failure}\nshrunk to {} actions: {shrunk:#?}\nwhich fails with: {again:?}",
                    shrunk.len()
                );
            }
        }
        let single = run(seed, &plan(seed, &alone), &alone)
            .unwrap_or_else(|failure| panic!("seed {seed:#x} on one replica: {failure}"));
        baseline += concurrent_repairs(&single.repairs);
    }
    // Concurrent edits did produce states that needed repairing beyond what one replica alone needs, so the views were not trivially valid.
    println!(
        "repairs only concurrency causes (N2, N3, N4, N6, N7): {concurrent} with three replicas, {baseline} with one"
    );
    assert!(
        concurrent > baseline,
        "the concurrent runs needed no more repairs than one replica alone: {concurrent} against {baseline}"
    );
}
