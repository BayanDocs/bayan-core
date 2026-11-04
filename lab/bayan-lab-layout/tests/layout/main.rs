//! The integration tests of bayan-lab-layout: the extractor on synthetic PDFs whose positions are known (AC-1), right-to-left and mixed lines (AC-2), the layout JSON and its schema against samples from both sides (AC-3), PDFs of known tools, the file structures it reads, and the hostile inputs it refuses.
//!
//! They form one test crate with modules, so the helpers in `support` are compiled once, and `cfg(test)` on the crate tells Clippy that all of it is test code, where `unwrap()` is fine.

#![cfg(test)]

mod hostile;
mod known_tools;
mod positions;
mod reading_order;
mod schema;
mod structure;
mod support;
