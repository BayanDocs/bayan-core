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

#[test]
fn replicas_converge_to_identical_valid_views() {
    let runs = setting("BAYAN_CONVERGENCE_RUNS", 6);
    let config = Config {
        replicas: 3,
        operations: setting("BAYAN_CONVERGENCE_OPERATIONS", 400),
        check_every: 100,
    };
    let mut repairs = [0_usize; 7];
    for index in 0..runs {
        let seed = 0xC0DE_0004_0000_0000 + u64::try_from(index).unwrap_or(0);
        let actions = plan(seed, &config);
        match run(seed, &actions, &config) {
            Ok(stats) => {
                for (total, count) in repairs.iter_mut().zip(stats.repairs) {
                    *total += count;
                }
                assert!(
                    stats.applied > config.operations / 3,
                    "seed {seed:#x}: only {} operations applied",
                    stats.applied
                );
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
    }
    // Concurrent edits did produce states that needed repairing, so the views were not trivially valid.
    assert!(
        repairs.iter().sum::<usize>() > 0,
        "no run needed any normalization: {repairs:?}"
    );
}
