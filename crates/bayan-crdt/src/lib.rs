//! # bayan-crdt
//!
//! The adapter between BayanDocs and its CRDT library.
//!
//! Collaborative editing stores documents in a CRDT (a data structure that lets several people edit at once and always merges to the same result). This crate will be the only one that talks to the CRDT library (Loro, [ADR-0008]); it exposes BayanDocs-shaped primitives such as story sequences with marks, property maps and movable row lists, so the library can be replaced without touching the rest of the engine.
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-004].
//!
//! ## Layer
//!
//! bayan-crdt belongs to the **Model and formats** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-crdt may therefore depend only on Model and formats crates, on Foundation crates and on external libraries.
//!
//! [CORE-004]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [ADR-0008]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0008-crdt-engine-and-local-first.md

#![forbid(unsafe_code)]
