# Comparisons and the fidelity report

The Fidelity Lab turns "looks like Word" into numbers ([Fidelity Lab specification](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md) §6). This page explains how two layouts of one document are compared, what each metric means, and how the reports are made and read. Work package LAB-004 built it: the library crate [bayan-lab-compare](bayan-lab-compare/) computes everything, and `bayan-lab compare` and `bayan-lab report` run it on files.

## What is compared

Each document is laid out twice, and each layout is a [layout JSON](schema/layout-json.md):

- **Word's layout, the reference:** the layout that `bayan-lab pdf-extract` extracts from the PDF Word exported on the reference machine (LAB-002). It has every page, line and glyph.
- **BayanDocs' layout, the candidate:** the layout that the engine writes (`bayan-cli`, from Phase 1). Until the engine can lay out a document, the candidate is missing, and the document counts as laid out on no pages: every break disagrees.
- **The `.docx` file itself**, for M7: the places where Word recorded, when it last saved the document, that a page began (`w:lastRenderedPageBreak`).

Word's object-model skeleton (`source: word`, LAB-002) gives page sizes and positions in Word's own count of the document's characters. The engine counts its positions in its own way, so the comparisons do not compare those positions; they align the texts of the PDF's layout and the engine's. The skeleton's page count can stand for Word's in M1.

## How two layouts are compared

1. **One stream of text per side.** Each line's text is taken in its [comparable form](schema/layout-json.md#the-comparable-form): spaces, tabs and invisible characters removed, hyphens and ligatures spelled one way. The lines are joined into one stream, page after page; a page start and a line start are positions in it. A PDF has one line for all the text on one baseline that it draws in one go, such as the cells of a table row, where the engine has a line for each cell, so on both sides consecutive lines of a page whose baselines lie within half an em of each other are joined first (the extractor's rule for glyphs, applied to lines).
2. **Alignment.** The two streams are aligned like `diff` aligns two files, character by character: what both have in the same order is matched, and the rest is a difference. Small differences, such as a numbering label (`1.` against `a)`) or a field result (a date, a page number), stay local, so they do not shift everything after them. Long documents are aligned in pieces between anchors (runs of 32 characters that occur once on each side), and every amount of work is limited.
3. **Breaks through the alignment.** A page or line start of Word's text agrees when a BayanDocs page or line starts at the same place of the aligned text: exactly there in aligned text; anywhere around text that only BayanDocs has (an extra label or header); anywhere strictly inside BayanDocs' side of a short difference of both texts (at most 32 characters on each side, such as a date that differs). A start inside text that BayanDocs lacks agrees nowhere. Where BayanDocs lacks a whole line, the first line included, the start of the next line, which both have, is the one that agrees: deletions are moved, where the text allows, to begin and end where lines begin.
4. **Glyphs** are compared only on lines whose breaks agree (both their start and their end) and on pages whose starts agree, so that a line on another page, or lower because its page starts elsewhere, measures pagination rather than positioning. Glyphs are paired by their order from left to right when both lines show the same glyphs, which also covers right-to-left lines whose display does not tell their reading order ([layout-json.md](schema/layout-json.md), step 3 of "Lines from a PDF"); otherwise through the alignment, glyph by glyph (a ligature against separate letters is not paired). Only glyphs that show characters are measured, not spaces.

The same inputs always give the same numbers, on every platform: integers only, and every choice in a fixed order. The tests run in WebAssembly too.

## The metrics

| Metric | What it counts | Notes |
|---|---|---|
| M1 page count | both page counts; they match or not, and the difference (BayanDocs − Word) | from the layouts' `page_count` |
| M2 page-break agreement | of Word's pages that have text, how many start where a BayanDocs page starts | a page without text (only an image, say) has no place in the text and is not counted |
| M3 line-break agreement | of Word's lines that have text, how many start where a BayanDocs line starts | after joining lines on one baseline |
| M4 glyph position error | median, 95th percentile and maximum horizontal and vertical distance between paired glyphs, in points | percentiles by nearest rank; exact to a thousandth of a point below about 2 pt, within 0.1 % above |
| M5 visual difference | the share of a page's pixels that look different | not measured yet: the lab has no PDF rasterizer for Word's PDFs and the engine no reference rasters of documents. `bayan_lab_compare::visual::difference` is the measure that will compare them: the share of pixels whose 3×3 neighbourhood differs by more than 32 grey levels on average |
| M6 round-trip integrity | whether Word opens BayanDocs' re-saved copy without repair, with the same content | a process of the reference machine, not of these tools (below) |
| M7 hint agreement | of the `w:lastRenderedPageBreak` hints in the `.docx` file, how many fall where a BayanDocs page starts; and the page count they imply (one more than the hints) against BayanDocs' | approximate, but free: covers every document Word saved, the private corpus included. Only documents with at least one hint have M7 |

Every metric also says where its first disagreement is (page, and line on the page), and which breaks agree, by their index in Word's layout: that is what a baseline keeps.

### M7: the text the hints are read in

The hints sit in the body of the document, so M7 aligns the body's displayed text, as the file shows it, with BayanDocs' text: the text of the body's runs, tables and content controls, symbols and non-breaking hyphens; not deleted text, field codes, runs hidden in their own properties, or text boxes, which a page shows where they float. Numbering labels, footnote marks and headers and footers are not text in the file; the alignment tolerates them as it tolerates labels. A document that bayan-opc or bayan-xml refuses, as hostile or invalid, has no M7, and the report lists it among the documents that could not be read.

