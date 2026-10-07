//! The integration tests of bayan-engine: the engine driven through its public interface, the way the shells and the bindings drive it.
//!
//! They form one test crate with modules, so the helpers in `support` are compiled once, and `cfg(test)` on the crate tells Clippy that all of it is test code, where `unwrap()` is fine.

#![cfg(test)]

mod hostile_input;
mod panic_recovery;
mod protocol_schema;
mod record_replay;
mod session;
mod support;
