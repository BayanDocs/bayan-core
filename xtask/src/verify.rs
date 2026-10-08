//! `cargo xtask verify`: the verification gate of bayan-core.
//!
//! The gate runs the same steps, in the same order, on a developer's machine and in CI on Linux, Windows and macOS. It stops at the first failing step. Each command is printed before it runs, so a failing step can be re-run by itself.

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use crate::process::{self, cargo};
use crate::sources::{self, Condition};
use crate::{canary, flags, policy};

/// The cargo-deny version the gate uses. CI installs exactly this version, checked against SHA-256 checksums in `.github/workflows/verify.yml`; change both together, in the monthly dependency session.
pub const CARGO_DENY_VERSION: &str = "0.20.2";

/// The WebAssembly target of the web app's engine (ADR-0014).
const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The WebAssembly target the tests run on. The engine's own target, wasm32-unknown-unknown, offers no standard way to run Rust's test harness, so the tests are built for WASI (wasm32-wasip1) and run in Node.js, as ADR-0025 §1 foresees for the determinism checks. Both targets compile the code to the same WebAssembly instructions with the same arithmetic; WASI adds only the operating-system interface the test harness needs to print its results and report how it ended. `rust-toolchain.toml` lists the target, so rustup installs it.
const WASM_TEST_TARGET: &str = "wasm32-wasip1";

/// The Node.js version that runs the tests in WebAssembly. CI installs exactly this version, checked against SHA-256 checksums, in `.github/workflows/verify.yml`, and `scripts/dev-setup.sh` installs it for developers; a test keeps the three equal. It is also the version of bayan-web's `.nvmrc` and of the BayanDocs cloud environment. Change all of them together, in the monthly dependency session.
pub const NODE_VERSION: &str = "24.21.0";

/// Crates that run natively only and are therefore not built for WebAssembly, with the reason. Every other workspace member must build for WebAssembly, so a new crate is covered without anyone having to remember it.
pub const NATIVE_ONLY: [(&str, &str); 3] = [
    ("bayan-cli", "a command-line tool, run natively"),
    (
        "bayan-ffi",
        "the C interface for the desktop app; the web app uses bayan-wasm",
    ),
    ("xtask", "this build automation"),
];

/// Workspace members that may have a build script, with the reason. A build script runs code during the build and can change how its crate is compiled (for example, setting `CLIPPY_CONF_DIR` through `cargo::rustc-env` would switch off the bans of the root `clippy.toml` for that crate), so the preflight rejects every build script that is not listed here (ADR-0017 asks to keep them to a minimum). Reviewers read every listed script, and a build script must never set Clippy or compiler variables.
pub const BUILD_SCRIPTS: [(&str, &str); 0] = [];

/// Workspace members that may be procedural-macro crates (`proc-macro = true` in `[lib]`), with the reason. A procedural macro runs code during the build and can produce any code, hidden conditions included, without that code appearing in a source file, so the source check cannot see what it writes; the preflight therefore rejects every procedural-macro crate of the workspace that is not listed here. Reviewers read every listed macro in full. Procedural macros from dependencies, such as serde's derive, are not affected.
pub const PROC_MACROS: [(&str, &str); 0] = [];

