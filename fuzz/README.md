# Fuzz targets

Every parser of untrusted input (ZIP, XML, document parts, fonts, images, PDF, protocol messages) has a fuzz target that feeds it random and mutated input to find crashes and hangs (ADR-0006 §5, ADR-0025 §4). The targets live here, and CI runs all of them on every pull request that can affect them and every night (see [In CI](#in-ci)).

## Targets

| Target | What it fuzzes | Properties it checks besides "no crash, no hang" | Work package |
|---|---|---|---|
| `zip_archive` | bayan-opc's ZIP reader | decompressing an entry into memory and verifying it through a window agree; an entry copied unchanged into a new archive reads back the same | CORE-005 |
| `package` | opening a whole package: ZIP, part names, content types, relationships, core properties | writing an unmodified package gives the same parts with byte-identical stored data, and fails exactly when a part is damaged | CORE-005 |
| `content_types` | the content types stream and the minimal XML parser under it | unchanged, it is written back byte for byte; changed, it parses again with the change; the change undone gives the input again | CORE-005 |
| `relationships` | relationships parts and the minimal XML parser under it | the same three properties, with a relationship added and removed | CORE-005 |
| `core_properties` | the core properties part | the properties written and read back are the same | CORE-005 |
| `compound_file` | OLE compound files through bayan-opc's API and the cfb crate | every stream that reads has its listed size | CORE-005 |
| `zip_archive_default_limits` | `zip_archive` with bayan-opc's default limits (`Limits::DEFAULT`) instead of the smaller ones that keep `zip_archive` quick, on inputs of up to 1 MiB | the same as `zip_archive` | bayan-core#21 |
| `package_default_limits` | `package` with bayan-opc's default limits, on inputs of up to 1 MiB, so that inputs can reach the shapes that the default limits must hold in memory | the same as `package` | bayan-core#21 |
| `xml_reader` | bayan-xml's reader, writer, fragments and canonical form | inside the root element the spans of the events follow each other without gap; decoding never makes text longer; written again from its events, a document keeps its canonical form, and the writer's output written again is the same bytes; the captured root element is its text; every child of the root, moved into a context that binds the prefixes it uses to another namespace, keeps its meaning | CORE-006 |
| `markup_compatibility` | bayan-xml's Markup Compatibility processing, with a configuration chosen by the first byte | the lossless events recreate the input; every fragment it reports stands on its own; a captured element reads again in its context; the output document, processed again, does not change | CORE-006 |

## Running them

The targets use [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), which needs a nightly Rust toolchain, so this folder is a separate Cargo workspace, excluded from the main one in the root `Cargo.toml`. Use the nightly version and the cargo-fuzz version pinned in the docs repository's `scripts/cloud-environment-setup.sh` (`RUST_NIGHTLY`, today `nightly-2026-10-02`, and cargo-fuzz 0.13.2), which CI uses too; BayanDocs cloud sessions have both. From the repository's root:

```sh
cargo +nightly-2026-10-02 fuzz build --target x86_64-unknown-linux-gnu
cargo +nightly-2026-10-02 fuzz run --target x86_64-unknown-linux-gnu zip_archive fuzz/corpus/zip_archive -- -max_total_time=600 -dict=fuzz/dictionaries/zip.dict -rss_limit_mb=2048 -timeout=10
```

`--target` names the host explicitly because cargo-fuzz's release build is statically linked (musl) and would otherwise build for its own target. Use `-dict=fuzz/dictionaries/xml.dict` for bayan-opc's targets that read XML, and `-dict=fuzz/dictionaries/ooxml.dict` (WordprocessingML and Markup Compatibility) for bayan-xml's. To run the targets the way CI does, with its seeds, settings and summary, use `ci.py` (Python 3.11 or later, standard library only) after building them:

```sh
python3 -I fuzz/ci.py seeds fuzz/seeds
python3 -I fuzz/ci.py run --all --seconds 150 --seeds fuzz/seeds
python3 -I fuzz/ci.py minimize --all
```

`run` also takes target names instead of `--all`, and `--jobs` for how many targets run at the same time (default 4). The corpora go to `fuzz/corpus/<target>/`, where `cargo fuzz run` finds them too, the inputs that fail to `fuzz/artifacts/<target>/`, and each target's output to `fuzz/logs/`. The seeds, corpora, crash artifacts, logs and build output stay local (`.gitignore`).

## In CI

`.github/workflows/fuzz.yml` builds every target with AddressSanitizer and runs them with `ci.py`, which holds each target's settings in `TARGETS` and refuses to run when a target of `Cargo.toml` has none, so that no target is left out:

- **Pull requests** that change bayan-opc, bayan-xml, this folder, the root `Cargo.toml`, `.cargo/` or the workflow: every target runs for 150 seconds (the default-limits targets 300), four at a time, from the seeds and from the corpus that the last nightly run saved. A pull request only reads that corpus and never saves one. Starting from it, rather than from the seeds alone, replays every input that the nightly runs have found, and gives `compound_file`, which has no seed, a corpus to start from: from nothing, it reached 56 edges of coverage in 20 seconds and 533 in 4 minutes, where CORE-005's seeded runs reached 1,606. The job takes about 10 minutes. It is not a required check, because a path filter skips it on most pull requests; look at it before merging a pull request that touches a parser.
- **What the shared corpus means for pull requests**: an input in the nightly corpus passed when it was saved. If a later change on `main` makes it crash, every pull request that touches the fuzzed crates fails on it, as the nightly run does, until the change is fixed; any crash is a release blocker (ADR-0025 §4). A crash that a run finds is never saved in the corpus: libFuzzer keeps it apart, in the uploaded artifact. A pull request therefore fails on a crash that a nightly run found only if its own fuzzing finds that crash again, while the next nightly runs, which start from the same corpus, are likely to find it again until it is fixed.
- **Every night** (02:23 UTC), and on demand (Actions → Fuzzing → Run workflow): every target runs for 15 minutes (the default-limits targets 30) from the corpus that the previous run left. Then each corpus is minimized (libFuzzer's `-merge=1` keeps only the inputs its coverage needs) and saved with actions/cache for the next run, so the corpora grow from run to run without keeping inputs they do not need. Only these runs save a corpus, into the cache of the branch they ran on: scheduled runs on `main`, and manual runs on the branch they were started on. A run reads the latest corpus of its own branch, or else of `main`, and a pull request that of its base branch or of `main`, so a pull request against `main` reads only what runs on `main` saved. The first run on a branch starts from the seeds alone if `main` has no corpus yet.
- **Failures**: a crash (a panic included), a timeout, running out of memory or a leak fails the job, which then uploads the inputs that failed and every target's log as the artifact `fuzzing-failures` (kept for 30 days). The job summary lists every target's result, how long it ran, its executions, coverage (edges and features), corpus and peak memory, and after a nightly run the coverage of each minimized corpus. That coverage leaves out what only the seeds reach: libFuzzer never copies the seeds into the corpus, and every run reads them again.
- **Notifications**: GitHub notifies whoever last changed the workflow's cron schedule when a scheduled run fails. Agents push and merge under the owner's account, so that is the owner, by email and on github.com as the owner's notification settings for Actions say (Settings → Notifications → Actions). GitHub switches scheduled workflows off after 60 days without activity in the repository; whoever switches one on again receives its notifications from then on.

### Settings

| Targets | Seeds | Dictionary | Inputs up to (`-max_len`) | Timeout of an input | Time on a pull request / at night |
|---|---|---|---|---|---|
| `zip_archive`, `package` | the five python-docx documents of bayan-opc's tests | `zip.dict` | 64 KiB | 10 s | 150 s / 15 min |
| `content_types`, `relationships`, `core_properties` | the parts of those documents that each reads (5, 9 and 5) | `xml.dict` | 64 KiB | 10 s | 150 s / 15 min |
| `compound_file` | none: the documents contain no compound file, so it starts from the nightly corpus, which grows night by night | none | 64 KiB | 10 s | 150 s / 15 min |
| `zip_archive_default_limits`, `package_default_limits` | the five documents | `zip.dict` | 1 MiB, from the start (`-len_control=0`) | 60 s | 300 s / 30 min |
| `xml_reader` | every XML part of the documents up to 64 KiB, and bayan-xml's two sample parts: 63 files | `ooxml.dict` | 64 KiB | 10 s | 150 s / 15 min |
| `markup_compatibility` | the same, each after six configuration bytes (0x00, 0x08, 0x38, 0x48, 0x78 and 0xFF): 378 files | `ooxml.dict` | 64 KiB | 10 s | 150 s / 15 min |

Every target may take 2,048 MiB (`-rss_limit_mb`), as in the runs of CORE-005 and CORE-006; the next section explains why that suits the default-limits targets too.

The seeds come only from public, licensed files of this repository, written by `ci.py seeds`: never add documents of the private corpus, because everyone who can read this repository's Actions can download what a run uploads.

Without `-len_control=0`, libFuzzer lengthens its inputs slowly, starting from the longest input of the corpus: in a ten-minute trial the default-limits targets made no input longer than 39 KB, although they may make inputs of 1 MiB. With it, they passed 64 KB within four minutes, and the nightly corpus keeps the longer inputs that reach new code.

The default-limits targets get a longer timeout because an input that fills bayan-opc's default metadata budgets takes seconds under AddressSanitizer: the worst metadata of the memory measurements below took 12 s in `package_default_limits`, which reads such a package twice (and the 34 MB combination 24 s, but its entry list alone needs 10 MB of ZIP headers, more than an input can hold). A 1 MiB input can hold metadata like the former, since bayan-opc lets a part compress up to 100 to 1. 60 s is five times that, while work that grows faster than its input takes minutes.

### The memory limit of the default-limits targets

bayan-opc documents a memory bound: with the default limits, opening a package and reading its core properties take at most about 256 MiB (`crates/bayan-opc/src/lib.rs`, "Limits"; derived from two measured worst cases, 66 MiB of entries and 149 MiB of metadata). The limit must let a package that needs that much pass, AddressSanitizer's overhead included, and fail the run when an input needs far more. These are the worst packages of that bound's derivation (CORE-005's pull request, built with the generators posted there), run once each, on `bc903b2`:

| Package | Opened, with its core properties, natively | The same under AddressSanitizer | `package_default_limits` under AddressSanitizer |
|---|---|---|---|
| The worst metadata: a content types stream of long CJK part names in UTF-16, then a relationships part with 217,132 elements and a long CJK target (16 MB, stored) | 149 MiB | 424 MiB | 659 MiB |
| The same with the long target before the elements | 145 MiB | 423 MiB | 660 MiB |
| The worst list of entries: 9,999 relationships parts with CJK names of about 1 KiB whose source parts do not exist (22 MB) | 66 MiB | 352 MiB | 756 MiB |
| The worst combination found: those entries, then the worst metadata (34 MB) | 202 MiB | 646 MiB | 1,082 MiB |
| The same, with the core properties part holding the metadata | 202 MiB | 640 MiB | 932 MiB |
| A one-byte input (libFuzzer alone) | | 33 MiB | 33 MiB |

The figures are the peak resident memory added by opening (native column, measured with the generators' own program, in a release build; it agrees with CORE-005's tables within a few MiB) or of the whole process, the package's own bytes included (the other two columns). The middle column used a scratch target that only opens the package with the default limits and reads its core properties; the last is the target as CI builds it. AddressSanitizer takes 2.5 to 4.5 times the native memory (a guard zone around every allocation, freed memory kept back for a while to catch late uses, and its shadow memory), the most for the entry list's many small allocations. `package_default_limits` takes more again, because it also reads every part, writes the package again and opens the copy while the original is still open. 2,048 MiB leaves almost half of it free above the worst of these packages, for libFuzzer's corpus and AddressSanitizer's quarantine, while an input that needs about three times the documented bound to be opened once, or about one and a half times for each of the target's two openings, exceeds it and fails the run as out of memory. `zip_archive_default_limits` reads every entry into memory instead, which the archive's compression ratio limits to 100 times the input: its peak on the same packages was 426 MiB. (Each figure varied by a few MiB between runs; the last column's were measured three times on the final code.)

### When a run fails

1. Open the failed run (Actions → Fuzzing, or the link in GitHub's notification). In the step "Fuzz every target", the failing target's report is printed unfolded after its line: the panic message and its stack trace, or libFuzzer's report of a timeout, of running out of memory or of a leak. Every target's whole log is folded under its line.
2. Download the artifact `fuzzing-failures` from the run's summary page. It holds `artifacts/<target>/` with the input that failed, named by its SHA-1 and by what went wrong (`crash-…`, `timeout-…`, `oom-…` or `leak-…`), and `logs/` with every target's log.
3. On the commit that the run tested, build the targets and run the one that failed on that input, with the target's options from the settings above for a timeout or out of memory. `cargo fuzz tmin` then shrinks the input to the smallest one that still fails:

   ```sh
   cargo +nightly-2026-10-02 fuzz build --target x86_64-unknown-linux-gnu
   cargo +nightly-2026-10-02 fuzz run --target x86_64-unknown-linux-gnu package_default_limits path/to/crash-<sha1> -- -rss_limit_mb=2048 -timeout=60
   cargo +nightly-2026-10-02 fuzz tmin --target x86_64-unknown-linux-gnu package_default_limits path/to/crash-<sha1>
   ```

4. If the failure is a security problem, which is any crash, hang or runaway memory that a document could cause in BayanDocs, report it privately, as `SECURITY.md` says, and delete the artifact from the run's page once you have it, since anyone who can read the Actions can download it until then. Any crash is a release blocker (ADR-0025 §4).
5. Fix it in a pull request of its own, with a regression test in the crate it affects (see Rules).

## Rules

- Its `Cargo.lock` is committed, and its dependencies follow the same rules as the main workspace: exact pins, at least 24 hours old, licenses on the allowlist (ADR-0017). Resolve it with the pinned nightly, which enforces the 24-hour minimum age (`cargo +nightly-2026-10-02 update` in this folder). CI builds the targets from it with `cargo fetch --locked` and fails if building would change it.
- **License check:** `cargo xtask verify` checks this workspace's dependencies with cargo-deny against its own `deny.toml`: the main policy plus one exception, the NCSA license of libFuzzer that libfuzzer-sys bundles, which ADR-0017's amendment of 2026-10-08 accepts for fuzzing dependencies that never ship. The exception is for that one crate and never goes into the main workspace's `deny.toml` (an xtask test keeps it so).
- **Exact pins and age check:** `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age`, the first step of `cargo xtask verify` and of the CodeQL workflow's Rust job, read this workspace like the main one (bayan-core#22). Every dependency of the targets is pinned exactly, and every package version that a change adds to this `Cargo.lock`, compared with this file at the merge base, must have been published on crates.io at least 24 hours before the commit that added it, with the checksum crates.io published. This workspace may depend by path on members of the main workspace, such as bayan-opc and bayan-xml, whose own dependencies are checked with the main workspace, and on nothing else outside this folder; their dependencies' versions in this `Cargo.lock` are checked with it. No hand check is needed any more.
- **Every target runs in CI:** a new target needs its settings in `TARGETS` of `ci.py`; the fuzzing workflow fails until it has them (`test_ci.py` checks it, and `ci.py run --all` refuses to run without them). `zip_archive_default_limits` and `package_default_limits` are copies of `zip_archive` and `package` that differ only in their limits and description (compare them with `diff`): change each pair together.
- **The package targets read their written copy back with raised limits**, because `PackageWriter` does not apply the reader's limits (its documentation in `crates/bayan-opc/src/package.rs`): each limit that writing a package unchanged can exceed is raised by exactly what the writer can add, which the comment at that reading derives. BayanDocs/bayan-core#27 makes the writer keep within the reader's limits; it then reads the copy back with the original limits again.
- Record how long each target ran, and on which commit, in the pull request that adds or changes it. An input that found a bug becomes a regression test in the crate it affects.
- libfuzzer-sys compiles LLVM's libFuzzer (C++) in its build script. That is acceptable here because nothing in this workspace is linked into, or shipped with, BayanDocs; the parsers under test stay pure Rust (ADR-0006).
