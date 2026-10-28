//! Reading a run of text that is all references grows WebAssembly's memory by one copy of the run at most, however many references it holds (finding M3 of the architecture review of 2026-10-27: the reader listed every piece of a run before decoding any, and every reference is a piece of its own, so that memory grew six to twelve times the part for runs of `&lt;` and `x&#9;`).
//!
//! A test program of its own, with this one test, because WebAssembly's linear memory never shrinks and the tests of one program share one instance: an earlier test that grew memory would let this one reuse it, see no growth and pass even on code that keeps far too much. Never add a test here (see `memory/mod.rs`).
//!
//! WebAssembly only, because only there can a test measure all the memory a program holds without `unsafe` code or a dependency (`core::arch::wasm32::memory_size`). In every configuration, `keeps_one_copy_of_a_run_of_text_however_many_references_it_holds` in `src/reader.rs` measures the capacities of the reader's buffers for the same runs.
#![cfg(target_arch = "wasm32")]

mod memory;

use memory::{AT_MOST, SIZE, growth, read};

/// About `SIZE` bytes of `unit` repeated, as the text of a root element.
fn run(unit: &str) -> String {
    format!("<r>{}</r>", unit.repeat(SIZE / unit.len()))
}

/// `text` in UTF-16, little-endian, with a byte order mark.
fn utf16(text: &str) -> Vec<u8> {
    [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

#[test]
fn a_run_of_references_takes_one_copy_of_the_run_at_most() {
    for part in [
        run("&lt;").into_bytes(),
        // Two pieces every five bytes, the most a run can have.
        run("x&#9;").into_bytes(),
        run("\r\n&#9;").into_bytes(),
        run("&#x10FFFF;").into_bytes(),
        utf16(&run("&lt;")),
    ] {
        let grown = growth(|| assert_eq!(read(&part), None));
        assert!(
            grown <= AT_MOST * part.len(),
            "took {grown} bytes for a part of {} bytes",
            part.len()
        );
    }
}
