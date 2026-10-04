//! # xtask
//!
//! Build automation for bayan-core, run as `cargo xtask <command>` (the alias is defined in `.cargo/config.toml`). It uses only the Rust standard library, so the tool that checks the workspace's dependencies brings none of its own.
//!
//! - `cargo xtask verify` runs the verification gate: every check a change must pass before it is pushed, the same locally and in CI. See `verify.rs` and the "Verification gate" section of AGENTS.md.
//!
//! Work package X-003 adds the `check-exact-pins` and `check-lockfile-age` commands and plugs them into the gate (see `SUPPLY_CHAIN_CHECKS` in `verify.rs`).
//!
//! xtask never builds its own executable while it runs (Windows cannot replace a running program), so the gate's commands check, test or document xtask but do not build its binary.

#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "xtask reports its progress to the person or CI job running it"
)]

mod canary;
mod flags;
mod json;
mod policy;
mod process;
mod toml_subset;
mod verify;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  verify   Run the verification gate: formatting, lints, tests, WebAssembly build,
           documentation, dependency policy, and the lint guardrails.
           Run it before every push; CI runs the same command.
  help     Show this message.";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let args: Vec<&str> = args.iter().map(|arg| arg.to_str().unwrap_or("")).collect();
    match args.as_slice() {
        ["verify"] => verify::run(),
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// The root of the workspace: xtask lives in `<root>/xtask`.
///
/// `cargo run` tells the program where its package is, which is right even when one compiled xtask is shared between two copies of the repository through a common `CARGO_TARGET_DIR`. The location recorded at compile time is only the fallback for running the binary directly.
fn workspace_root() -> PathBuf {
    let xtask = std::env::var_os("CARGO_MANIFEST_DIR")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
    xtask
        .parent()
        .map_or_else(|| xtask.clone(), Path::to_path_buf)
}
