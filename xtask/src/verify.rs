//! `cargo xtask verify`: the verification gate of bayan-core.
//!
//! The gate runs the same steps, in the same order, on a developer's machine and in CI on Linux, Windows and macOS. It stops at the first failing step. Each command is printed before it runs, so a failing step can be re-run by itself.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use crate::process::{self, cargo};
use crate::{canary, policy};

/// The cargo-deny version the gate uses. CI installs exactly this version, checked against SHA-256 checksums in `.github/workflows/verify.yml`; change both together, in the monthly dependency session.
pub const CARGO_DENY_VERSION: &str = "0.20.2";

/// The WebAssembly target of the web app's engine (ADR-0014).
const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// Crates that run natively only and are therefore not built for WebAssembly, with the reason. Every other workspace member must build for WebAssembly, so a new crate is covered without anyone having to remember it.
pub const NATIVE_ONLY: [(&str, &str); 3] = [
    ("bayan-cli", "a command-line tool, run natively"),
    (
        "bayan-ffi",
        "the C interface for the desktop app; the web app uses bayan-wasm",
    ),
    ("xtask", "this build automation"),
];

/// A check that plugs into one of the hook steps.
type Check = (&'static str, fn(&Path) -> Result<(), String>);

/// Hook for the supply-chain checks of ADR-0017 that Cargo and cargo-deny do not cover. Work package X-003 adds `check-exact-pins` (every entry of `[workspace.dependencies]` is an exact `=x.y.z` requirement) and `check-lockfile-age` (no package version in `Cargo.lock` is younger than 24 hours) here, and as `cargo xtask` commands.
const SUPPLY_CHAIN_CHECKS: &[Check] = &[];

/// Hook for determinism checks (ADR-0004, ADR-0025 §1): checks that the same input produces identical layout and pixel hashes on every platform plug in here, from CORE-002 and CORE-003 onward.
const DETERMINISM_CHECKS: &[Check] = &[];

/// One step of the gate.
pub struct Step {
    /// A short, stable name.
    pub name: &'static str,
    /// What the step checks.
    pub title: &'static str,
    run: fn(&Path) -> Result<(), String>,
}

/// The steps, in order. The first six are the ones work package CORE-001 lists, in its order.
pub const STEPS: [Step; 9] = [
    Step {
        name: "fmt",
        title: "Formatting (rustfmt)",
        run: fmt,
    },
    Step {
        name: "clippy",
        title: "Lints (Clippy; warnings are errors)",
        run: clippy,
    },
    Step {
        name: "test",
        title: "Tests",
        run: test,
    },
    Step {
        name: "wasm32",
        title: "WebAssembly build",
        run: wasm32,
    },
    Step {
        name: "doc",
        title: "Documentation (warnings are errors)",
        run: doc,
    },
    Step {
        name: "deny",
        title: "Dependency policy (cargo-deny: advisories, licenses, bans, sources)",
        run: deny,
    },
    Step {
        name: "guardrails",
        title: "Guardrails: lint configuration and lint canaries",
        run: guardrails,
    },
    Step {
        name: "supply-chain",
        title: "Supply-chain checks (X-003)",
        run: supply_chain,
    },
    Step {
        name: "determinism",
        title: "Determinism checks (ADR-0025)",
        run: determinism,
    },
];

/// Runs the whole gate.
pub fn run() -> ExitCode {
    let root = crate::workspace_root();
    let started = Instant::now();
    println!("cargo xtask verify: the bayan-core verification gate");
    if let Err(problem) = preflight(&root) {
        return fail("preflight", &problem);
    }
    for (index, step) in STEPS.iter().enumerate() {
        let heading = format!(
            "[{}/{}] {}: {}",
            index + 1,
            STEPS.len(),
            step.name,
            step.title
        );
        group_start(&heading);
        let step_started = Instant::now();
        let result = (step.run)(&root);
        group_end();
        match result {
            Ok(()) => println!("    ok ({})", duration(step_started.elapsed())),
            Err(problem) => return fail(&heading, &problem),
        }
    }
    println!(
        "\ncargo xtask verify: all {} steps passed in {}",
        STEPS.len(),
        duration(started.elapsed())
    );
    ExitCode::SUCCESS
}

/// Checks that the gate runs with the pinned tools and with the root Clippy configuration only, so that a pass means the same thing everywhere.
fn preflight(root: &Path) -> Result<(), String> {
    let toolchain_toml = std::fs::read_to_string(root.join("rust-toolchain.toml"))
        .map_err(|error| format!("cannot read rust-toolchain.toml: {error}"))?;
    let channel = policy::toolchain_channel(&toolchain_toml)?;
    let mut rustc = Command::new("rustc");
    rustc.arg("--version").current_dir(root);
    let rustc_version = first_line(&mut rustc)?;
    if !rustc_version.starts_with(&format!("rustc {channel} ")) {
        return Err(format!(
            "the gate runs on the toolchain pinned in rust-toolchain.toml ({channel}), but `rustc --version` reports `{rustc_version}`. Run plain `cargo xtask verify` (without `+toolchain`), and rustup installs the pinned version."
        ));
    }
    println!("    toolchain: {rustc_version} (pinned in rust-toolchain.toml)");

    let mut deny = cargo(root);
    deny.args(["deny", "--version"]);
    let found = first_line(&mut deny).map_err(|_| {
        format!(
            "cargo-deny is not installed. Install version {CARGO_DENY_VERSION}: `cargo install --locked cargo-deny@{CARGO_DENY_VERSION}` (CI downloads the release binary and checks its SHA-256)."
        )
    })?;
    match tool_version(&found, "cargo-deny") {
        Some(version) if version == CARGO_DENY_VERSION => {
            println!("    cargo-deny: {version} (pinned in xtask/src/verify.rs)");
        }
        _ => {
            return Err(format!(
                "the gate is pinned to cargo-deny {CARGO_DENY_VERSION}, but found `{found}`. Install it with `cargo install --locked cargo-deny@{CARGO_DENY_VERSION}`."
            ));
        }
    }

    // Checked before Clippy first runs, because another configuration file would quietly change what every later step checks.
    let clippy_conf_dir = std::env::var_os("CLIPPY_CONF_DIR");
    let line = policy::check_clippy_configuration(root, clippy_conf_dir.as_deref())?;
    println!("    {line}");
    Ok(())
}

fn fmt(root: &Path) -> Result<(), String> {
    process::run(cargo(root).args(["fmt", "--all", "--check"]))
}

fn clippy(root: &Path) -> Result<(), String> {
    process::run(cargo(root).args([
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--locked",
        "--",
        "-D",
        "warnings",
    ]))
}

fn test(root: &Path) -> Result<(), String> {
    process::run(cargo(root).args(["test", "--workspace", "--locked"]))
}

fn wasm32(root: &Path) -> Result<(), String> {
    for (name, reason) in NATIVE_ONLY {
        println!("    not built for WebAssembly: {name} ({reason})");
    }
    let mut command = cargo(root);
    command.args(["build", "--workspace"]);
    for (name, _) in NATIVE_ONLY {
        command.args(["--exclude", name]);
    }
    // Warnings are errors here too: code compiled only for WebAssembly is not seen by the Clippy step, which checks the host platform.
    command
        .args(["--target", WASM_TARGET, "--locked"])
        .env("RUSTFLAGS", "-D warnings")
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    process::run(&mut command)
}

fn doc(root: &Path) -> Result<(), String> {
    process::run(
        cargo(root)
            .args(["doc", "--workspace", "--no-deps", "--locked"])
            .env("RUSTDOCFLAGS", "-D warnings")
            .env_remove("CARGO_ENCODED_RUSTDOCFLAGS"),
    )
}

fn deny(root: &Path) -> Result<(), String> {
    process::run(cargo(root).args(["deny", "--locked", "check"]))
}

fn guardrails(root: &Path) -> Result<(), String> {
    let workspace = policy::Workspace::load(root)?;
    let native_only: Vec<&str> = NATIVE_ONLY.iter().map(|(name, _)| *name).collect();
    for line in policy::check(&workspace, &native_only)? {
        println!("    ok: {line}");
    }
    canary::check_all(root, &workspace.clippy)
}

fn supply_chain(root: &Path) -> Result<(), String> {
    run_hook(SUPPLY_CHAIN_CHECKS, root, "work package X-003 adds them")
}

fn determinism(root: &Path) -> Result<(), String> {
    run_hook(
        DETERMINISM_CHECKS,
        root,
        "they arrive with the first layout and rendering code",
    )
}

fn run_hook(checks: &[Check], root: &Path, when_empty: &str) -> Result<(), String> {
    if checks.is_empty() {
        println!("    no checks yet: {when_empty}");
    }
    for (name, check) in checks {
        println!("    {name}");
        check(root)?;
    }
    Ok(())
}

/// Runs a command and returns the first line it printed, or an error if it failed.
fn first_line(command: &mut Command) -> Result<String, String> {
    let output = process::capture(command)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    match stdout.lines().next() {
        Some(line) if output.status.success() => Ok(line.trim().to_owned()),
        _ => Err(format!("`{}` failed", process::display(command))),
    }
}

/// Reads the version from a line such as `cargo-deny 0.20.2`.
fn tool_version<'a>(line: &'a str, tool: &str) -> Option<&'a str> {
    let mut words = line.split_whitespace();
    (words.next() == Some(tool)).then(|| words.next()).flatten()
}

