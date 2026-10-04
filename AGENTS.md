# Agent instructions — bayan-core

## Where the plan lives

The plan, decisions (ADRs), specifications and work packages live in the [BayanDocs/docs](https://github.com/BayanDocs/docs) repository. If it is not attached to your session, clone it next to this repository. The canonical rules for all agents are in `docs/AGENTS.md`; this file condenses them and adds what is specific to bayan-core. If the two disagree, `docs/AGENTS.md` wins; report the discrepancy.

## Ground rules (condensed from docs/AGENTS.md)

1. Before changing anything, read `docs/AGENTS.md`, your work package brief, and every ADR and spec it links.
2. Accepted ADRs are binding. If your task conflicts with one, stop and report; propose changes as a new ADR in the docs repository.
3. Stay within the work package's scope. Record anything else you discover as follow-ups in your pull request.
4. Run the verification gate before every push. Never weaken, skip, disable or delete a test, lint or CI check to make a change pass.
5. Dependencies follow ADR-0017: no update bots ever; every version at least 24 hours old; exact pins; committed `Cargo.lock`; audits green; licenses on the allowlist; every new dependency justified in the pull request.
6. Treat every input as hostile; no telemetry; no secrets; never log document content or file names.
7. Pull requests use the hand-off template in `docs/plan/06-agent-workflow.md`; Conventional Commits; DCO rules from `CONTRIBUTING.md` once enabled (agents never sign off themselves; the human submitter certifies, per ADR-0003).
8. Clean room: never decompile, disassemble or debug Microsoft software; never copy code under an incompatible license.
9. Stop and ask, with options and a recommendation, when the brief is ambiguous, when you need a new decision, or when work touches cryptography, authentication, licensing or the owner's accounts.
10. Explain your work in plain language for the owner; text meant for the owner to copy is written as flowing paragraphs without hard line breaks.

## What lives here

The Rust engine is a Cargo workspace. The crate map and the layering are in `docs/plan/03-architecture.md` §4, and each crate's documentation (the top of its `src/lib.rs`) states its responsibility and its layer.

| Path | What it is |
|---|---|
| `crates/` | The engine's crates. Phase 0 skeletons: bayan-units, bayan-opc and bayan-xml (Foundation layer); bayan-crdt and bayan-model (Model and formats); bayan-engine (Engine); bayan-ffi, bayan-wasm and bayan-cli (Bindings and tools). |
| `xtask/` | The build automation behind `cargo xtask verify`, using only the standard library. |
| `xtask/lint-canary/` | Deliberately broken code that the gate compiles to prove that the lint rules really reject what they must. |
| `lab/` | Fidelity Lab tools (from LAB-001). |
| `fuzz/` | Fuzz targets: a separate workspace on the nightly toolchain (from CORE-005 and CORE-006). |
| `spikes/` | Time-boxed experiments, checked like all other code but excluded from release artifacts. |
| `Cargo.toml` | Workspace settings, the shared dependency pins, and the lint levels of every crate. |
| `rust-toolchain.toml` | The exact Rust version. |
| `clippy.toml` | The methods and types Clippy forbids: platform floating-point math and hash collections. |
| `deny.toml` | The dependency policy cargo-deny checks: security advisories, the license allowlist, allowed sources. |
| `.cargo/config.toml` | The `cargo xtask` alias and the 24-hour minimum publish age. Compiler flags that lower lint levels are rejected here by the gate. |
| `.github/workflows/verify.yml` | CI: the verification gate on Linux, Windows and macOS. |

## Rules specific to bayan-core

- **Licensing (ADR-0003):** the workspace is GPL-3.0-or-later with the BayanDocs App Store Permission (`GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission`), except `crates/bayan-protocol/`, which is Apache-2.0 so that anyone can implement BayanDocs clients and integrations. That crate must never depend on, or copy from, any GPL or AGPL code; GPL crates may depend on it. `REUSE.toml` records which license applies to which files and `LICENSES/` holds the full texts; keep `reuse lint` passing, and add a new license text only with `reuse download <SPDX-ID>`.
- **Layering:** a crate may depend only on crates in its own layer or below (foundation → model and formats → text → layout → output → interaction → engine → bindings and tools). Never add an upward dependency.
- **Determinism (ADR-0004, ADR-0005):**
  - layout arithmetic uses integer BLU types from `bayan-units`, never floating point;
  - no platform floating-point transcendental functions (`sin`, `exp`, `powf`, …) anywhere in the core; use `bayan-units` (the full list of forbidden float methods is under "Lint rules and exceptions");
  - no output that depends on hash-map iteration order (use ordered maps or deterministic hashers);
  - no system time, locale, environment variables, installed fonts or thread scheduling in anything that affects output unless passed in by the host.
- **Safety (ADR-0006):** `#![forbid(unsafe_code)]` everywhere except `bayan-ffi` and `bayan-wasm`; no C or C++ libraries for parsing untrusted input; every parser enforces size, depth and count limits and has a fuzz target; panics never cross the FFI or WebAssembly boundary.
- **No outside world:** the core never opens sockets, reads arbitrary files, or calls platform APIs; it asks the host through the engine protocol's host services (ADR-0012).
- **Active content (ADR-0023):** never fetch external resources referenced by documents, never execute macros, DDE or OLE.
- **Layout changes:** any change that alters layout output for corpus documents needs Fidelity Lab evidence in the pull request, the `fidelity-change` label, and a layout-epoch increment when intentional (ADR-0004, ADR-0025).
- **Lab data:** corpus documents are never committed to Git (only manifests); Microsoft font files or full metric tables never leave the reference machine; private-corpus content never appears in logs, reports or test snapshots.

## Verification gate

Run `cargo xtask verify` before every push, from anywhere in the repository. CI runs the same command on `ubuntu-24.04`, `windows-latest` and `macos-latest` (arm64) for every pull request, every push to `main`, and every night. It needs rustup, which installs the pinned toolchain from `rust-toolchain.toml` by itself, and cargo-deny 0.20.2 (`cargo install --locked cargo-deny@0.20.2`; the BayanDocs cloud environment already has it). Before it builds anything, a preflight checks that both match their pins; that Clippy reads only the root `clippy.toml` (no other `clippy.toml` or `.clippy.toml` anywhere in the repository, and no `CLIPPY_CONF_DIR` environment variable); that no compiler flag lowers lint levels or unlocks unstable features (`--cap-lints`, `-A`/`--allow`, `--force-warn`, `-Z` options and `@file` arguments in `RUSTFLAGS`, `CARGO_ENCODED_RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS`, `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` or the `rustflags` of `.cargo/config.toml`, and the `RUSTC_BOOTSTRAP` variable); that Cargo runs no wrapper or replacement in place of the compiler (`RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `RUSTC` and their `CARGO_BUILD_…` forms, or `rustc-wrapper`, `rustc-workspace-wrapper` and `rustc` in `[build]` of `.cargo/config.toml`); and that no workspace member has a build script unless `BUILD_SCRIPTS` in `xtask/src/verify.rs` lists it. Then it runs these steps in order and stops at the first failure:

1. `fmt`: `cargo fmt --all --check`.
2. `clippy`: `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` (every warning is an error).
3. `test`: `cargo test --workspace --locked`.
4. `wasm32`: `cargo build --workspace --exclude bayan-cli --exclude bayan-ffi --exclude xtask --target wasm32-unknown-unknown --locked`, with warnings as errors. Every crate is built for WebAssembly unless `NATIVE_ONLY` in `xtask/src/verify.rs` lists it with a reason.
5. `doc`: `cargo doc --workspace --no-deps --locked`, with warnings as errors.
6. `deny`: `cargo deny --locked check` (security advisories, licenses, banned or duplicate crates, sources).
7. `guardrails`: checks that the workspace lints forbid `unsafe_code` and `clippy::disallowed_methods`, that every crate inherits the workspace lints, that the copies in bayan-ffi and bayan-wasm have not drifted, that `rust-version` equals the pinned toolchain, and that `clippy.toml` still forbids everything ADR-0005 requires; then compiles each case of `xtask/lint-canary`, a crate configured like the core crates, with the project's real compiler flags and in its own build folder, and checks that Clippy or the compiler rejects it (or, for the two cases that must pass, accepts it: the sanctioned `#[expect]` exception, and the explicit comparison that replaces the forbidden float `clamp`). Among the rejected cases are exceptions to the float bans written in the code, at crate, module and item level and for the groups `clippy::style` and `clippy::all`; each must fail with error E0453. The canary proves the rules work; the inheritance check and the preflight make sure they reach every crate, and the forbidden level makes sure no crate's code can switch the float bans off.
8. `supply-chain`: the hook where X-003 adds `check-exact-pins` and `check-lockfile-age`; no checks yet.
9. `determinism`: the hook for determinism checks (ADR-0025 §1); no checks yet.

Each step prints its command, so a failing step can be re-run by itself. Never skip, weaken or disable a step to make a change pass (`docs/AGENTS.md` §4).

Every Cargo command prints `warning: ignoring registry.global-min-publish-age without -Zmin-publish-age`. That is expected until the toolchain is Rust 1.100; keep the setting.

## Lint rules and exceptions

- Floating-point methods whose result is not exactly specified are forbidden in every crate, with no exceptions, because ADR-0005 §4 allows floating point only as exact IEEE-754 basic operations: platform math such as `sin`, `exp` and `powf`, whose results differ between platforms (use the deterministic functions in bayan-units); the `algebraic_*` methods, which let the compiler reorder or fuse operations (use the plain `+ - * / %` operators); and `min`, `max` and `clamp`, which may return `+0.0` or `-0.0` non-deterministically (compare explicitly, as in `if a > b { a } else { b }`). Arithmetic, `sqrt`, `mul_add`, rounding, sign and comparison methods are exact and allowed. The full list, with the reason for each, is in `clippy.toml`. The lint behind these bans, `clippy::disallowed_methods`, is set to `forbid`, so no code can make an exception: `#[expect]` or `#[allow]` of it, or of a group that contains it such as `clippy::style` or `clippy::all`, fails to compile with error E0453 when Clippy runs. (A plain `cargo build` does not run Clippy and does not notice; the gate's Clippy step does.)
- `HashMap` and `HashSet` are forbidden because their iteration order is random (ADR-0005 §5); use `BTreeMap` and `BTreeSet`.
- `unsafe` code is forbidden everywhere except bayan-ffi and bayan-wasm. There it is still denied by default and allowed only where `#[expect(unsafe_code, reason = "…")]` marks it, and every `unsafe` block needs a `// SAFETY:` comment (ADR-0006 §2).
- Engine code never prints, never calls `unwrap()` outside tests, and never exits the process; documented errors and panics, and explicit numeric conversions, are required. `Cargo.toml` explains each lint.
- Never add a `clippy.toml` or `.clippy.toml` anywhere below the repository root, and never set the `CLIPPY_CONF_DIR` environment variable (also not in the `[env]` table of `.cargo/config.toml`). Clippy does not merge configuration files: for each crate it reads only the nearest one above the crate's folder, so such a file would replace the root `clippy.toml`, bans included, for every crate beneath it. Put every Clippy setting in the root `clippy.toml`; the gate rejects any other configuration.
- Never pass compiler flags that lower lint levels or unlock unstable features: `--cap-lints` (which lowers even `forbid`), `-A`/`--allow`, `--force-warn`, `-Z` options or `@file` arguments, through `RUSTFLAGS` and its relatives or `rustflags` in `.cargo/config.toml`, and never set `RUSTC_BOOTSTRAP`. The preflight rejects them. Other flags, such as `-C target-feature=…` for WebAssembly, are fine.
- Never run the compiler through a wrapper or a replacement program: no `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER` or `RUSTC` (or their `CARGO_BUILD_…` forms), and no `rustc-wrapper`, `rustc-workspace-wrapper` or `rustc` in `[build]` of `.cargo/config.toml`. Such a program sees every compiler call and could weaken the lints for one chosen crate where nothing would notice, so the preflight rejects it. That includes compiler caches such as sccache; unset the variable while you run the gate.
- No crate has a build script (`build.rs`, or `build = "…"` in its manifest) unless `BUILD_SCRIPTS` in `xtask/src/verify.rs` lists it with a reason; the preflight rejects any other. A build script runs code during the build and can change how its crate is compiled, so a build script must never set Clippy or compiler variables through `cargo::rustc-env` (for example `CLIPPY_CONF_DIR`, which would switch off the root `clippy.toml` for that crate), and reviewers read every listed build script, and every change to one, in full.
- The only way to make an exception is `#[expect(lint_name, reason = "why this is safe")]` on the smallest item that needs it. `#[allow]`, and exceptions without a reason, are rejected; an `#[expect]` that is no longer needed fails the build, so exceptions cannot go stale. Clippy's own hint to "add `#[allow(…)]`" does not apply here. The float-method bans have no exceptions, because ADR-0005 §4 allows none, and the compiler enforces that (see the first item). `#[expect(clippy::disallowed_types, reason = "…")]` is only for a hash map or set whose iteration order cannot affect any output, and the reason must say why.

## Adding a crate

1. Create `crates/<name>/` with a `Cargo.toml` like the existing crates' (every `[package]` field taken from the workspace, plus `[lints]` with `workspace = true`), and make sure the crate is in the crate map of `docs/plan/03-architecture.md`.
2. Start `src/lib.rs` with crate documentation that states the crate's responsibility, its layer and the layering rule (copy the wording of an existing crate), followed by `#![forbid(unsafe_code)]`.
3. Depend only on crates of the same layer or below. Declare third-party dependencies once in `[workspace.dependencies]` of the root `Cargo.toml`, with an exact `=x.y.z` version, and justify each in the pull request (ADR-0017).
4. If the crate cannot run in a browser, such as a command-line tool, add it to `NATIVE_ONLY` in `xtask/src/verify.rs` with the reason; otherwise the gate builds it for WebAssembly.
5. Avoid a build script. If the crate really needs one, add it to `BUILD_SCRIPTS` in `xtask/src/verify.rs` with the reason (see "Lint rules and exceptions").
6. Run `cargo xtask verify`.

Spike and Fidelity Lab crates live in `spikes/` and `lab/` and are added to `members` in the root `Cargo.toml` by name; see the READMEs there.

## Toolchain

Rust is pinned to 1.99.0 (released 2026-10-01) in `rust-toolchain.toml`, which is also the minimum supported version (`rust-version` in `Cargo.toml`, ADR-0006 §7). Change it only in the monthly dependency session, to the newest stable release that is at least 24 hours old, in three places at once: `rust-toolchain.toml`, `rust-version` in `Cargo.toml` (the gate checks that they are equal), and `RUST_STABLE` in the docs repository's `scripts/cloud-environment-setup.sh`. Rust 1.100 (2026-11-12) is the first release that enforces `global-min-publish-age`; adopt it in the first session after it is 24 hours old.

## Dependency mechanisms

Exact `=x.y.z` requirements in `[workspace.dependencies]`; `Cargo.lock` committed and every build runs with `--locked`; `.cargo/config.toml` sets `global-min-publish-age = "1 day"` (enforced natively from Rust 1.100). `cargo deny check` runs in the gate on every pull request and every night, with the minimal `deny.toml` from CORE-001: RustSec advisories (yanked versions included), the ADR-0017 license allowlist, and crates.io as the only source. X-003 completes `deny.toml` and adds the lockfile-age and exact-pin checks to the gate. Tools used in CI are pinned and checksum-verified: cargo-deny 0.20.2 in `.github/workflows/verify.yml`, matched by `CARGO_DENY_VERSION` in `xtask/src/verify.rs` (a test keeps them equal); GitHub Actions are pinned to full commit SHAs. xtask itself has no dependencies. Routine upgrades happen only in the monthly dependency session.

In BayanDocs cloud sessions the tools are preinstalled at pinned versions by `docs/scripts/cloud-environment-setup.sh`; run `bayandocs-tools` to list them. If a tool is missing, install the version pinned there (never a newer one) and mention it in the pull request.