/// The conditions that code may use in `cfg(…)`, `cfg!(…)` and the first argument of `cfg_attr(…)`, combined with `all`, `any`, `not`, `true` and `false`, each with the reason it is approved. The Clippy step compiles the code under every combination of their values ([`CLIPPY_RUNS`]), so no code can choose a configuration that Clippy does not check (ADR-0005 §4 and §5, ADR-0025); the preflight rejects every other condition (`sources.rs`). A condition is added only in a reviewed pull request, together with the Clippy runs that check the code with it both on and off: the test `the_clippy_runs_check_every_combination_of_the_approved_conditions` fails otherwise.
pub const APPROVED_CONDITIONS: [Condition; 3] = [
    Condition {
        name: "test",
        value: None,
        reason: "unit tests and the code only they use; every Clippy run compiles each crate both with and without it (`--all-targets`)",
    },
    Condition {
        name: "debug_assertions",
        value: None,
        reason: "checks that only debug builds make, such as checked arithmetic in bayan-units (ADR-0005 §6, CORE-002); on in the dev profile, off in the release profile",
    },
    Condition {
        name: "target_arch",
        value: Some("wasm32"),
        reason: "code for the web app's WebAssembly engine only (ADR-0014), such as the binding in bayan-wasm (CORE-007) and tests that run in WebAssembly (CORE-002); on in the wasm32 runs, off in the host runs",
    },
];

/// The lint canary's switch, `bayan_lint_canary = "<case>"`, which only the canary's own crate ([`CANARY_PACKAGE`]) may use, with any value. No Clippy run sets it: the guardrails step compiles each case on its own (`canary.rs`).
pub const CANARY_SWITCH: &str = "bayan_lint_canary";

/// The crate that may use the lint canary's switch.
pub const CANARY_PACKAGE: &str = "lint-canary";

/// One run of Clippy in the Clippy step.
pub struct ClippyRun {
    /// The platform: `None` for the host that the gate runs on, otherwise a target such as `wasm32-unknown-unknown`.
    pub target: Option<&'static str>,
    /// Whether it uses the release profile instead of the dev profile.
    pub release: bool,
}

/// The runs of the Clippy step: the host and WebAssembly, each in the dev and the release profile, all with `--all-targets --all-features --locked -- -D warnings`. Together they compile the code under every combination of the [`APPROVED_CONDITIONS`]: `--all-targets` compiles each crate with and without `test`, the dev profile switches `debug_assertions` on and the release profile off, and only the WebAssembly runs have `target_arch = "wasm32"`. The WebAssembly runs leave out the `NATIVE_ONLY` crates, as the wasm32 step does, and like the other runs they read the compiler flags of `.cargo/config.toml` (the wasm32 step sets `RUSTFLAGS` instead).
pub const CLIPPY_RUNS: [ClippyRun; 4] = [
    ClippyRun {
        target: None,
        release: false,
    },
    ClippyRun {
        target: None,
        release: true,
    },
    ClippyRun {
        target: Some(WASM_TARGET),
        release: false,
    },
    ClippyRun {
        target: Some(WASM_TARGET),
        release: true,
    },
];

