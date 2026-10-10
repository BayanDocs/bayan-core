# Fuzz targets

Every parser of untrusted input (ZIP, XML, document parts, fonts, images, PDF, protocol messages) has a fuzz target that feeds it random and mutated input to find crashes and hangs (ADR-0006 §5, ADR-0025 §4). The targets live here.

## Targets

| Target | What it fuzzes | Properties it checks besides "no crash, no hang" | Work package |
|---|---|---|---|
| `zip_archive` | bayan-opc's ZIP reader | decompressing an entry into memory and verifying it through a window agree; an entry copied unchanged into a new archive reads back the same | CORE-005 |
| `package` | opening a whole package: ZIP, part names, content types, relationships, core properties | writing an unmodified package gives the same parts with byte-identical stored data, and fails exactly when a part is damaged | CORE-005 |
| `content_types` | the content types stream and the minimal XML parser under it | unchanged, it is written back byte for byte; changed, it parses again with the change; the change undone gives the input again | CORE-005 |
| `relationships` | relationships parts and the minimal XML parser under it | the same three properties, with a relationship added and removed | CORE-005 |
| `core_properties` | the core properties part | the properties written and read back are the same | CORE-005 |
| `compound_file` | OLE compound files through bayan-opc's API and the cfb crate | every stream that reads has its listed size | CORE-005 |

## Running them

The targets use [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), which needs a nightly Rust toolchain, so this folder is a separate Cargo workspace, excluded from the main one in the root `Cargo.toml`. Use the nightly version and the cargo-fuzz version pinned in the docs repository's `scripts/cloud-environment-setup.sh` (`RUST_NIGHTLY`, today `nightly-2026-10-02`, and cargo-fuzz 0.13.2); BayanDocs cloud sessions have both. From the repository's root:

```sh
cargo +nightly-2026-10-02 fuzz build --target x86_64-unknown-linux-gnu
cargo +nightly-2026-10-02 fuzz run --target x86_64-unknown-linux-gnu zip_archive fuzz/corpus/zip_archive -- -max_total_time=600 -dict=fuzz/dictionaries/zip.dict -rss_limit_mb=2048 -timeout=10
```

`--target` names the host explicitly because the cloud environment's cargo-fuzz is a statically linked (musl) build that would otherwise build for its own target. Use `-dict=fuzz/dictionaries/xml.dict` for the targets that read XML. A good starting corpus is the real documents of bayan-opc's tests: copy `crates/bayan-opc/tests/fixtures/python-docx/*.docx` into `fuzz/corpus/zip_archive/` and `fuzz/corpus/package/`, and the metadata parts inside them (`[Content_Types].xml`, the `.rels` parts, `docProps/core.xml`) into the corpora of the XML targets. The corpora, crash artifacts and build output stay local (`.gitignore`).

## Rules

- Its `Cargo.lock` is committed, and its dependencies follow the same rules as the main workspace: exact pins, at least 24 hours old, licenses on the allowlist (ADR-0017). Resolve it with the pinned nightly, which enforces the 24-hour minimum age (`cargo +nightly-2026-10-02 update` in this folder).
- **License check:** `cargo xtask verify` checks this workspace's dependencies with cargo-deny against its own `deny.toml`: the main policy plus one exception, the NCSA license of libFuzzer that libfuzzer-sys bundles, which ADR-0017's amendment of 2026-10-08 accepts for fuzzing dependencies that never ship. The exception is for that one crate and never goes into the main workspace's `deny.toml` (an xtask test keeps it so).
- **Age check, by hand for now:** `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age` read the main workspace only. They cannot read this one yet, because they refuse path dependencies on crates outside the workspace being checked, and every target depends on `../crates/bayan-opc`. Until they learn to (a change to `xtask/src/supply_chain/`, which is identical in bayan-server, so both change together), whoever changes this `Cargo.lock` checks the publish time and checksum of every package version it adds, as the pull request of CORE-005 did for all of them, and states them in the pull request.
- Record how long each target ran, and on which commit, in the pull request that adds or changes it. An input that found a bug becomes a regression test in the crate it affects.
- libfuzzer-sys compiles LLVM's libFuzzer (C++) in its build script. That is acceptable here because nothing in this workspace is linked into, or shipped with, BayanDocs; the parsers under test stay pure Rust (ADR-0006).
