//! The engine's protocol messages (engine protocol §4 to §8, §13): an engine with its test switches off, after a valid handshake, pastes from blob 5 the bytes that the input starts with, after their length as a little-endian 16-bit number, and handles the rest of the input split into messages at each line feed. Every answer must be a valid protocol message, no handler may panic, not even where the engine would catch the panic, and a valid request still succeeds afterwards. The harness is `bayan_fuzz::engine::messages` (`fuzz/src/engine.rs`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| bayan_fuzz::engine::messages(data));