/// A check that plugs into one of the hook steps.
type Check = (&'static str, fn(&Path) -> Result<(), String>);

/// Hook for the supply-chain checks of ADR-0017 that Cargo and cargo-deny do not cover. Work package X-003 adds `check-exact-pins` (every entry of `[workspace.dependencies]` is an exact `=x.y.z` requirement) and `check-lockfile-age` (no package version in `Cargo.lock` is younger than 24 hours) here, and as `cargo xtask` commands.
const SUPPLY_CHAIN_CHECKS: &[Check] = &[];

/// Hook for determinism checks (ADR-0004, ADR-0025 §1): checks that the same input produces identical layout and pixel hashes on every platform plug in here, from CORE-002 and CORE-003 onward.
const DETERMINISM_CHECKS: &[Check] = &[(
    "bayan-units: the fixed test vector of the deterministic math (CORE-002) gives the committed fingerprints on the host and in WebAssembly",
    math_fingerprints,
)];

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
        title: "Lints (Clippy for the host and WebAssembly, each in the dev and the release profile; warnings are errors)",
        run: clippy_step,
    },
    Step {
        name: "test",
        title: "Tests (on the host, then in WebAssembly with Node.js)",
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

/// Checks that the gate runs with the pinned tools, with the root Clippy configuration only, with no compiler flag that lowers lint levels and no unlisted build script or procedural-macro crate, and that the Rust sources use only the approved conditions, written out in full, and do not make the compiler read a file that the source check does not (`sources.rs`), so that a pass means the same thing everywhere. Everything here is checked before anything is built.
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

    let mut node = Command::new("node");
    node.arg("--version").current_dir(root);
    let found = first_line(&mut node).map_err(|_| {
        format!(
            "Node.js is not installed, or not on PATH. The gate runs the tests in WebAssembly with Node.js {NODE_VERSION}: run scripts/dev-setup.sh, which installs it with a verified checksum (on Linux and macOS) and prints how to put it on PATH."
        )
    })?;
    println!("    {}", check_node(&found)?);

    // Checked before Clippy first runs, because another configuration file would quietly change what every later step checks.
    let clippy_conf_dir = std::env::var_os("CLIPPY_CONF_DIR");
    let line = policy::check_clippy_configuration(root, clippy_conf_dir.as_deref())?;
    println!("    {line}");

    // Cargo passes the [env] table of .cargo/config.toml to `cargo run`, so xtask's environment includes it.
    let env: Vec<(String, String)> = std::env::vars_os()
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .collect();
    let line = flags::check(&env, &flags::read_configs(root)?)?;
    println!("    {line}");

    // Checked before anything is built, because Cargo runs a build script before it compiles the script's crate, and a procedural macro while it compiles the crates that use it.
    let packages = policy::packages(&policy::metadata(root)?)?;
    let members: Vec<String> = packages
        .iter()
        .map(|package| package.name.clone())
        .collect();
    for (code, list) in [
        (&policy::BUILD_SCRIPT, &BUILD_SCRIPTS[..]),
        (&policy::PROC_MACRO, &PROC_MACROS[..]),
    ] {
        let allowed: Vec<&str> = list.iter().map(|(name, _)| *name).collect();
        let found = policy::targets_of_kind(&packages, code.kind);
        let line = policy::check_listed(code, &members, &found, &allowed)?;
        println!("    {line}");
    }

    // Checked before Clippy first runs, because code under a condition that no Clippy run sets, or that a macro builds, would pass the Clippy step unseen.
    let allowed = sources::Allowed {
        conditions: &APPROVED_CONDITIONS,
        canary_switch: CANARY_SWITCH,
        canary_package: CANARY_PACKAGE,
    };
    let line = sources::check(root, &packages, &allowed)?;
    println!("    {line}");
    Ok(())
}

fn fmt(root: &Path) -> Result<(), String> {
    process::run(cargo(root).args(["fmt", "--all", "--check"]))
}

/// Runs Clippy once for every entry of [`CLIPPY_RUNS`], printing which run it is, and stops at the first that fails.
fn clippy_step(root: &Path) -> Result<(), String> {
    for (index, run) in CLIPPY_RUNS.iter().enumerate() {
        let heading = format!(
            "Clippy run {} of {}: {}",
            index + 1,
            CLIPPY_RUNS.len(),
            describe(run)
        );
        println!("    {heading}");
        if run.target.is_some() {
            for (name, reason) in NATIVE_ONLY {
                println!("    not checked for WebAssembly: {name} ({reason})");
            }
        }
        process::run(cargo(root).args(clippy_args(run)))
            .map_err(|problem| format!("{heading}: {problem}"))?;
    }
    Ok(())
}

/// The platform and profile of a Clippy run, for a person.
fn describe(run: &ClippyRun) -> String {
    format!(
        "{}, {}",
        run.target.unwrap_or("the host"),
        if run.release {
            "release profile (debug assertions off)"
        } else {
            "dev profile (debug assertions on)"
        }
    )
}

