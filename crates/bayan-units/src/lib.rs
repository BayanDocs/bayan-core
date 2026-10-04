//! # bayan-units
//!
//! Integer layout units, geometry, unit conversions, rounding and deterministic math.
//!
//! All layout geometry in BayanDocs is measured in **Bayan Layout Units** (BLU), stored as integers: 1 BLU = 1/1,828,800 inch = 1/25,400 point, so twips, points, EMUs, millimetres and common screen resolutions are all exact whole numbers of BLU. This crate will provide the BLU type with checked arithmetic, exact conversions, explicit rounding modes, geometry types, and the deterministic math functions that replace the platform's floating-point `sin`, `exp` and similar methods, which the workspace's Clippy configuration forbids ([ADR-0005]).
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-002].
//!
//! ## Layer
//!
//! bayan-units belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-units may therefore depend only on other Foundation crates and on external libraries.
//!
//! [CORE-002]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-002-bayan-units.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [ADR-0005]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0005-layout-units-and-deterministic-math.md

#![forbid(unsafe_code)]
