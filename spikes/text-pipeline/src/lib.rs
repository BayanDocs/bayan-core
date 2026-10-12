//! # text-pipeline
//!
//! The CORE-003 spike: a deterministic text pipeline, from a paragraph of Unicode text to the positions of its glyphs and a reference raster of its lines, run on every platform BayanDocs supports to show that it gives the same bits everywhere.
//!
//! Spike code, never part of a release (`spikes/README.md`).

#![forbid(unsafe_code)]

pub mod bench;
pub mod fonts;
pub mod hash;
pub mod itemize;
pub mod layout;
pub mod linebreak;
pub mod manifest;
pub mod png;
pub mod probes;
pub mod raster;
pub mod run;
pub mod segment;
pub mod shape;
pub mod wasm;
