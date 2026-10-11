# Public corpus v1 (tier T1)

The public corpus is the Fidelity Lab's set of real or realistic `.docx` documents that anyone may download and redistribute ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers)). Version 1, built by work package LAB-001, holds **1,256 documents** (23.1 MiB) from the test suites of seven open-source projects, every one tagged with the coverage-matrix features it uses.

| File | What it is |
|---|---|
| [manifest.json](manifest.json) | The corpus manifest: every document's SHA-256, size, license, provenance and tags ([manifest-schema.md](../manifest-schema.md)). Generated; never edit it by hand. |
| [frequency-report.md](frequency-report.md) | How many documents use each feature, font, script, language and compatibility setting. Generated from the manifest. |
| [excluded.tsv](excluded.tsv) | The 277 documents the review kept out, by SHA-256, each with its reason. |
| [import.sh](import.sh) | Rebuilds the corpus from its sources: the manifest, the report, and a store of the documents. |
| [inspect-content.py](inspect-content.py) | The automated part of the content review: reports what a curator must judge in every document of a store (see below). |

The documents themselves are not in Git. Until the public bucket exists ([storage.md](../storage.md) describes it), build a local copy with the import script, which needs git, curl and Rust (about 120 MB of downloads):

```sh
lab/corpus/public/import.sh ~/bayandocs-corpus/sources ~/bayandocs-corpus/store
bayan-lab corpus verify --manifest lab/corpus/public/manifest.json --store ~/bayandocs-corpus/store
```

The import is reproducible: run against the committed manifest, it changes nothing in it (`git status lab/corpus/public` stays clean) and only fills the store. Run without a manifest, it produces the same manifest byte for byte.

## Sources

Each source is a Git repository at a pinned commit. Like every dependency of the project (ADR-0017), each commit was at least 24 hours old when it was pinned. The store keeps each source's license and notice files with the documents, so the corpus can be redistributed with them.

