//! The request of `bayan_render_tile` (engine protocol §3.1, §6.4): `Engine::render_tile` with the request and a size taken from the input, on an engine with the mock document open, now and then while recording. A rendered tile has its size, a refused request is refused as documented, a recorded call replays identically, and the engine stays usable. The harness is `bayan_fuzz::engine::tile` (`fuzz/src/engine.rs`).

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| bayan_fuzz::engine::tile(data));
