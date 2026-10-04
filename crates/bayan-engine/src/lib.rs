//! # bayan-engine
//!
//! The engine facade that both shells talk to.
//!
//! The desktop and web shells drive the engine through one versioned message protocol: commands, queries and input go in, events come out. This crate will implement that protocol, session management, background scheduling, host-service requests (the engine never opens sockets or reads arbitrary files itself; it asks the shell) and record and replay ([engine protocol specification][protocol-spec], [ADR-0012]).
//!
//! **Status:** an empty skeleton created by CORE-001. Its implementation arrives with work package [CORE-007].
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
