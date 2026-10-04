# bayan-core

The engine of [BayanDocs](https://github.com/BayanDocs/docs): a free, open-source word processor built to match Microsoft Word's document fidelity, with identical layout on every platform, local-first editing, and zero-knowledge end-to-end-encrypted collaboration.

bayan-core contains everything that understands documents: the document model, `.docx` (and later RTF, DOC, ODT) import and export, text shaping, layout, rendering, PDF output, editing logic, proofing, collaboration data structures (CRDT) and end-to-end encryption (MLS). It is written in Rust and compiles to native libraries for the [desktop app](https://github.com/BayanDocs/bayan-desktop) and to WebAssembly for the [web app](https://github.com/BayanDocs/bayan-web). It also hosts the Fidelity Lab, which measures BayanDocs against Microsoft Word.

> **Status: Phase 0 (Foundations).** The Cargo workspace, the crate skeletons and the verification gate, `cargo xtask verify`, are in place ([CORE-001](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-001-workspace-and-gate.md)); the crates themselves are still empty and are filled in by the next work packages.

## Where things are decided

- Master plan: [docs/plan](https://github.com/BayanDocs/docs/tree/HEAD/plan)
- Architecture of this repository: [docs/plan/03-architecture.md §4](https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core)
- Decisions: [docs/adr](https://github.com/BayanDocs/docs/tree/HEAD/adr)
- Specifications: [document model](https://github.com/BayanDocs/docs/blob/HEAD/specs/document-model.md), [engine protocol](https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md), [Fidelity Lab](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md)
- Work packages: [docs/workpackages](https://github.com/BayanDocs/docs/tree/HEAD/workpackages)

Contributors and agents: start with [AGENTS.md](AGENTS.md).

## License

Licensed under the GNU General Public License v3.0 or later ([LICENSE](LICENSE)) with the [BayanDocs App Store Permission](LICENSES/LicenseRef-BayanDocs-App-Store-Permission.txt), an additional permission under GPLv3 section 7 that allows distribution through app stores as long as the source code stays freely available to everyone. SPDX: `GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission`. Exception: the protocol definitions crate `crates/bayan-protocol/` is licensed Apache-2.0, so anyone can build clients and integrations under any license. [REUSE.toml](REUSE.toml) records which license applies to which files, and [LICENSES/](LICENSES) holds the full texts. See the [licensing FAQ](https://github.com/BayanDocs/docs/blob/HEAD/LICENSING.md) and [ADR-0003](https://github.com/BayanDocs/docs/blob/HEAD/adr/0003-licensing-and-contribution-model.md).
