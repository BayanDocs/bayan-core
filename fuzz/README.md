# Fuzz targets

Every parser of untrusted input (ZIP, XML, document parts, fonts, images, PDF, protocol messages) has a fuzz target that feeds it random and mutated input to find crashes and hangs (ADR-0006 §5, ADR-0025 §4). The targets live here. There are none yet; [CORE-005](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-005-bayan-opc.md) and [CORE-006](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-006-bayan-xml.md) add the first ones.

## Rules

- The targets use [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), which needs a nightly Rust toolchain, so this folder is a separate Cargo workspace, excluded from the main one in the root `Cargo.toml`. Use the nightly version pinned in the docs repository's `scripts/cloud-environment-setup.sh`.
- Its `Cargo.lock` is committed, and its dependencies follow the same rules as the main workspace (exact pins, at least 24 hours old, licenses on the allowlist; ADR-0017).
- Record how long each target ran, and on which commit, in the pull request that adds or changes it. An input that found a bug becomes a regression test in the crate it affects.