### M6: round-trip integrity on the reference machine

M6 needs Word, so it runs where Word runs (LAB-108): BayanDocs opens and saves every public document; on the reference machine, Word opens each saved copy read-only, as the harness of LAB-002 opens documents, and the run records whether a repair prompt appeared and whether the content is the same (the text and the page and line starts of Word's layout of the copy against Word's layout of the original, compared with these tools). The Word-side tests wait for the reference machine.

## The report

`bayan-lab report` compares every document of a corpus manifest and writes two files:

- **The JSON summary** (`--json`): every document's metrics (by SHA-256, and for public documents by the path its source gives it), and their aggregation. It is also what a later run reads as its baseline (`--baseline`).
- **The HTML report** (`--html`): one static page with no scripts, no images and no fonts, nothing from any other origin, so it renders the same offline, from a CI artifact or a file. Its charts are inline SVG.

Both show:

- **Aggregates** over all documents and per tier, per compatibility mode, per script and per feature tag of the manifest; a document counts in every group it belongs to. Agreement is summed over breaks: a group's M3 is the share of all its documents' lines that agree. M4's percentiles are those of all the group's glyphs.
- **The worst documents:** the 20 public documents with the lowest line-break agreement, and the 20 with the lowest hint agreement.
- **The feature tags most correlated with disagreement:** for each tag with at least five documents with it and five without, the point-biserial correlation between having the tag and a document's share of breaks that disagree, from 0 (no difference) to 1 (only documents with the tag disagree). The strongest are where work pays most.
- **Against the baseline**, if given: the documents that lost agreement (a break that agreed in the baseline and no longer does, or a page count that matched and no longer does), the documents that gained agreement and lost none, the documents whose Word layout changed (re-baselined: only M7 is compared), and documents gone or new. Breaks are compared one by one, so a document that loses one break and gains another still regresses. A baseline made by another version of the metrics (`metrics` in the summary) is refused, and so is one whose agreed breaks are not as this tool writes them (a damaged or edited file).
- **The documents that could not be read**, and why: a layout that is not valid, or a package that bayan-opc or bayan-xml refuses, which BayanDocs could not open either.

### Privacy

No result contains a document's text. Private (T2) documents count in the aggregates and in the numbers of regressions, but are never named: their entries in the summary carry only their SHA-256, as their manifest does, and the HTML report never lists them one by one. A summary of the private corpus is therefore as private as its manifest. Reports that CI publishes hold public-corpus results only (Fidelity Lab specification §12).

## Running it

```sh
bayan-lab compare [--hints DOCX] [--output FILE] REFERENCE CANDIDATE
bayan-lab report --manifest FILE [--references DIR] [--candidates DIR] [--store ADDRESS]
                 [--baseline FILE] [--json FILE] [--html FILE] [--title TEXT]
```

- `compare` compares two layout JSON files of one document and writes its metrics as JSON; with `--hints`, also M7 from the `.docx` file. Messages name the files by their role (the reference, the candidate), never by their path.
- `report` reads Word's layouts from `--references` and BayanDocs' from `--candidates`, each a folder of files named `<sha256>.json` by the document's SHA-256; a missing reference means no ground truth yet, a missing candidate counts as no pages. With `--store` (or `BAYAN_LAB_STORE`), it reads each document for M7. It exits with 1 when a document lost agreement against the baseline or could not be compared, and with 2 when it could not run.

For example, for the public corpus as CI runs it, before ground truth or engine layouts exist:

```sh
lab/corpus/public/import.sh ~/bayandocs-corpus/sources ~/bayandocs-corpus/store
mkdir -p word bayandocs
bayan-lab report --manifest lab/corpus/public/manifest.json --store ~/bayandocs-corpus/store \
  --references word --candidates bayandocs --json summary.json --html report.html
```

### In CI

`.github/workflows/fidelity.yml` builds the public corpus from its pinned sources (every document checked against the manifest's SHA-256), runs `bayan-lab report`, checks that the page stands alone, and uploads both files as the artifact `fidelity-report`. It runs on pull requests that change the lab or the readers M7 uses, on pushes to `main`, weekly and on demand. Today no document has Word's ground truth and the engine lays out nothing, so every score is zero and only M7 has data: 94 of the 1,256 public documents carry hints, and bayan-opc refuses 3 documents of the public corpus (an unsafe entry name, a compression ratio above 100 to 1, and a part name derivable from another's). The job compares with no baseline yet: work package LAB-106 turns these numbers into the fidelity gate and decides where its baselines are stored. It is not a required check.

## Limits

The comparison is bounded like every reader of untrusted input (ADR-0006): each stream at most 16 million characters (`metrics::Limits`), the alignment at most 4 billion steps of work, after which the text it has not reached counts as one difference and the summary says the alignment was cut short (`align::Limits`), Myers' algorithm at most 2,048 edits in one gap between anchors, and the hint reader at most 16 million characters and a million hints, within bayan-opc's and bayan-xml's own limits (`hints::Limits`). The fuzz target `docx_hints` covers the hint reader and M7's alignment together.
