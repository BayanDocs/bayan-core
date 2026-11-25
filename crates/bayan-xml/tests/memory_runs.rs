//! Reading a run of text that is all references grows WebAssembly's memory by one copy of the run at most, however many references it holds (finding M3 of the architecture review of 2026-10-27: the reader listed every piece of a run before decoding any, and every reference is a piece of its own, so that memory grew six to twelve times the part for runs of `&lt;` and `x&#9;`). A UTF-16 part adds its decoded text and no more: exactly the room that the text takes in UTF-8, also for characters that take three bytes there and two in UTF-16 (finding L14 of the architecture review of 2026-11-20: `Source::new` gave the text as many bytes as the part had, and the text doubled that room as it grew).
//!
//! A test program of its own, with this one test, because WebAssembly's linear memory never shrinks and the tests of one program share one instance: an earlier test that grew memory would let this one reuse it, see no growth and pass even on code that keeps far too much. Never add a test here (see `memory/mod.rs`).
//!
//! WebAssembly only, because only there can a test measure all the memory a program holds without `unsafe` code or a dependency (`core::arch::wasm32::memory_size`), and because WebAssembly's memory grows for all the room a buffer reserves, where an operating system counts only the pages written. In every configuration, `keeps_one_copy_of_a_run_of_text_however_many_references_it_holds` in `src/reader.rs` measures the capacities of the reader's buffers for the same runs, and `decodes_utf16_into_exactly_the_room_the_text_needs` in `src/source.rs` that of the decoded text.
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
    // The largest decoded text: characters that take three bytes in UTF-8 and two in UTF-16, after a reference that makes the reader copy the run. The decoded text and the run's copy each take one and a half times the part, three times the part together, and a quarter more is left for WebAssembly's pages of 64 KiB; a text that doubled its room as it grew took three and a half times the part. The part is about four times `SIZE`, so that the rounding to pages counts for less.
    let wide = utf16(&format!("<r>&lt;{}</r>", "\u{4E2D}".repeat(2 * SIZE)));
    let grown = growth(|| assert_eq!(read(&wide), None));
    assert!(
        grown * 4 <= wide.len() * 13,
        "took {grown} bytes for a UTF-16 part of {} bytes",
        wide.len()
    );
}
