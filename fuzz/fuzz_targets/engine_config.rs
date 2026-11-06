//! The engine's configuration (engine protocol §3.3): `Config::from_json`. An accepted configuration starts an engine that completes a handshake. The harness is `bayan_fuzz::engine::config_json` (`fuzz/src/engine.rs`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| bayan_fuzz::engine::config_json(data));
