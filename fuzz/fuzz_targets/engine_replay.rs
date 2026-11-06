//! Recordings that `diag.replay` reads (engine protocol §10, §13): `recording::replay` with a fixed configuration, its test switches off. A recording is refused with a documented error or replayed into a consistent report, the same both times. The harness is `bayan_fuzz::engine::replay` (`fuzz/src/engine.rs`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| bayan_fuzz::engine::replay(data));
