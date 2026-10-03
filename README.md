# bayan-core

The engine of [BayanDocs](https://github.com/BayanDocs/docs): a free, open-source word processor built to match Microsoft Word's document fidelity, with identical layout on every platform, local-first editing, and zero-knowledge end-to-end-encrypted collaboration.

bayan-core contains everything that understands documents: the document model, `.docx` (and later RTF, DOC, ODT) import and export, text shaping, layout, rendering, PDF output, editing logic, proofing, collaboration data structures (CRDT) and end-to-end encryption (MLS). It is written in Rust and compiles to native libraries for the [desktop app](https://github.com/BayanDocs/bayan-desktop) and to WebAssembly for the [web app](https://github.com/BayanDocs/bayan-web). It also hosts the Fidelity Lab, which measures BayanDocs against Microsoft Word.

> **Status: Phase 0 (Foundations).** No code yet. The first work package is [CORE-001](https://github.com/BayanDocs/docs/blob/HEAD/workpackages/phase-0/CORE-001-workspace-and-gate.md).

## Where things are decided

- Master plan: [docs/plan](https://github.com/BayanDocs/docs/tree/HEAD/plan)
- Architecture of this repository: [docs/plan/03-architecture.md §4](https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core)
- Decisions: [docs/adr](https://github.com/BayanDocs/docs/tree/HEAD/adr)
- Specifications: [document model](https://github.com/BayanDocs/docs/blob/HEAD/specs/document-model.md), [engine protocol](https://github.com/BayanDocs/docs/blob/HEAD/specs/engine-protocol.md), [Fidelity Lab](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md)
- Work packages: [docs/workpackages](https://github.com/BayanDocs/docs/tree/HEAD/workpackages)

Contributors and agents: start with [AGENTS.md](AGENTS.md).

## License

Planned: MPL-2.0 ([ADR-0003](https://github.com/BayanDocs/docs/blob/HEAD/adr/0003-licensing-and-contribution-model.md), awaiting the owner's confirmation). Until a `LICENSE` file is added, all rights are reserved.
