//! The engine's protocol messages (engine protocol §4 to §8, §13): an engine with its test switches off, after a valid handshake, handles the input split into messages at each line feed. Every answer must be a valid protocol message, no handler may panic, not even where the engine would catch the panic, and a valid request still succeeds afterwards. The harness is `bayan_fuzz::engine::messages` (`fuzz/src/engine.rs`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| bayan_fuzz::engine::messages(data));
