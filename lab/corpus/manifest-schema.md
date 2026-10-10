# Corpus manifest

A corpus manifest is one JSON file that describes every document of a corpus without containing any of them. The documents themselves live in a store, outside Git ([storage.md](storage.md)); the manifest is committed, so every change to the corpus is reviewed like code. The public corpus's manifest is [public/manifest.json](public/manifest.json).

The tools write the manifest; nobody edits it by hand. `bayan-lab corpus add`, `tag` and `source add` change it, and `bayan-lab corpus verify` checks it. Its JSON Schema (draft 2020-12) is [manifest.schema.json](manifest.schema.json), generated from the Rust types in `lab/bayan-lab/src/manifest.rs`, so the schema cannot drift from the code: a test fails when the committed schema differs from the generated one (regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab --test lab`).

## Layout

The file is written deterministically, so the same corpus always gives the same bytes on every platform: two-space indentation, a final line feed, keys in a fixed order, sources sorted by name, documents sorted by SHA-256, and every list sorted (except provenance, whose first entry has a meaning, see below). Unknown fields are refused when the file is read, so a typo cannot slip in unnoticed.

```json
{
  "format": "bayan-lab-corpus-manifest",
  "version": 1,
  "sources": { "apache-poi": { … } },
  "documents": [ { … } ]
}
```

| Field | Meaning |
|---|---|
| `format` | Always `bayan-lab-corpus-manifest`. |
| `version` | The version of this layout: 1. A tool refuses a version it does not know. |
| `sources` | Where documents come from, by source name (lower-case letters, digits and hyphens). |
| `documents` | One entry per document, sorted by SHA-256. |

## Sources

A source is described once for all its documents: typically an upstream Git repository at one exact revision. **Sources never change:** `corpus source add` refuses to change a registered source, so taking documents from a newer revision means registering a new source (for example `python-docx-2`).

| Field | Meaning |
|---|---|
| `url` | Where the source can be found, an `https://` address such as the repository's. |
| `revision` | The exact revision the documents were taken from, such as a full Git commit hash. |
| `license` | The SPDX license expression under which the source distributes its documents, or `NOASSERTION` for a private source. Public sources must use one of the licenses listed below. |
| `copyright` | The copyright notice of the source, as its license file states it. |
| `license_texts` | The source's license and notice files: each with its `path` in the source (such as `legal/NOTICE`) and the `sha256` of its bytes. The store keeps these texts beside the documents (`licenses/<sha256>.txt`), so the corpus can be redistributed with them, as Apache-2.0's notice requirement and the MIT and BSD licenses' copyright-notice requirements demand. |
| `notes` | Optional: why the license applies to the documents, and anything else a reviewer should know. |

## Documents

| Field | Meaning |
|---|---|
| `sha256` | The SHA-256 of the document's bytes, in lower-case hexadecimal: its identity, and its key in the store (`objects/ab/cdef….docx`). The same bytes found in several places are one document. |
| `size` | Its size in bytes. |
| `tier` | Its corpus tier ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers)): `T0` probes, `T1` public, `T2` private, `T3` torture. |
| `license` | The SPDX license expression under which the corpus redistributes the document: the license of its first source. Public tiers (`T0`, `T1`, `T3`) allow only Apache-2.0, BSD-2-Clause, BSD-3-Clause, CC-BY-4.0, CC0-1.0, MIT and MPL-2.0 (`PUBLIC_LICENSES` in `manifest.rs`; adding one needs a reviewed pull request). A private (`T2`) document's license is always `NOASSERTION`: it is never redistributed. |
| `provenance` | Where the document was found: at least one entry, each naming a `source` and, for public documents, the document's `path` in that source. The first entry is the one whose license applies. **Private documents never record paths**, because file names can reveal personal data. |
| `scan` | What the tagger found ([tagging-rules.md](tagging-rules.md)), or `null` until the document is tagged. |
| `page_count` | The document's page count in Word, once ground truth exists (LAB-002); `null` until then. |
| `ground_truth` | Whether Word ground truth exists: a `status` of `none`, `produced` or `failed`, and, once produced, the `word_build` that produced it. LAB-002 fills it in. |
| `notes` | Optional curator's notes. **Never document content.** |

### What the tagger found (`scan`)

| Field | Meaning |
|---|---|
| `tagger` | The version of the tagging rules that produced this entry. Whenever a rule changes what a document is tagged with, the version rises; `corpus verify` then reports every document tagged by an older version, and `corpus tag` tags them again. |
| `features` | The [coverage matrix](https://github.com/BayanDocs/docs/blob/HEAD/specs/coverage-matrix.md) features the document uses, by their exact names in the matrix, sorted. Four features cannot be detected by scanning a package (password encryption, rows splitting across pages, markup display modes, compare and combine) and never appear here. |
| `fonts` | The font names the document refers to, sorted: its font table, the fonts its runs name, the fonts of its DrawingML text and charts, and the theme fonts it actually uses. At most 200, each at most 64 characters. |
| `languages` | The BCP 47 language tags it declares, in canonical case (`en-US`, `zh-Hant-TW`), sorted, at most 100. East Asian and complex-script tags count only if the text holds characters of such scripts, because Word declares them in nearly every document. |
| `scripts` | Characters per script in the document's text (the main story, headers, footers, notes and comments), by Unicode block. Digits, punctuation and symbols that all scripts share are not counted. |
| `compatibility` | The `mode` of the `compatibilityMode` setting (15 for Word 2013 and later, or 14, 12, 11), or `null` without one; and the legacy compatibility `options` the document switches on, by element name, sorted. |
| `application` | The program that last saved the document, from its application properties, reduced to the program and its version: LibreOffice's platform and build and WPS Office's installation GUID are removed, so the name cannot identify a machine. Absent when not recorded. |
| `pages_hint` | The page count that program recorded: a hint until Word's own count is known. Absent when not recorded. |
| `notes` | Non-fatal observations, from a fixed list of messages, such as "a relationship points to a part that is missing". Never document content. |

## Consistency rules

The tools keep these rules when they write a manifest, and `bayan-lab corpus verify` checks them (and, given a store, that every object is present with its SHA-256, and that the store holds nothing else):

- documents are sorted by SHA-256 and each appears once; no document has size 0;
- every source is defined, has an `https://` address, a revision, an allowed license, and for public sources a copyright notice and at least one license text with a relative path;
- every document's provenance names defined sources, with relative paths inside them, and no provenance appears twice; the same source path is never recorded for two different documents;
- public documents carry an allowed license equal to their first source's; private documents carry `NOASSERTION` and record no paths;
- every document is tagged with the current tagger version, with known coverage-matrix names only (and never a feature that cannot be detected), and with sorted lists;
- ground truth marked as produced names its Word build; notes are never empty strings.

With `--exclude FILE`, `corpus verify` also fails if the manifest contains a document that the exclusion list names (see [public/README.md](public/README.md)).
