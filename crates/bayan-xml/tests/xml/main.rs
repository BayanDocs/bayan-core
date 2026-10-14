//! The integration tests of bayan-xml (work package CORE-006). They read no files at run time (fixtures are compiled in), so they also run in WebAssembly.
//!
//! A failure message never prints text from a document, only positions, kinds and the names of fixtures, so that nothing copies document content into logs (`docs/AGENTS.md` §6).

#![cfg(test)]

mod common;
mod namespaces;
mod source;
mod wellformed;
