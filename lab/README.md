# Fidelity Lab

The Fidelity Lab measures how closely BayanDocs matches Microsoft Word ([Fidelity Lab specification](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md), ADR-0004, ADR-0025). Its tools live here: corpus management (LAB-001), the Word ground-truth harness (LAB-002), layout JSON and PDF extraction (LAB-003), comparison metrics and reports (LAB-004), and probes (LAB-005).

| Path | What it is |
|---|---|
| [bayan-lab/](bayan-lab/) | The `bayan-lab` command-line tool and its library (LAB-001): the corpus manifest, content-addressed storage, verification, and feature tagging of `.docx` documents. |
| [corpus/manifest-schema.md](corpus/manifest-schema.md) | The corpus manifest, field by field; [corpus/manifest.schema.json](corpus/manifest.schema.json) is its generated JSON Schema. |
| [corpus/tagging-rules.md](corpus/tagging-rules.md) | What each feature tag means and how the scanner stays safe on hostile files. |
| [corpus/storage.md](corpus/storage.md) | How documents are stored (local, S3-compatible, HTTPS), and the proposal for the public corpus's location. |
| [corpus/public/](corpus/public/README.md) | The public corpus v1 (tier T1): 1,368 documents, their manifest, exclusion list, frequency report and import script. |
| [corpus/private-corpus-options.md](corpus/private-corpus-options.md) | Options for the private corpus (tier T2), for the owner's decision. |

## Using bayan-lab

Build it with `cargo build --release -p bayan-lab`; the binary is `target/release/bayan-lab`. `bayan-lab --help` lists the commands:

```text
bayan-lab corpus source add NAME --url URL --revision REV --license SPDX --copyright TEXT --license-file PATH=FILE…
bayan-lab corpus add --source NAME [--tier T1] [--root DIR] [--exclude LIST] FILE…
bayan-lab corpus tag [--all]
bayan-lab corpus verify [--exclude LIST]
bayan-lab corpus list [--feature NAME] [--font NAME] [--script NAME] [--source NAME] [--format text|json]
bayan-lab corpus stats [--format markdown|json] [--title TEXT]
bayan-lab corpus schema
```

Every command takes `--manifest FILE` and, where it reads or writes documents, `--store ADDRESS` (or the environment variables `BAYAN_LAB_MANIFEST` and `BAYAN_LAB_STORE`). The exit status is 0 when everything succeeded, 1 when some documents or checks failed (the messages say which), and 2 when the command could not run.

For example, to list the public corpus's documents that use tracked moves, and to check a local copy of it:

```sh
bayan-lab corpus list --manifest lab/corpus/public/manifest.json --feature "Tracked moves"
bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store ~/bayandocs-corpus/store
```

## Rules

- Lab tools belong to the **Bindings and tools** layer: they may use every engine crate, and no crate under `crates/` may depend on them (architecture §4).
- Add each lab crate to `members` in the root `Cargo.toml` by name (for example `"lab/bayan-lab"`), so `cargo xtask verify` checks it like every other crate. Do not use a glob such as `"lab/*"`: a folder without a `Cargo.toml` (for example `lab/corpus/`) would break the whole workspace. A lab crate that cannot run in a browser is listed in `NATIVE_ONLY` in `xtask/src/verify.rs`, as bayan-lab is.
- Corpus documents are never committed to Git; only their manifests are. Microsoft font files and full font metric tables never leave the reference machine. Content of the private corpus never appears in logs, reports or test snapshots.
- Every document is hostile input. bayan-lab's scanner reads only package structure and element names, within limits, and never runs or fetches anything a document contains ([corpus/tagging-rules.md](corpus/tagging-rules.md)). Open corpus documents in Word or any other program only with macros and external content disabled, on the isolated reference machine or in a sandbox.
- The scanner's ZIP and XML readers (`bayan-lab/src/scan/zip.rs` and `xml.rs`) are minimal stand-ins, marked for replacement by `bayan-opc` (CORE-005) and `bayan-xml` (CORE-006) once those exist.
