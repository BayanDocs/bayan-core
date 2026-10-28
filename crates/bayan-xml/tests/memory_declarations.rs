//! An XML declaration that goes on with fields it may not have is refused at the first of them, before anything after it is read, and keeps nothing of its fields (finding M3 of the architecture review of 2026-10-27: `Source::new` collected every field before checking any, so that a declaration of empty fields kept 40 bytes for every 4 of its own, and grew WebAssembly's memory twenty times its size, before it was refused).
//!
//! A test program of its own, with this one test, because WebAssembly's linear memory never shrinks and the tests of one program share one instance: an earlier test that grew memory would let this one reuse it, see no growth and pass even on code that keeps far too much. Never add a test here (see `memory/mod.rs`).
//!
//! The memory is measured in WebAssembly only. On the host no test can measure memory that `Source::new` allocates and frees before it returns without a counting allocator, which needs `unsafe` code or a dependency, and the operating system's measures work neither on every system nor in WebAssembly. There the point of refusal stands in for it, in every configuration: these declarations never end, so a reader that collected the fields before checking them would read on to the end and report that instead, and a reader that stops at the first field it may not have cannot have kept more than three.

mod memory;

use bayan_xml::ErrorKind;
#[cfg(target_arch = "wasm32")]
use memory::{AT_MOST, growth};
use memory::{SIZE, read};

#[test]
fn a_declaration_is_refused_at_the_first_field_it_may_not_have() {
    let empty_fields = " =\"\"".repeat(SIZE / 4);
    for (start, offset) in [
        // A field without a name,
        ("<?xml version=\"1.0\"", 22),
        // one that repeats a field,
        ("<?xml version=\"1.0\" version=\"1.0\"", 29),
        // and a fourth field.
        (
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"",
            56,
        ),
    ] {
        let part = format!("{start}{empty_fields}").into_bytes();
        let error = read(&part).unwrap();
        assert_eq!(
            (error.kind(), error.offset()),
            (ErrorKind::MalformedDeclaration, offset),
            "{start}"
        );
        #[cfg(target_arch = "wasm32")]
        {
            let grown = growth(|| assert!(read(&part).is_some()));
            assert!(
                grown <= AT_MOST * part.len(),
                "took {grown} bytes for a part of {} bytes",
                part.len()
            );
        }
    }
}
