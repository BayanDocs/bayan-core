//! What the two tests of the memory that reading takes in WebAssembly share (finding M3 of the architecture review of 2026-10-27; ADR-0006 §5).
//!
//! Each of those tests is a test program of its own, `memory_runs` and `memory_declarations`, with that one test, because WebAssembly's linear memory never shrinks and the tests of one program run one after another in the same instance: memory that an earlier test grew would be reused by a later one, which would then see no growth and pass even on code that keeps far too much. Never add a test to either program; put other tests in `tests/xml/`.

use bayan_xml::{Error, Limits, Reader, Source};

/// About the size of the crafted parts: large enough that memory kept for every piece of a run or every field of a declaration shows far above the bound, small enough to read quickly in a debug build and in WebAssembly.
pub const SIZE: usize = 1 << 20;

/// The most memory that reading a crafted part may take, in bytes for every byte of the part: the bound the tests hold the reader to. The parts' text is ASCII. Reading keeps one copy of a run of text, never longer than the run itself, so at most the part's size; a UTF-16 part adds its decoded text, for which `Source::new` reserves as many bytes as the part has, while the run's copy then takes half as many, one and a half times the part in all. Memory kept for every piece of a run or every field of a declaration goes far beyond this.
#[cfg(target_arch = "wasm32")]
pub const AT_MOST: usize = 2;

/// Reads `part` to its end or to its first error, and returns the error, if there is one.
pub fn read(part: &[u8]) -> Option<Error> {
    let limits = Limits::default();
    let source = match Source::new(part, &limits) {
        Ok(source) => source,
        Err(error) => return Some(error),
    };
    let mut reader = Reader::new(&source, &limits);
    loop {
        match reader.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => return None,
            Err(error) => return Some(error),
        }
    }
}

/// How much WebAssembly's linear memory grows while `work` runs, in bytes. Linear memory never shrinks, so this is the most memory that `work` held at once, besides what it found free. A test measures several parts one after another, and each frees memory that the next could reuse, so the free memory is filled first with blocks of 64 KiB: all that can be left free is pieces smaller than a block, too small for a large buffer, and the rest of the last page.
#[cfg(target_arch = "wasm32")]
pub fn growth(work: impl FnOnce()) -> usize {
    use core::arch::wasm32::memory_size;
    const PAGE: usize = 64 << 10;
    let mut ballast = Vec::new();
    let start = memory_size::<0>();
    while memory_size::<0>() == start {
        ballast.push(vec![0_u8; PAGE]);
    }
    let before = memory_size::<0>();
    work();
    let grown = memory_size::<0>() - before;
    drop(ballast);
    grown * PAGE
}
