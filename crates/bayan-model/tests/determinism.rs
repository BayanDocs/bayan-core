//! Cross-platform determinism of the model (ADR-0005; CORE-004 report §5.4): a fixed simulation run must end with exactly the outcome committed here, on every platform. The gate runs the tests natively and in WebAssembly (wasm32-wasip1), and CI runs the gate on Linux x86-64, Windows x86-64 and macOS arm64, so a pass everywhere means that every platform made the same edits, refused the same ones, and computed the same view, bit for bit.
//!
//! The fingerprint hashes the view's `Debug` text (`simulation::fingerprint`). A change to the model, to the simulation's generator or to the toolchain's `Debug` output changes it: then check that the native and the WebAssembly run print the same values, and pin them in the same commit, saying why they changed.

use bayan_model::simulation::{Config, plan, run};

/// The run: three replicas and 400 operations, with partitions, reordered delivery and undo, as in the convergence test.
const SEED: u64 = 0xC0DE_0004_0005_0000;
const CONFIG: Config = Config {
    replicas: 3,
    operations: 400,
    check_every: 0,
};

/// The outcome every platform must reach: operations applied, refused and skipped, the main story's length in the final view, and the view's fingerprint.
const EXPECTED: (usize, usize, usize, usize, &str) = (328, 46, 26, 136, "5580e1b4474b07c3");

#[test]
fn a_fixed_run_ends_with_the_same_view_on_every_platform() {
    let actions = plan(SEED, &CONFIG);
    let stats = run(SEED, &actions, &CONFIG).expect("the run converges");
    let fingerprint = format!("{:016x}", stats.fingerprint);
    let outcome = (
        stats.applied,
        stats.refused,
        stats.skipped,
        stats.main_len,
        fingerprint.as_str(),
    );
    println!("outcome {outcome:?}");
    assert_eq!(outcome, EXPECTED);
}