fn fail(step: &str, problem: &str) -> ExitCode {
    eprintln!("\ncargo xtask verify FAILED at {step}:\n{problem}");
    if in_github_actions() {
        // An annotation that GitHub shows on the run's summary page.
        let message = format!("cargo xtask verify failed at {step}: {problem}");
        println!(
            "::error title=cargo xtask verify::{}",
            message
                .replace('%', "%25")
                .replace('\r', "%0D")
                .replace('\n', "%0A")
        );
    }
    ExitCode::FAILURE
}

/// In GitHub Actions, each step's output becomes a collapsible group in the log.
fn group_start(heading: &str) {
    if in_github_actions() {
        println!("::group::{heading}");
    } else {
        println!("\n==> {heading}");
    }
}

fn group_end() {
    if in_github_actions() {
        println!("::endgroup::");
    }
}

fn in_github_actions() -> bool {
    std::env::var_os("GITHUB_ACTIONS").is_some_and(|value| value == "true")
}

fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds >= 60 {
        format!("{} min {} s", seconds / 60, seconds % 60)
    } else {
        format!("{}.{} s", seconds, elapsed.subsec_millis() / 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_the_briefs_steps_first_and_in_its_order() {
        let names: Vec<&str> = STEPS.iter().map(|step| step.name).collect();
        assert_eq!(
            names,
            [
                "fmt",
                "clippy",
                "test",
                "wasm32",
                "doc",
                "deny",
                "guardrails",
                "supply-chain",
                "determinism"
            ]
        );
    }

    #[test]
    fn reads_tool_versions() {
        assert_eq!(
            tool_version("cargo-deny 0.20.2", "cargo-deny"),
            Some("0.20.2")
        );
        assert_eq!(
            tool_version("cargo-deny 0.20.2\n", "cargo-deny"),
            Some("0.20.2")
        );
        assert_eq!(tool_version("cargo-audit 0.20.2", "cargo-deny"), None);
        assert_eq!(tool_version("cargo-deny", "cargo-deny"), None);
        assert_eq!(tool_version("", "cargo-deny"), None);
    }

    #[test]
    fn pins_the_same_cargo_deny_as_ci() {
        let workflow = include_str!("../../.github/workflows/verify.yml");
        assert!(
            workflow.contains(&format!("CARGO_DENY_VERSION: {CARGO_DENY_VERSION}\n"))
                || workflow.contains(&format!("CARGO_DENY_VERSION: {CARGO_DENY_VERSION}\r\n")),
            "the CI workflow must install cargo-deny {CARGO_DENY_VERSION}"
        );
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(Duration::from_millis(4_250)), "4.2 s");
        assert_eq!(duration(Duration::from_secs(83)), "1 min 23 s");
    }
}