| Source | Repository | Commit (date) | License | Documents |
|---|---|---|---|---:|
| open-xml-powertools | [OfficeDev/Open-Xml-PowerTools](https://github.com/OfficeDev/Open-Xml-PowerTools) | `5881422a881f` (2019-02-04; archived) | MIT | 649 |
| open-xml-sdk | [dotnet/Open-XML-SDK](https://github.com/dotnet/Open-XML-SDK) | `431ab05cf160` (2026-08-18) | MIT | 289 |
| docx4j | [plutext/docx4j](https://github.com/plutext/docx4j) | `0e8e7633ef46` (2026-09-27) | Apache-2.0 | 116 |
| apache-poi | [apache/poi](https://github.com/apache/poi) | `ae62bb5116b9` (2026-10-05) | Apache-2.0 | 105 |
| python-docx | [python-openxml/python-docx](https://github.com/python-openxml/python-docx) | `e45454602b53` (2025-06-16) | MIT | 42 |
| apache-tika | [apache/tika](https://github.com/apache/tika) | `63284c17c0ee` (2026-10-07) | Apache-2.0 | 38 |
| mammoth-js | [mwilliamson/mammoth.js](https://github.com/mwilliamson/mammoth.js) | `791a18923097` (2026-09-26) | BSD-2-Clause | 17 |

A document found in several sources (the same bytes) is one document; it takes the license of the source the import reaches first, in the order of `import.sh`, and the manifest records every place it was found.

## How the documents were chosen

A repository's license covers what its authors could license. Test suites, however, also collect documents that their authors received from others: files attached to bug reports, documents downloaded from the web, excerpts of publications, pictures found online. Their copyright belongs to someone else, and the repository's license may not cover them. LAB-001's rule is therefore applied to every one of the 1,706 `.docx` files in these seven repositories: **when a document's license is unclear, it is excluded.** Documents with personal data are excluded too.

### What was inspected

1. **Every file by its path, its properties and the beginning of its text** (the first review).
2. **Every part of every admitted document, automatically**, with [inspect-content.py](inspect-content.py) (the second review, after an independent review found what the first had missed, and its re-check):
   - the text of every part: the main story, headers, footers, footnotes, endnotes, comments, text boxes, the glossary, the document properties, the relationship targets, the text and metadata of SVG pictures, HTML and plain-text parts (such as the HTML that an `altChunk` brings into a document) and the text strings of parts of any other kind, against lists of markers (copyright, "all rights reserved" and confidentiality notices, other licenses, news agencies and picture libraries, standards bodies and the wording of ECMA-376's own definitions, publishers, résumé and business templates, Office.com template identifiers), and for e-mail addresses, bank account and payment card numbers, and user folders in paths. Each part is searched twice: as it is stored, and with each paragraph's text runs (and field instructions) joined, because Word splits a word into several runs wherever its formatting, revision marks or proofing marks change, so that "Confidential" can be stored as "Confidentia" and "l" (the re-check found a "Microsoft Confidential" footer that way, and searching HTML parts found a public authority's procurement document that a test file brought in as an `altChunk`);
   - every embedded object, by its content rather than its name, listed for every document that has one: programs (Windows, Linux and macOS executables, and package files of executable types), e-mail messages, PDFs (also inside Acrobat's OLE objects), legacy Office files (their text strings) and embedded Office documents (inspected in turn, like the document itself, three levels deep; the two packages nested deeper, in a recursive-embedding test, were opened by hand);
   - every picture's metadata: camera, author and copyright fields, GPS positions, and picture-library credits in EXIF, IPTC, XMP and PNG text;
   - every embedded font: complete or a subset, its vendor, its embedding permission and its copyright notice;
   - every signature certificate: whom it names (as field types, never the names themselves) and who issued it;
   - the people the metadata names, counted but never shown: the names in the properties, comments and tracked changes, and the presence information in `word/people.xml`, by the kind of account it identifies.
3. **Every picture, by eye:** every distinct raster and vector picture of the documents, small ones included, and the documents' thumbnails, which show their first page.
4. **The eight PDFs embedded in seven documents**, by their extracted text and their properties: all hold test sentences ("This is a test pdf", repeated test words, keyboard-mash text with three first names), and their properties name their authors.

The script prints names, addresses and numbers masked, so its report can be shared; run it as `python3 -I lab/corpus/public/inspect-content.py STORE lab/corpus/public/manifest.json`. It uses only Python's standard library, treats every document as hostile (it limits what it reads and how deeply it opens embedded packages), and decides nothing: the curator does, and records the decision in `excluded.tsv`. What it cannot judge, a person still must: whether text that names nobody and carries no notice belongs to someone else, and what a picture shows.

### The rules applied

- **Kept:** documents the projects made for their tests (synthetic content, "lorem ipsum", feature samples), documents whose content belongs to the project or its sponsor (for example Microsoft's own documents and templates in Microsoft's MIT-licensed repositories, the OpenDoPE specification in docx4j, or the OpenXML4J test files that came to Apache POI with OpenXML4J, made at the company whose logo they carry, as their properties say), templates and examples written for the project, screenshots of the programs they discuss, and short quotations with their source inside a project's own text (docx4j's guide quotes one sentence of Wikipedia).
- **Excluded for their license (211 documents):**
  - third-party text the repository's license may not cover: government and United Nations documents, news and news-agency articles, contracts, academic papers and theses, book excerpts, Wikipedia articles (CC BY-SA, a license the corpus does not allow), templates of other publishers (a business plan template, résumé templates, a telecom operator's letter template), verbatim excerpts of standards (ECMA-376 and others; documents that discuss a standard in their own words stay), a contributor license agreement form, and a file from Ecma's distribution of the ECMA-376 schemas;
  - documents marked "All rights reserved", "Microsoft Confidential" or with another publisher's copyright inside, even where the repository's own owner wrote the marking;
  - **documents that embed programs, always** (one embedded two Windows programs of other companies), and documents that embed complete font files (Microsoft and Monotype fonts; the owner's decision of 2026-10-10; two documents with font subsets keep "Embedded fonts" covered);
  - pictures whose origin the review could not establish, or whose owners did not license them (68 documents): photographs (among them space agency material and Windows sample pictures credited to photographers and picture agencies), drawings and clip art, a figure from a research publication, cartoon characters, a film poster, maps, and other organizations' logos and icons.
- **Excluded for personal data (38 documents):** signature certificates of real people (five from national identity cards, one of a company's employee), e-mail addresses of real people (the comment authors of 23 Open XML SDK samples, another company's employee, and the senders and recipients of two embedded e-mail messages), pictures of people, and a picture of a comment list with its author's name and e-mail alias. A source's maintainers' own contact addresses, published on purpose in their own documents (docx4j's), stay.
- **Refused by the scanner (28 documents):** encrypted documents and binary `.doc` files, truncated or deliberately damaged archives (fuzzer findings kept by the projects as regression tests), a template, a package whose main part is not a word-processing document, a document with a document type declaration, and a document nested deeper than the scanner's limits. They are listed in the exclusion list too, so the import is exact: any new refusal is an error.

The exclusion list identifies documents by SHA-256, so an excluded document stays out wherever else it is found. `bayan-lab corpus add --exclude excluded.tsv` skips them, `bayan-lab corpus verify --exclude excluded.tsv` fails if one gets in, and a test of bayan-lab checks the committed manifest against the list.

**Bug-report files.** Apache POI and Apache Tika keep files from their issue trackers among their test documents, and docx4j some from its users. Their license is clear: the Apache License 2.0 counts as a *Contribution* anything sent to the licensor through its issue tracking systems for discussing and improving the work (§1), and licenses every Contribution under the same terms unless its submitter says otherwise (§5). The submitter can only license what is theirs, though, so these files went through the same content review as every other; most exclusions above are bug-report files. LibreOffice's test documents have no such clause behind them (see below).

### What the review accepted

The inspection still reports a few things in the admitted documents; each was looked at and kept: the names of the people who wrote or edited a test document, in its properties, comments and tracked changes, with the presence information Word keeps beside those names in `word/people.xml` (a Microsoft account ID in 137 Open-XML-PowerTools documents, a Windows domain account ID in one more, and in eight documents only the name again, under the provider "None"), the names of the embedded PDFs' authors in their properties, and the user folder names in file paths the documents refer to (attached templates, mail-merge data sources and linked files; 17 documents), as the sources published them; two signed documents (one with a certificate named "Test", one with a self-made certificate that holds a first name and a city: the owner's decision of 2026-10-10 excluded only the seven documents listed in the exclusion list); two documents with embedded font subsets; one document with ActiveX controls (their stored settings, no code); fictitious addresses at `test.com`; public-domain text (the United States Declaration of Independence, in the deepest file of the recursive-embedding test that Apache POI and Apache Tika share); two docx4j documents with icons from Office's built-in icon library (`Icons_Team` and `Icons_Aperture`), which Microsoft says are ["free to use and there's no royalty or copyright to worry about"](https://support.microsoft.com/en-us/office/insert-icons-in-microsoft-365-e2459f17-3996-4795-996e-b9a13486fa79), so their owner allows this use; and confidentiality wording that is template text or a watermark ("[Company name] Confidential", a fax disclaimer, the "CONFIDENTIAL" watermark).

A person named in a document, or the holder of rights in it, can have it removed: [storage.md](../storage.md#removing-a-document) describes how. The corpus is still hostile input, like any document: open it only with macros and external content disabled, ideally on the isolated reference machine or in a sandbox ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers), "Corpus hygiene").

## Not included, and why

- **LibreOffice's Writer test documents** (about 2,300 `.docx` files in [LibreOffice/core](https://github.com/LibreOffice/core)) stay out of the project: the owner decided so on 2026-10-10. The repository carries three license texts (MPL-2.0, LGPL-3.0, and GPL-3.0, which is there because LGPL-3.0 is written as additional permissions on top of it) and no statement about which applies to its test documents, and many of them are attachments to public bug reports, contributed by the reporters rather than written by LibreOffice's developers. Unlike the Apache License, the MPL has no clause that licenses what reporters submit through an issue tracker, so their license is unclear, and a content review would be needed on top.
- **Public-domain government documents** and **documents authored for the project** are not in version 1; they are candidates for the corpus growth of LAB-101, each with a recorded provenance. Authored documents would also bring back two features that lost their only documents in this review: cropping, recoloring and transparency of pictures, and table formulas.

## Changing the corpus

- **New documents from an existing source** need a new source entry at a newer commit (sources never change): add the source to `import.sh` under a new name, build a local store with the import, run `inspect-content.py` on it and look at every finding and every picture of the new documents, extend `excluded.tsv`, run the import again, and commit the manifest, the report and the list together.
- **A change to the tagger** that changes tags raises `TAGGER_VERSION`; then run `bayan-lab corpus tag --manifest lab/corpus/public/manifest.json --store …` (or the import from scratch) and commit the new manifest and report.
- **A change to the report's format** needs the report regenerated: `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab --test lab`.
