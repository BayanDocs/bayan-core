# Fidelity Lab

The Fidelity Lab measures how closely BayanDocs matches Microsoft Word ([Fidelity Lab specification](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md), ADR-0004, ADR-0025). Its tools live here: corpus management (LAB-001), the Word ground-truth harness (LAB-002), layout JSON and PDF extraction (LAB-003), comparison metrics and reports (LAB-004), and probes (LAB-005). There is nothing here yet; [LAB-001](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/LAB-001-corpus-infrastructure.md) adds the first tool.

## Rules

- Lab tools belong to the **Bindings and tools** layer: they may use every engine crate, and no crate under `crates/` may depend on them (architecture §4).
- Add each lab crate to `members` in the root `Cargo.toml` by name (for example `"lab/bayan-lab"`), so `cargo xtask verify` checks it like every other crate. Do not use a glob such as `"lab/*"`: a folder without a `Cargo.toml` (for example `lab/schema/`) would break the whole workspace.
- Corpus documents are never committed to Git; only their manifests are. Microsoft font files and full font metric tables never leave the reference machine. Content of the private corpus never appears in logs, reports or test snapshots.
