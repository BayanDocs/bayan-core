//! The integration tests of bayan-lab: the scanner, the tagger, the stores and the `bayan-lab corpus` commands, driven through their public interfaces, plus the checks of the committed public corpus.
//!
//! They form one test crate with modules, so the helpers in `support` are compiled once, and `cfg(test)` on the crate tells Clippy that all of it is test code, where `unwrap()` is fine.

#![cfg(test)]

mod cli;
mod hostile_packages;
mod public_corpus;
mod s3_store;
mod support;
mod tagging;
