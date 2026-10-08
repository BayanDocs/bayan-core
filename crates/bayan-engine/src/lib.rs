//! # bayan-engine
//!
//! The engine facade that both shells talk to.
//!
//! The desktop and web shells drive the engine through one versioned message protocol: commands, queries and input go in, events come out ([engine protocol specification][protocol-spec], [ADR-0012]). This crate implements version 0 of that protocol around a **mock document** (the private module `mock`: a fixed three-page document with an editable line), so the shells can be built and tested end to end before the engine can open real documents ([CORE-007]):
//!
//! - [`Engine`] handles one message at a time and returns the messages it sends in answer. It never opens sockets, reads files or clocks, or calls platform APIs; the host passes it everything (architecture §8).
//! - [`thread::EngineThread`] runs an [`Engine`] on its own thread for native hosts, as the desktop's C interface (bayan-ffi) needs; the WebAssembly binding (bayan-wasm) drives an [`Engine`] directly inside the web worker.
//! - [`protocol`] defines every message as Rust types, [`schema`] generates the JSON Schema from them, and [`typescript`] the TypeScript declarations for the web shell (spec §9).
//! - [`recording`] records sessions and replays them (spec §10), with [`digest`] for the digests it compares.
//! - [`blobs`], [`config`] and [`limits`] hold the bulk data, the configuration and the limits of spec §13.
//!
//! A panic while the engine handles a message is caught and reported as `engine.error`; the engine then continues with a fresh session (ADR-0006 §4, spec §12). In WebAssembly, where panics cannot be caught, the worker host does the same by replacing the engine instance.
//!
//! **Status:** the v0 skeleton of work package [CORE-007]. Opening real documents, editing and the real UI manifest arrive with later work packages.
//!
//! ## Layer
//!
//! bayan-engine belongs to the **Engine** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-engine may therefore depend on crates of every layer except Bindings and tools, and on external libraries.
//!
//! [CORE-007]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-007-engine-skeleton.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [protocol-spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md
//! [ADR-0012]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0012-engine-boundary.md

#![forbid(unsafe_code)]

mod base64;
pub mod blobs;
pub mod config;
pub mod digest;
mod engine;
mod envelope;
pub mod limits;
mod manifest;
mod mock;
pub mod protocol;
mod raster;
pub mod recording;
pub mod schema;
pub mod thread;
pub mod typescript;

pub use engine::{ENGINE_VERSION, Engine, TileError, TileOutcome};