/// The arguments after `cargo` for one Clippy run.
fn clippy_args(run: &ClippyRun) -> Vec<String> {
    let mut args = vec!["clippy".to_owned(), "--workspace".to_owned()];
    if run.target.is_some() {
        for (name, _) in NATIVE_ONLY {
            args.extend(["--exclude".to_owned(), name.to_owned()]);
        }
    }
    args.extend(["--all-targets", "--all-features", "--locked"].map(str::to_owned));
    if run.release {
        args.push("--release".to_owned());
    }
    if let Some(target) = run.target {
        args.extend(["--target".to_owned(), target.to_owned()]);
    }
    args.extend(["--", "-D", "warnings"].map(str::to_owned));
    args
}

/// Runs the tests on the host, then the same tests in WebAssembly (CORE-002, acceptance criterion 2), where they run as WASI programs in Node.js.
fn test(root: &Path) -> Result<(), String> {
    process::run(cargo(root).args(["test", "--workspace", "--locked"]))?;
    println!(
        "    The same tests in WebAssembly ({WASM_TEST_TARGET}), run by Node.js {NODE_VERSION} through xtask/wasi-runner.mjs:"
    );
    for (name, reason) in NATIVE_ONLY {
        println!("    not tested in WebAssembly: {name} ({reason})");
    }
    process::run(cargo(root).args(wasm_test_args(root, &workspace_without_native_only(), &[])?))
}

/// Runs the fixed test vector of bayan-units' deterministic math on the host and in WebAssembly, showing the fingerprints each run prints. The tests compare them with the fingerprints committed in `crates/bayan-units/tests/determinism.rs`, so both passing means both produced exactly those bits. CI runs the gate on Linux x86-64, Windows x86-64 and macOS arm64, which makes four platforms (CORE-002, acceptance criterion 4).
fn math_fingerprints(root: &Path) -> Result<(), String> {
    let vector = ["--package", "bayan-units", "--test", "determinism"];
    process::run(
        cargo(root)
            .arg("test")
            .args(vector)
            .args(["--locked", "--", "--show-output"]),
    )?;
    process::run(cargo(root).args(wasm_test_args(root, &vector, &[])?))
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
    // Warnings are errors here too. Setting RUSTFLAGS makes Cargo ignore the compiler flags of .cargo/config.toml in this step (CORE-001 follow-up 4); the Clippy step's WebAssembly runs read them.
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
    canary::check_all(root, &workspace.clippy_config)
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

/// The arguments that select the workspace members to test in WebAssembly: all but the `NATIVE_ONLY` crates.
fn workspace_without_native_only() -> Vec<&'static str> {
    let mut selection = vec!["--workspace"];
    for (name, _) in NATIVE_ONLY {
        selection.extend(["--exclude", name]);
    }
    selection
}

/// The arguments after `cargo` that run the tests of `selection` (such as `--workspace`) in WebAssembly, with Node.js and the runner script as Cargo's runner for the target, followed by `--no-capture` and `harness` for the test harness itself.
///
/// The runner is passed as a TOML array through `--config`, not as an environment variable, because Cargo splits the variable's value at spaces and the repository's path may contain some.
fn wasm_test_args(
    root: &Path,
    selection: &[&str],
    harness: &[&str],
) -> Result<Vec<String>, String> {
    let script = root.join("xtask").join("wasi-runner.mjs");
    let script = script.to_str().ok_or_else(|| {
        format!(
            "the path of the WebAssembly test runner, {}, is not valid Unicode, so it cannot be handed to Cargo",
            script.display()
        )
    })?;
    let runner = format!(
        "target.{WASM_TEST_TARGET}.runner = [\"node\", \"--disable-warning=ExperimentalWarning\", {}]",
        toml_string(script)
    );
    let mut args = vec!["test".to_owned()];
    args.extend(selection.iter().map(|&arg| arg.to_owned()));
    args.extend(["--locked", "--target", WASM_TEST_TARGET, "--config"].map(str::to_owned));
    args.push(runner);
    // In WebAssembly a panic aborts the whole test program, before the test harness could print the output it captured, and with it the failure's message (or proptest's smallest failing input). So nothing is captured: everything is printed as it happens. The tests run one after the other there, so their output does not interleave.
    args.extend(["--", "--no-capture"].map(str::to_owned));
    args.extend(harness.iter().map(|&arg| arg.to_owned()));
    Ok(args)
}

