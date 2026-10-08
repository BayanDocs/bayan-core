//! # xtask
//!
//! Build automation for bayan-core, run as `cargo xtask <command>` (the alias is defined in `.cargo/config.toml`). It uses only the Rust standard library, so the tool that checks the workspace's dependencies brings none of its own.
//!
//! - `cargo xtask verify` runs the verification gate: every check a change must pass before it is pushed, the same locally and in CI. See `verify.rs` and the "Verification gate" section of AGENTS.md.
//! - `cargo xtask sdk`, `cargo xtask wasm-package` and `cargo xtask c-driver` build the engine's artifacts for the shells, the C SDK and the WebAssembly package, and run the C test driver against the SDK; `cargo xtask miri` runs bayan-ffi's tests under Miri (`artifacts.rs`, work package CORE-007).
//!
//! - `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age [--base <revision>]` run the supply-chain checks of ADR-0017 that Cargo and cargo-deny do not make (work package X-003; see `supply_chain/`). The gate runs both in its supply-chain step.
//!
//! xtask never builds its own executable while it runs (Windows cannot replace a running program), so the gate's commands check, test or document xtask but do not build its binary.

#![forbid(unsafe_code)]
#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "xtask reports its progress to the person or CI job running it"
)]

mod artifacts;
mod canary;
mod flags;
mod json;
mod notices;
mod policy;
mod process;
mod sources;
mod supply_chain;
mod toml_subset;
mod verify;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  verify   Run the verification gate: formatting, lints, tests (on the host and in
           WebAssembly), WebAssembly build, documentation, dependency policy, the lint
           guardrails, the supply-chain checks and the determinism checks.
           Run it before every push; CI runs the same command.
  check-exact-pins
           Check that every dependency is pinned exactly (=x.y.z), in
           [workspace.dependencies] and in every crate (ADR-0017 rule 5).
  check-lockfile-age [--base <revision>]
           Check that every package version added to Cargo.lock since the merge
           base with <revision> (default: origin/<GITHUB_BASE_REF> in a pull request
           on GitHub Actions, otherwise origin/main) was published on crates.io at
           least 24 hours before the commit that added it, and before now
           (ADR-0017 rule 4). Needs git and curl; uses the network only when
           Cargo.lock changed.
  sdk [--target <triple>] [--out <folder>]
           Build the engine's C SDK (static and dynamic library, C header, JSON Schema,
           licence notices) for the host or a target, by default into
           target/artifacts/bayan-core-sdk-<triple>.
  wasm-package [--out <folder>]
           Build the engine's WebAssembly package for the web shell (module, glue, worker
           host, schema, TypeScript declarations, test page, licence notices), by default
           into target/artifacts/bayan-core-wasm. Needs wasm-bindgen-cli at the version of
           the wasm-bindgen crate in Cargo.lock (scripts/dev-setup.sh installs it).
  c-driver [--sdk <folder>]
           Compile the C test driver against the SDK, linked statically and dynamically,
           and run both. Uses cc (or $CC), or MSVC's cl on Windows.
  miri     Run bayan-ffi's unit tests under Miri, on the nightly toolchain that
           scripts/dev-setup.sh installs with it.
  help     Show this message.";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let args: Vec<&str> = args.iter().map(|arg| arg.to_str().unwrap_or("")).collect();
    match args.as_slice() {
        ["verify"] => verify::run(),
        ["check-exact-pins"] => {
            standalone(|report| supply_chain::exact_pins::check(&workspace_root(), report))
        }
        ["check-lockfile-age"] => {
            standalone(|report| supply_chain::lockfile_age::check(&workspace_root(), None, report))
        }
        ["check-lockfile-age", "--base", base] => standalone(|report| {
            supply_chain::lockfile_age::check(&workspace_root(), Some(base), report)
        }),
        ["sdk", rest @ ..] => artifacts::sdk(rest),
        ["wasm-package", rest @ ..] => artifacts::wasm_package(rest),
        ["c-driver", rest @ ..] => artifacts::c_driver(rest),
        ["miri", rest @ ..] => artifacts::miri(rest),
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

/// Runs one check on its own, printing what it reports, and turns its result into the exit status.
fn standalone(check: impl FnOnce(&mut dyn FnMut(&str)) -> Result<(), String>) -> ExitCode {
    match check(&mut |line| println!("{line}")) {
        Ok(()) => {
            println!("ok");
            ExitCode::SUCCESS
        }
        Err(problem) => {
            eprintln!("{problem}");
            ExitCode::FAILURE
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
