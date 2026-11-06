//! CRDT updates and snapshots from other people (ADR-0008, decision 4): bayan-crdt's `Doc::import` with both of its presets, into a replica that holds handles and an undo manager. No panic in BayanDocs code; a panic of Loro's that `import` contains poisons the document, which then behaves as `doc.rs` documents; anything else fails. The panic hook, the seeds from real editing and the structure-aware mutator ported from the CORE-004 spike are in `bayan_fuzz::crdt` (`fuzz/src/crdt.rs`).

#![no_main]

use libfuzzer_sys::{fuzz_mutator, fuzz_target, fuzzer_mutate};

fuzz_target!(
    init: {
        // After libfuzzer-sys's own hook, which this one hands every panic to that is not Loro's.
        if let Err(problem) = bayan_fuzz::crdt::install_panic_hook() {
            panic!("crdt_import cannot tell Loro's panics from others: {problem}");
        }
        // The seeds, made once before fuzzing starts.
        bayan_fuzz::crdt::corpus();
    },
    |data: &[u8]| bayan_fuzz::crdt::import_case(data)
);

fuzz_mutator!(|data: &mut [u8], size: usize, max_size: usize, seed: u32| {
    bayan_fuzz::crdt::mutate(data, size, max_size, seed, &mut fuzzer_mutate)
});
