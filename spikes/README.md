# Spikes

A spike is a time-boxed experiment that answers a design question before production code depends on the answer, for example [CORE-003](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-003-spike-deterministic-text.md) (a deterministic text pipeline) and [CORE-004](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-004-spike-crdt-document-model.md) (the document model on Loro). Each spike gets its own folder, `spikes/<name>/`, holding its code (if any) and its report, `REPORT.md`.

## Spikes are excluded from release builds

- No crate under `crates/` may depend on a spike crate, so spike code can never reach a release. Release artifacts are built from bayan-ffi, bayan-wasm and bayan-cli and their dependencies only.
- Code that proves itself moves into a crate under `crates/` through a work package, with tests and review.

## Spikes are still checked

Spike code follows the same rules as everything else in this repository: dependencies follow ADR-0017, and the lints forbid unsafe code and platform floating-point math. To make sure, add a spike crate to `members` in the root `Cargo.toml` by name (for example `"spikes/text-pipeline"`); `cargo xtask verify` then formats, lints, tests, builds for WebAssembly and audits it like every other crate. Do not use a glob such as `"spikes/*"`: a folder that holds only a report has no `Cargo.toml`, and a glob that matches it breaks the whole workspace.
