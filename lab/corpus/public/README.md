# Public corpus v1 (tier T1)

The public corpus is the Fidelity Lab's set of real or realistic `.docx` documents that anyone may download and redistribute ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers)). Version 1, built by work package LAB-001, holds **1,368 documents** (42.5 MiB) from the test suites of seven open-source projects, every one tagged with the coverage-matrix features it uses.

| File | What it is |
|---|---|
| [manifest.json](manifest.json) | The corpus manifest: every document's SHA-256, size, license, provenance and tags ([manifest-schema.md](../manifest-schema.md)). Generated; never edit it by hand. |
| [frequency-report.md](frequency-report.md) | How many documents use each feature, font, script, language and compatibility setting. Generated from the manifest. |
| [excluded.tsv](excluded.tsv) | The 165 documents the review kept out, by SHA-256, each with its reason. |
| [import.sh](import.sh) | Rebuilds the corpus from its sources: the manifest, the report, and a store of the documents. |

The documents themselves are not in Git. Until the public bucket exists ([storage.md](../storage.md) proposes one), build a local copy with the import script, which needs git, curl and Rust (about 120 MB of downloads):

```sh
lab/corpus/public/import.sh ~/bayandocs-corpus/sources ~/bayandocs-corpus/store
bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store ~/bayandocs-corpus/store
```

The import is reproducible: run against the committed manifest, it changes nothing in it (`git status lab/corpus/public` stays clean) and only fills the store. Run without a manifest, it produces the same manifest byte for byte.

## Sources

Each source is a Git repository at a pinned commit. Like every dependency of the project (ADR-0017), each commit was at least 24 hours old when it was pinned. The store keeps each source's license and notice files with the documents, so the corpus can be redistributed with them.

| Source | Repository | Commit (date) | License | Documents |
|---|---|---|---|---:|
| open-xml-powertools | [OfficeDev/Open-Xml-PowerTools](https://github.com/OfficeDev/Open-Xml-PowerTools) | `5881422a881f` (2019-02-04; archived) | MIT | 665 |
| open-xml-sdk | [dotnet/Open-XML-SDK](https://github.com/dotnet/Open-XML-SDK) | `431ab05cf160` (2026-08-18) | MIT | 339 |
| docx4j | [plutext/docx4j](https://github.com/plutext/docx4j) | `0e8e7633ef46` (2026-09-27) | Apache-2.0 | 135 |
| apache-poi | [apache/poi](https://github.com/apache/poi) | `ae62bb5116b9` (2026-10-05) | Apache-2.0 | 119 |
| apache-tika | [apache/tika](https://github.com/apache/tika) | `63284c17c0ee` (2026-10-07) | Apache-2.0 | 48 |
| python-docx | [python-openxml/python-docx](https://github.com/python-openxml/python-docx) | `e45454602b53` (2025-06-16) | MIT | 45 |
| mammoth-js | [mwilliamson/mammoth.js](https://github.com/mwilliamson/mammoth.js) | `791a18923097` (2026-09-26) | BSD-2-Clause | 17 |

A document found in several sources (the same bytes) is one document; it takes the license of the source the import reaches first, in the order of `import.sh`, and the manifest records every place it was found.

## How the documents were chosen

A repository's license covers what its authors could license. Test suites, however, also collect documents that their authors received from others: files attached to bug reports, documents downloaded from the web, excerpts of publications. Their copyright belongs to someone else, and the repository's license may not cover them. So every one of the 1,706 `.docx` files in these seven repositories was reviewed, by its path, its properties and the beginning of its text, before it was admitted, and LAB-001's rule was applied: **when a document's license is unclear, it is excluded.**

- **Kept:** documents the projects made for their tests (synthetic content, "lorem ipsum", feature samples), documents whose content belongs to the project or its sponsor (for example Microsoft's own documents in Microsoft's MIT-licensed repositories, or the OpenDoPE specification in docx4j), and templates and examples written for the project.
- **Excluded for their license (137 documents):** third-party content the repository's license may not cover, such as government and United Nations documents, news articles, contracts, academic papers and theses, book excerpts, Wikipedia text (CC BY-SA, a license the corpus does not allow), a résumé and other documents naming private persons, and documents that state "All rights reserved" or another publisher's copyright inside; plus a contributor license agreement form and a file from Ecma's distribution of the ECMA-376 schemas.
- **Refused by the scanner (28 documents):** encrypted documents and binary `.doc` files, truncated or deliberately damaged archives (fuzzer findings kept by the projects as regression tests), a template, a package whose main part is not a word-processing document, a document with a document type declaration, and a document nested deeper than the scanner's limits. They are listed in the exclusion list too, so the import is exact: any new refusal is an error.

The exclusion list identifies documents by SHA-256, so an excluded document stays out wherever else it is found. `bayan-lab corpus add --exclude excluded.tsv` skips them, `bayan-lab corpus verify --exclude excluded.tsv` fails if one gets in, and a test of bayan-lab checks the committed manifest against the list.

No document of the public corpus may contain personal data beyond what its authors published on purpose; documents that named private persons were excluded. The corpus is still hostile input, like any document: open it only with macros and external content disabled, ideally on the isolated reference machine or in a sandbox ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers), "Corpus hygiene").

## Not included, and why

- **LibreOffice's Writer test documents** (about 2,300 `.docx` files in [LibreOffice/core](https://github.com/LibreOffice/core)) are not included, although the work package brief names them as a likely MPL-2.0 source. The repository carries three license texts (MPL-2.0, LGPL-3.0 and GPL-3.0) and no statement about which applies to its test documents, and many of those documents are attachments to public bug reports, contributed by the reporters rather than written by LibreOffice's developers. Their license is therefore unclear, and the brief says to exclude such sources and raise them as an open question; the pull request that adds this corpus lists the options.
- **Public-domain government documents** and **documents authored for the project** are not in version 1; they are candidates for the corpus growth of LAB-101, each with a recorded provenance.

## Changing the corpus

- **New documents from an existing source** need a new source entry at a newer commit (sources never change): add the source to `import.sh` under a new name, review every new file, extend `excluded.tsv`, run the import, and commit the manifest, the report and the list together.
- **A change to the tagger** that changes tags raises `TAGGER_VERSION`; then run `bayan-lab corpus tag --manifest lab/corpus/public/manifest.json --store …` (or the import from scratch) and commit the new manifest and report.
- **A change to the report's format** needs the report regenerated: `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab --test lab`.
