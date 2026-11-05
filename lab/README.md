# Fidelity Lab

The Fidelity Lab measures how closely BayanDocs matches Microsoft Word ([Fidelity Lab specification](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md), ADR-0004, ADR-0025). Its tools live here: corpus management (LAB-001), the Word ground-truth harness (LAB-002), layout JSON and PDF extraction (LAB-003), comparison metrics and reports (LAB-004), and probes (LAB-005).

| Path | What it is |
|---|---|
| [bayan-lab/](bayan-lab/) | The `bayan-lab` command-line tool and its library (LAB-001): the corpus manifest, content-addressed storage, verification, and feature tagging of `.docx` documents; and the layout JSON commands `pdf-extract` and `layout` (LAB-003). |
| [bayan-lab-layout/](bayan-lab-layout/) | The layout JSON's Rust types, rules and JSON Schema, and the strict PDF reader that extracts a layout JSON from a PDF (LAB-003). A library without input or output of its own, so that the gate's outside-world check covers the parser of untrusted PDFs, and its tests also run in WebAssembly. |
| [schema/layout-json.md](schema/layout-json.md) | The layout JSON, version 1: every field, the text rules that make both sides comparable, how the extractor forms lines, and what it refuses; [schema/layout.schema.json](schema/layout.schema.json) is its generated JSON Schema, and [schema/samples/](schema/samples/) holds sample files of both sides. |
| [corpus/manifest-schema.md](corpus/manifest-schema.md) | The corpus manifest, field by field; [corpus/manifest.schema.json](corpus/manifest.schema.json) is its generated JSON Schema. |
| [corpus/tagging-rules.md](corpus/tagging-rules.md) | What each feature tag means and how the scanner stays safe on hostile files. |
| [corpus/storage.md](corpus/storage.md) | How documents are stored (local, S3-compatible, HTTPS), and the proposal for the public corpus's location. |
| [corpus/public/](corpus/public/README.md) | The public corpus v1 (tier T1): 1,256 documents, their manifest, exclusion list, frequency report, import script and content inspection. |
| [corpus/private-corpus-options.md](corpus/private-corpus-options.md) | Options for the private corpus (tier T2), for the owner's decision. |

## Using bayan-lab

Build it with `cargo build --release -p bayan-lab`; the binary is `target/release/bayan-lab`. `bayan-lab --help` lists the commands:

```text
bayan-lab corpus source add NAME --url URL --revision REV --license SPDX --copyright TEXT --license-file PATH=FILE… [--notes TEXT]
bayan-lab corpus add --source NAME [--tier T1] [--root DIR] [--notes TEXT] [--exclude LIST] FILE…
bayan-lab corpus tag [--all]
bayan-lab corpus verify [--exclude LIST]
bayan-lab corpus list [--feature NAME] [--font NAME] [--script NAME] [--source NAME] [--format text|json]
bayan-lab corpus stats [--format markdown|json] [--title TEXT]
bayan-lab corpus schema
bayan-lab pdf-extract [--output FILE] PDF
bayan-lab layout schema
bayan-lab layout check FILE…
```

Every corpus command takes `--manifest FILE` and, where it reads or writes documents, `--store ADDRESS` (or the environment variables `BAYAN_LAB_MANIFEST` and `BAYAN_LAB_STORE`); options are written `--name VALUE`, and a command refuses words it does not use. `corpus stats` reports on public manifests only, until LAB-101 adds the folding of rare names that a report on private documents needs. The exit status is 0 when everything succeeded, 1 when some documents or checks failed (the messages say which), and 2 when the command could not run.

For example, to list the public corpus's documents that use tracked moves, and to check a local copy of it:

```sh
bayan-lab corpus list --manifest lab/corpus/public/manifest.json --feature "Tracked moves"
bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store ~/bayandocs-corpus/store
```

`pdf-extract` turns a PDF, such as one that Word exported on the reference machine, into a layout JSON ([schema/layout-json.md](schema/layout-json.md)); it exits with 1 when it refuses the PDF (malformed, unsupported or over a limit) and says why, without quoting the document. It reads the PDF within the default limits of `bayan_lab_layout::pdf::Limits`, whose documentation works out what they allow at most: about 4.5 GiB of memory and a few minutes; it writes the JSON as it forms it, so the text of a large layout adds nothing to that. `layout check` checks layout JSON files of any producer against the schema and its rules, and reports each file by its number in the list and a JSON error by its kind, line and column, so that its messages quote neither a file's name nor its text:

```sh
bayan-lab pdf-extract --output word-layout.json word.pdf
bayan-lab layout check word-layout.json
```

## Rules

- Lab tools belong to the **Bindings and tools** layer: they may use every engine crate, and no crate under `crates/` may depend on them (architecture §4).
- Add each lab crate to `members` in the root `Cargo.toml` by name (for example `"lab/bayan-lab"`), so `cargo xtask verify` checks it like every other crate. Do not use a glob such as `"lab/*"`: a folder without a `Cargo.toml` (for example `lab/corpus/`) would break the whole workspace. A lab crate that cannot run in a browser is listed in `NATIVE_ONLY` in `xtask/src/verify.rs`, as bayan-lab is.
- Corpus documents are never committed to Git; only their manifests are. Microsoft font files and full font metric tables never leave the reference machine. Content of the private corpus never appears in logs, reports or test snapshots.
- Every PDF is hostile input too. bayan-lab-layout's reader reads only what extraction needs, within the limits of `bayan_lab_layout::pdf::Limits`, refuses what it does not support (encryption, LZW, vertical writing, optional content and more) instead of guessing or repairing, and has the fuzz target `pdf_extract` in `fuzz/`. A layout JSON contains its document's text: keep the layout of a private document as private as the document.
- Every document is hostile input. bayan-lab's scanner reads the package's structure and the XML of the parts it needs, within limits, and never runs or fetches anything a document contains; it records what it derives (feature names, compatibility options, language tags, character counts per script), not the document's text, but font names and the name of the program that saved a document come from the document and can carry any short text its author chose, so they are limited in length, cleaned of invisible characters, and reported only for public manifests ([corpus/tagging-rules.md](corpus/tagging-rules.md)). Open corpus documents in Word or any other program only with macros and external content disabled, on the isolated reference machine or in a sandbox.
- The scanner's ZIP, package and XML readers (`bayan-lab/src/scan/zip.rs`, `opc.rs` and `xml.rs`) are minimal stand-ins, written before `bayan-opc` (CORE-005) was merged, and marked for replacement: the ZIP and package readers by `bayan-opc`, whose fuzz targets in `fuzz/` cover the same formats, and the XML reader by `bayan-xml` once CORE-006 has built it. Until then they have no fuzz targets of their own. The crafted packages in `bayan-lab/tests/lab/hostile_packages.rs` test the scanner through its public interface, so they stay as acceptance tests for the replacement.