/// `text` as a TOML basic string: in double quotes, with backslashes, double quotes and control characters escaped.
fn toml_string(text: &str) -> String {
    let mut quoted = String::from("\"");
    for character in text.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            control if control.is_control() => {
                quoted.push_str(&format!("\\u{:04X}", u32::from(control)));
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Checks the first line that `node --version` printed against the pinned version, and returns how to report it.
fn check_node(found: &str) -> Result<String, String> {
    if found == format!("v{NODE_VERSION}") {
        Ok(format!(
            "Node.js: {NODE_VERSION} (pinned in xtask/src/verify.rs), which runs the tests in WebAssembly"
        ))
    } else {
        Err(format!(
            "the gate runs the tests in WebAssembly with Node.js {NODE_VERSION}, but `node --version` reports `{found}`. Run scripts/dev-setup.sh, which installs it with a verified checksum (on Linux and macOS) and prints how to put it on PATH; in BayanDocs cloud sessions it is already installed, so run `. /etc/profile.d/zz-bayandocs.sh` first (see AGENTS.md)."
        ))
    }
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
    fn runs_clippy_for_the_host_and_webassembly_in_both_profiles() {
        let commands: Vec<String> = CLIPPY_RUNS
            .iter()
            .map(|run| clippy_args(run).join(" "))
            .collect();
        assert_eq!(
            commands,
            [
                "clippy --workspace --all-targets --all-features --locked -- -D warnings",
                "clippy --workspace --all-targets --all-features --locked --release -- -D warnings",
                "clippy --workspace --exclude bayan-cli --exclude bayan-ffi --exclude xtask --all-targets --all-features --locked --target wasm32-unknown-unknown -- -D warnings",
                "clippy --workspace --exclude bayan-cli --exclude bayan-ffi --exclude xtask --all-targets --all-features --locked --release --target wasm32-unknown-unknown -- -D warnings",
            ]
        );
        let described: Vec<String> = CLIPPY_RUNS.iter().map(describe).collect();
        assert_eq!(
            described,
            [
                "the host, dev profile (debug assertions on)",
                "the host, release profile (debug assertions off)",
                "wasm32-unknown-unknown, dev profile (debug assertions on)",
                "wasm32-unknown-unknown, release profile (debug assertions off)",
            ]
        );
    }

    /// The values that `condition` takes in a Clippy run with `args` (the arguments after `cargo`): both, when the run compiles code with and without it; one; or none, when this function does not know how a run sets the condition. They are read from the arguments themselves, so that the tests below check the commands that the Clippy step really runs. Approving a condition means teaching this function how the runs set it, and adding runs that set it both ways.
    fn values(condition: &Condition, args: &[String]) -> Vec<bool> {
        let flag = |name: &str| args.iter().any(|arg| arg == name);
        let target = args
            .iter()
            .position(|arg| arg == "--target")
            .and_then(|at| args.get(at + 1));
        match (condition.name, condition.value) {
            // `--all-targets` compiles every library and binary both on its own and as unit tests.
            ("test", None) => {
                if flag("--all-targets") {
                    vec![false, true]
                } else {
                    vec![false]
                }
            }
            // Cargo's dev profile switches debug assertions on, and its release profile off; another profile could do either.
            ("debug_assertions", None) if !flag("--profile") => vec![!flag("--release")],
            // The host is never WebAssembly, and a target names its architecture first.
            ("target_arch", Some(arch)) => {
                vec![target.is_some_and(|target| target.split('-').next() == Some(arch))]
            }
            _ => Vec::new(),
        }
    }

    /// The combinations of values of `conditions` under which none of the runs (`runs`, each as its arguments) compiles code, described for a person.
    fn unchecked(conditions: &[Condition], runs: &[Vec<String>]) -> Vec<String> {
        let mut missing = Vec::new();
        for combination in 0..1_usize << conditions.len() {
            let wanted = |index: usize| combination >> index & 1 == 1;
            let checked = runs.iter().any(|args| {
                conditions
                    .iter()
                    .enumerate()
                    .all(|(index, condition)| values(condition, args).contains(&wanted(index)))
            });
            if !checked {
                let described: Vec<String> = conditions
                    .iter()
                    .enumerate()
                    .map(|(index, condition)| {
                        format!(
                            "{} {}",
                            condition.written(),
                            if wanted(index) { "on" } else { "off" }
                        )
                    })
                    .collect();
                missing.push(described.join(", "));
            }
        }
        missing
    }

    /// The arguments of the real Clippy runs.
    fn real_runs() -> Vec<Vec<String>> {
        CLIPPY_RUNS.iter().map(clippy_args).collect()
    }

    #[test]
    fn every_approved_condition_is_on_and_off_in_some_clippy_run() {
        let runs = real_runs();
        for condition in &APPROVED_CONDITIONS {
            for value in [false, true] {
                assert!(
                    runs.iter()
                        .any(|args| values(condition, args).contains(&value)),
                    "no Clippy run compiles code with `{}` {}. A condition can be approved only together with the Clippy runs that check the code with it both on and off: add them to CLIPPY_RUNS, and teach `values` how they set the condition.",
                    condition.written(),
                    if value { "on" } else { "off" }
                );
            }
        }
    }

    #[test]
    fn the_clippy_runs_check_every_combination_of_the_approved_conditions() {
        let missing = unchecked(&APPROVED_CONDITIONS, &real_runs());
        assert!(
            missing.is_empty(),
            "no Clippy run compiles the code under these combinations of the approved conditions: {missing:?}. Code under such a combination (written with `all`, `any` and `not`) would be compiled by the tests or in a release but never checked by Clippy. Add the Clippy runs to CLIPPY_RUNS, or approve fewer conditions."
        );
    }

    #[test]
    fn notices_conditions_and_combinations_that_no_clippy_run_checks() {
        let runs = real_runs();
        // A feature approved without Clippy runs that switch it on and off: every combination with it is missing.
        let feature = Condition {
            name: "feature",
            value: Some("serde"),
            reason: "",
        };
        let missing = unchecked(&[APPROVED_CONDITIONS[1], feature], &runs);
        assert_eq!(missing.len(), 4, "{missing:?}");
        // Runs that set every condition both on and off, but not in every combination: here nothing checks debug builds for the host or release builds for WebAssembly.
        let partial = [clippy_args(&CLIPPY_RUNS[1]), clippy_args(&CLIPPY_RUNS[2])];
        for condition in &APPROVED_CONDITIONS {
            for value in [false, true] {
                assert!(
                    partial
                        .iter()
                        .any(|args| values(condition, args).contains(&value))
                );
            }
        }
        assert_eq!(
            unchecked(&APPROVED_CONDITIONS, &partial),
            [
                "test off, debug_assertions on, target_arch = \"wasm32\" off",
                "test on, debug_assertions on, target_arch = \"wasm32\" off",
                "test off, debug_assertions off, target_arch = \"wasm32\" on",
                "test on, debug_assertions off, target_arch = \"wasm32\" on",
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

    #[test]
    fn runs_the_tests_in_webassembly_with_node_and_the_runner_script() {
        let root = Path::new("/repo");
        let args = wasm_test_args(root, &workspace_without_native_only(), &[]).unwrap();
        let config = args.iter().position(|arg| arg == "--config").unwrap();
        assert_eq!(
            args[..=config],
            [
                "test",
                "--workspace",
                "--exclude",
                "bayan-cli",
                "--exclude",
                "bayan-ffi",
                "--exclude",
                "xtask",
                "--locked",
                "--target",
                "wasm32-wasip1",
                "--config",
            ]
        );
        let runner = &args[config + 1];
        assert!(
            runner.starts_with(r#"target.wasm32-wasip1.runner = ["node", "--disable-warning=ExperimentalWarning", "/repo"#),
            "{runner}"
        );
        assert!(runner.ends_with(r#"wasi-runner.mjs"]"#), "{runner}");
        // Nothing is captured: in WebAssembly a failing test aborts the program before the harness could print what it captured.
        assert_eq!(args[config + 2..], ["--", "--no-capture"]);
        // The runner script is the file in this repository.
        assert!(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("wasi-runner.mjs")
                .is_file()
        );
    }

    #[test]
    fn passes_arguments_to_the_test_harness_after_a_double_dash() {
        let args = wasm_test_args(
            Path::new("/repo"),
            &["--package", "bayan-units", "--test", "determinism"],
            &["--exact", "a_test"],
        )
        .unwrap();
        assert_eq!(
            &args[..5],
            ["test", "--package", "bayan-units", "--test", "determinism"]
        );
        assert_eq!(
            &args[args.len() - 4..],
            ["--", "--no-capture", "--exact", "a_test"]
        );
    }

    #[test]
    fn writes_text_as_toml_strings() {
        assert_eq!(
            toml_string("/repo/xtask/wasi-runner.mjs"),
            r#""/repo/xtask/wasi-runner.mjs""#
        );
        assert_eq!(
            toml_string(r"C:\Users\Ana\bayan-core"),
            r#""C:\\Users\\Ana\\bayan-core""#
        );
        assert_eq!(toml_string(r#"a "quoted" name"#), r#""a \"quoted\" name""#);
        assert_eq!(
            toml_string("tab\tand\u{7f}delete"),
            r#""tab\u0009and\u007Fdelete""#
        );
        assert_eq!(toml_string("Zoë/Ω"), "\"Zoë/Ω\"");
    }

    #[test]
    fn accepts_only_the_pinned_node() {
        assert!(check_node(&format!("v{NODE_VERSION}")).is_ok());
        let problem = check_node("v22.22.0").unwrap_err();
        assert!(
            problem.contains("v22.22.0") && problem.contains(NODE_VERSION),
            "{problem}"
        );
        assert!(check_node("").is_err());
    }

    #[test]
    fn pins_the_same_node_as_ci_and_the_setup_script() {
        let workflow = include_str!("../../.github/workflows/verify.yml");
        assert!(
            workflow
                .lines()
                .any(|line| line.trim() == format!("NODE_VERSION: {NODE_VERSION}")),
            "the CI workflow must install Node.js {NODE_VERSION}"
        );
        let script = include_str!("../../scripts/dev-setup.sh");
        assert!(
            script
                .lines()
                .any(|line| line.starts_with(&format!("NODE_VERSION={NODE_VERSION} "))),
            "scripts/dev-setup.sh must install Node.js {NODE_VERSION}"
        );
    }

    #[test]
    fn the_setup_script_never_lets_rustup_update_itself() {
        // `rustup toolchain install` and `rustup update` also update rustup itself, by default to its newest release, which can be less than 24 hours old (ADR-0017: no silent upgrades).
        let script = include_str!("../../scripts/dev-setup.sh");
        let installs: Vec<&str> = script
            .lines()
            .map(str::trim)
            .filter(|line| !line.starts_with('#'))
            .filter(|line| {
                line.contains("rustup toolchain install") || line.contains("rustup update")
            })
            .collect();
        assert!(
            !installs.is_empty(),
            "scripts/dev-setup.sh no longer installs the toolchain with rustup; update this test"
        );
        for line in installs {
            assert!(
                line.contains("--no-self-update"),
                "scripts/dev-setup.sh must pass --no-self-update to rustup: {line}"
            );
        }
    }
}
