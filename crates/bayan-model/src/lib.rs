//! # bayan-model
//!
//! The Bayan Document Model.
//!
//! Documents are modelled the way Word models them internally: a set of **stories** (main text, headers and footers, notes, comments, text boxes, table cells), each a sequence of **atoms** (characters, paragraph ends, field delimiters, object anchors), with formatting as marks over ranges, tables as objects whose cells own stories, and everything BayanDocs does not understand preserved for round-trip. This crate will hold that model, its invariants and its deterministic normalization ([document model specification][model-spec]).
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-004].
//!
//! ## Layer
//!
//! bayan-model belongs to the **Model and formats** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-model may therefore depend only on Model and formats crates, on Foundation crates and on external libraries.
//!
//! [CORE-004]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [model-spec]: https://github.com/BayanDocs/docs/blob/HEAD/specs/document-model.md

#![forbid(unsafe_code)]
