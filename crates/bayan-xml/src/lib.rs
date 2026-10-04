//! # bayan-xml
//!
//! Streaming XML with Markup Compatibility processing and lossless preservation.
//!
//! This crate will provide a namespace-aware streaming XML reader that rejects document type declarations and external entities and enforces limits; Markup Compatibility processing (`mc:AlternateContent`, `mc:Ignorable`, ECMA-376 Part 3); lossless capture of fragments BayanDocs does not understand, so they can be written back unchanged; and a deterministic writer with namespace management.
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-006].
//!
//! ## Layer
//!
//! bayan-xml belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-xml may therefore depend only on other Foundation crates and on external libraries.
//!
//! [CORE-006]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-006-bayan-xml.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]
