//! The tests of bayan-lab-compare (LAB-004). They build every layout and package they compare in code and read no files, so they also run in WebAssembly, which shows that the metrics give the same numbers there.
//!
//! They form one test crate with modules, so the helpers in `support` are compiled once, and `cfg(test)` on the crate tells Clippy that all of it is test code, where `unwrap()` is fine.

#![cfg(test)]

mod hints;
mod report;
mod summary;
mod support;
mod synthetic;
