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

The Rust engine (a Cargo workspace under `crates/`), the Fidelity Lab tools (`lab/`), fuzz targets (`fuzz/`), time-boxed experiments (`spikes/`, excluded from release artifacts), and the `xtask` automation crate. The crate map and layering are in `docs/plan/03-architecture.md` §4.

## Rules specific to bayan-core

- **Licensing (ADR-0003):** the workspace is GPL-3.0-or-later with the BayanDocs App Store Permission (`GPL-3.0-or-later WITH LicenseRef-BayanDocs-App-Store-Permission`), except `crates/bayan-protocol/`, which is Apache-2.0 so that anyone can implement BayanDocs clients and integrations. That crate must never depend on, or copy from, any GPL or AGPL code; GPL crates may depend on it. `REUSE.toml` records which license applies to which files and `LICENSES/` holds the full texts; keep `reuse lint` passing, and add a new license text only with `reuse download <SPDX-ID>`.
- **Layering:** a crate may depend only on crates in its own layer or below (foundation → model and formats → text → layout → output → interaction → engine → bindings and tools). Never add an upward dependency.
- **Determinism (ADR-0004, ADR-0005):**
  - layout arithmetic uses integer BLU types from `bayan-units`, never floating point;
  - no platform floating-point transcendental functions (`sin`, `exp`, `powf`, …) anywhere in the core; use `bayan-units`;
  - no output that depends on hash-map iteration order (use ordered maps or deterministic hashers);
  - no system time, locale, environment variables, installed fonts or thread scheduling in anything that affects output unless passed in by the host.
- **Safety (ADR-0006):** `#![forbid(unsafe_code)]` everywhere except `bayan-ffi` and `bayan-wasm`; no C or C++ libraries for parsing untrusted input; every parser enforces size, depth and count limits and has a fuzz target; panics never cross the FFI or WebAssembly boundary.
- **No outside world:** the core never opens sockets, reads arbitrary files, or calls platform APIs; it asks the host through the engine protocol's host services (ADR-0012).
- **Active content (ADR-0023):** never fetch external resources referenced by documents, never execute macros, DDE or OLE.
- **Layout changes:** any change that alters layout output for corpus documents needs Fidelity Lab evidence in the pull request, the `fidelity-change` label, and a layout-epoch increment when intentional (ADR-0004, ADR-0025).
- **Lab data:** corpus documents are never committed to Git (only manifests); Microsoft font files or full metric tables never leave the reference machine; private-corpus content never appears in logs, reports or test snapshots.

## Verification gate

`cargo xtask verify` (created by CORE-001). Until CORE-001 has landed there is no code and no gate.

## Dependency mechanisms

Exact `=x.y.z` requirements in `[workspace.dependencies]`; `Cargo.lock` committed and builds run with `--locked`; `.cargo/config.toml` sets `global-min-publish-age = "1 day"` (enforced natively from Rust 1.100); a lockfile-age check and `cargo deny` run in CI (X-003). Routine upgrades happen only in the monthly dependency session.

In BayanDocs cloud sessions the tools are preinstalled at pinned versions by `docs/scripts/cloud-environment-setup.sh`; run `bayandocs-tools` to list them. If a tool is missing, install the version pinned there (never a newer one) and mention it in the pull request.
