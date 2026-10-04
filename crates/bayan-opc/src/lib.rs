//! # bayan-opc
//!
//! Open Packaging Conventions: the hardened package layer under every `.docx`.
//!
//! A `.docx` file is a ZIP archive with content types and relationships (ECMA-376 Part 2). This crate will read and write such packages: hardened ZIP reading with limits on entry count, sizes and compression ratio, deterministic ZIP writing that can copy untouched parts byte for byte, content types, relationships, part-name validation, and read-only access to OLE compound files (`.doc` files, VBA projects, encrypted packages). It is the first code to touch hostile bytes, so every parser enforces resource limits and has a fuzz target ([ADR-0006]).
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-005].
//!
//! ## Layer
//!
//! bayan-opc belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-opc may therefore depend only on other Foundation crates and on external libraries.
//!
//! [CORE-005]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-005-bayan-opc.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [ADR-0006]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0006-rust-core-and-memory-safety.md

#![forbid(unsafe_code)]
