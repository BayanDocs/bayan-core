//! The harness of the fuzz targets of the engine boundary and of CRDT updates (work package CORE-010), shared by their targets in `fuzz_targets/` and tested here with `cargo +nightly-2026-10-02 test --manifest-path fuzz/Cargo.toml --lib`, which needs no fuzzing engine (README.md, "Targets").
//!
//! - [`engine`]: `engine_messages`, `engine_tile`, `engine_config` and `engine_replay`.
//! - [`crdt`]: `crdt_import`, with [`loro_panics`], which tells Loro's panics from all others.
//!
//! The other targets (bayan-opc, bayan-xml and the Fidelity Lab's readers) keep their code in their own files.

pub mod crdt;
pub mod engine;
pub mod loro_panics;

/// The JSON Schema validator of bayan-engine's tests, compiled from its file there rather than copied, so that the targets validate the engine's answers exactly as `hostile_input.rs` does.
#[path = "../../crates/bayan-engine/tests/engine/support/schema_validator.rs"]
#[allow(dead_code)]
mod schema_validator;

/// This workspace's lockfile, from which [`loro_panics::loro_packages`] reads Loro's packages.
pub const LOCKFILE: &str = include_str!("../Cargo.lock");

#[cfg(test)]
mod tests;
