//! # bayan-cli
//!
//! The BayanDocs command-line tool for headless conversion, rendering and inspection of documents.
//!
//! It runs the engine without a user interface, which makes it the backbone of the Fidelity Lab (comparing BayanDocs with Microsoft Word) and of debugging. It is built for the desktop platforms only, not for WebAssembly.
//!
//! **Status:** an empty skeleton created by CORE-001. It has no commands yet; they arrive with later work packages.
//!
//! ## Layer
//!
//! bayan-cli belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-cli may therefore depend on crates of every layer and on external libraries.
//!
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

use std::process::ExitCode;

#[expect(
    clippy::print_stderr,
    reason = "a command-line tool reports to the person running it on standard error"
)]
fn main() -> ExitCode {
    eprintln!("bayan-cli has no commands yet; they arrive with later BayanDocs work packages.");
    ExitCode::FAILURE
}
