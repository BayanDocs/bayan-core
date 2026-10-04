//! # bayan-wasm
//!
//! The WebAssembly binding of the engine and its Web Worker host, for the web shell.
//!
//! The web shell runs the engine as WebAssembly inside a Web Worker and exchanges protocol messages with it ([ADR-0012], [ADR-0014]). This crate will be that thin binding around bayan-engine, built with wasm-bindgen.
//!
//! **Unsafe code.** The JavaScript boundary requires `unsafe` code, so this is one of the two crates where it is not forbidden ([ADR-0006] §2). It is still denied by default: it is allowed only where `#[expect(unsafe_code, reason = "…")]` marks it, every `unsafe` block carries a `// SAFETY:` comment explaining why it is sound, and panics are reported to the worker host instead of crossing the boundary (ADR-0006 §4).
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-007].
//!
//! ## Layer
//!
//! bayan-wasm belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-wasm may therefore depend on crates of every layer and on external libraries.
//!
//! [CORE-007]: https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-007-engine-skeleton.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core
//! [ADR-0012]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0012-engine-boundary.md
//! [ADR-0014]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0014-web-shell.md
//! [ADR-0006]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0006-rust-core-and-memory-safety.md
