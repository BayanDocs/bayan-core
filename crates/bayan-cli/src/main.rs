//! # bayan-cli
//!
//! The BayanDocs command-line tool for headless conversion, rendering and inspection of documents.
//!
//! It runs the engine without a user interface, which makes it the backbone of the Fidelity Lab (comparing BayanDocs with Microsoft Word) and of debugging. It is built for the desktop platforms only, not for WebAssembly.
//!
//! **Status:** its only commands so far write the engine protocol's machine-readable descriptions (work package CORE-007), which the build of the engine's artifacts uses: `bayan-cli protocol schema` writes the JSON Schema of the protocol's messages, and `bayan-cli protocol typescript` their TypeScript declarations, both to standard output. Document commands arrive with later work packages.
//!
//! ## Layer
//!
//! bayan-cli belongs to the **Bindings and tools** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-cli may therefore depend on crates of every layer and on external libraries.
//!
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "\
Usage: bayan-cli <command>

Commands:
  protocol schema       Write the JSON Schema of the engine protocol's messages.
  protocol typescript   Write the TypeScript declarations of the engine protocol's messages.
  help                  Show this message.";

#[expect(
    clippy::print_stderr,
    reason = "a command-line tool reports problems to the person running it on standard error"
)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let generated = match args.as_slice() {
        ["protocol", "schema"] => bayan_engine::schema::json_schema_file(),
        ["protocol", "typescript"] => bayan_engine::typescript::declarations_file(),
        ["help" | "--help" | "-h"] => Ok(format!("{USAGE}\n")),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let written = generated.and_then(|text| {
        let mut stdout = std::io::stdout().lock();
        stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("cannot write the output: {error}"))
    });
    match written {
        Ok(()) => ExitCode::SUCCESS,
        Err(problem) => {
            eprintln!("bayan-cli: {problem}");
            ExitCode::FAILURE
        }
    }
}
