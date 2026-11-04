# Layout JSON, version 1

The Fidelity Lab compares how Microsoft Word and BayanDocs lay out the same document ([Fidelity Lab specification](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md), §5). A comparison is fair only if both sides describe their layout in the same format. This is that format: one JSON file per document, listing its pages, the lines on each page with their baselines and text, the glyphs of each line with their positions, and other elements such as images and rules. Work package LAB-003 defines it; LAB-004 compares two such files.

- **Schema:** [layout.schema.json](layout.schema.json) is the JSON Schema (draft 2020-12), generated from the Rust types in `lab/bayan-lab-layout/src/layout.rs`; `bayan-lab layout schema` prints it.
- **Rules the schema cannot express** are listed below under [Rules](#rules); `bayan-lab layout check FILE...` checks a file against both, and so does the Rust function `Layout::from_json`.
- **Samples** of both sides are in [samples/](samples/): `pdf-libreoffice.json` and `pdf-chromium.json`, which the extractor wrote from PDFs of LibreOffice and Chromium (Word's side, as the lab extracts it from Word's PDFs), `engine.json`, a hand-written example of what BayanDocs' engine will write, with lengths in BLU too, and `word.json`, a hand-written example of the skeleton that Word's object model gives (page starts and sampled line starts, no glyphs). The tests of `bayan-lab-layout` validate all four against the schema and the rules.

A layout JSON from a document contains the document's text. Treat it like the document itself: a layout of a private-corpus document is private, and never goes into logs, reports or test snapshots that leave the machine.

## Who writes it

| `source` | Producer | What it gives |
|---|---|---|
| `pdf` | `bayan-lab pdf-extract`, from a PDF (in the lab, the PDF that Word exported on the reference machine, LAB-002) | every page, line and glyph with its exact position, images and rules; text offsets count through its own text (`text_offsets: lines`) |
| `word` | the Word ground-truth harness (LAB-002), from Word's object model | page sizes and the positions in Word's document where pages and sampled lines start; no glyphs, no positions on the page |
| `engine` | `bayan-cli`, from BayanDocs' layout (Phase 1) | everything, with every length also in BLU |

Fields that a producer cannot give are left out ("marked absent", as LAB-002 asks). A field that is present is always meaningful: an empty list means "none", an absent one "not known".

## Units and coordinates

- **Lengths** are in points (1/72 inch), as JSON numbers with at most three decimals: a fixed precision of a thousandth of a point. Readers refuse a number with more decimals. Rust code keeps them as whole numbers of thousandths (`Points`), so comparisons are exact.
- **BLU** (Bayan Layout Units, ADR-0005: 1 BLU = 1/25,400 point) appear where the engine produced a length: next to a length `x`, the field `x_blu` holds the same length in BLU, and `x` must then be the BLU value rounded to the nearest thousandth of a point (no value lies exactly halfway, since a thousandth of a point is 25.4 BLU).
- **Coordinates** are on the page **as displayed**: the origin is the top left corner, x grows to the right and y grows downwards. For a PDF, the page is its crop box (clipped to its media box), turned by its `Rotate`.
- **Text offsets** count characters, that is Unicode scalar values, not bytes or UTF-16 units. Producers that work in UTF-16, such as .NET and Word's object model, count a character outside the Basic Multilingual Plane once.

## Fields

### The document

| Field | Meaning |
|---|---|
| `schema` | Always `urn:bayandocs:lab:layout:v1`. A later version of the format gets a new identifier. |
| `source` | `pdf`, `word` or `engine` (above). |
| `producer` | The program and its version, such as `bayan-lab pdf-extract 1`: 1 to 256 printable characters. The extractor's number changes whenever the same PDF would give another file, so that ground truth extracted with older rules can be found and extracted again. |
| `text_offsets` | What the `start` and `end` of pages and lines count: `lines` or `document` (below). |
| `line_coverage` | `all` when the pages list every line, `sampled` when they list only some (Word's object model samples paragraphs). |
| `page_count` | The number of pages, equal to the length of `pages`. |
| `fonts` | The fonts that glyphs refer to by their position in this list (from 0). |
| `pages` | The pages, in order. |

**`text_offsets: lines`** means the file's own text stream: the `text` of every line, page after page and line after line, each line followed by one line feed (U+000A). Every line then has `start`, `end` and `text`, `end - start` is the number of characters of its `text`, each line starts one character after the one before it ends, and a page's `start` is where its first line starts. The PDF extractor always writes this; LAB-004 aligns two such streams.

**`text_offsets: document`** means positions in the source document's text, as its producer counts them: Word's character positions, or the engine's. They grow through the document, but need not match the lines' `text`, which holds what is displayed (a field shows its result, a numbering label is not in the document's text at all). Lines may skip positions, and lines of headers, footers and notes may leave `start` and `end` out.

### A font

| Field | Meaning |
|---|---|
| `name` | The font's name. From a PDF, the `BaseFont` without its six-letter subset tag (`Calibri-Bold`, `TimesNewRomanPSMT`), which for TrueType and OpenType fonts is the font's PostScript name; for a composite font, its CIDFont's `BaseFont`; `Type3` for a Type 3 font without a name. For the engine, the PostScript name of the font file. |
| `family` | The family name where the producer knows it: in a PDF, the font descriptor's `FontFamily`, which not every producer writes; the engine always knows it. |

Two fonts of a PDF with the same name and family, such as two subsets of Calibri, are one entry.

### A page

| Field | Meaning |
|---|---|
| `width`, `height` | The size of the page as displayed. With `width_blu` and `height_blu` from the engine. |
| `start` | The offset of the page's first character, where known. |
| `lines` | The page's lines in reading order (see [Lines from a PDF](#lines-from-a-pdf) for the extractor's). |
| `elements` | Images, rules and table cells, in drawing order; absent when the producer does not report them. |
| `skipped_glyphs` | From the PDF extractor only: how many glyphs the page draws that are on no line, because they are not upright (rotated, mirrored or vertical text) or not visible (render modes 3 and 7). |

### A line

| Field | Meaning |
|---|---|
| `start`, `end` | The offsets of its first character and of the character after its last (see `text_offsets`). |
| `text` | The characters the line displays, in reading (logical) order, by the [text rules](#text-rules). |
| `direction` | Its base direction: `ltr` or `rtl`. |
| `baseline` | The y of its baseline (with `baseline_blu` from the engine). |
| `glyphs` | Its glyphs in **display order**, from left to right. Needs `text`. |

### A glyph

| Field | Meaning |
|---|---|
| `offset`, `length` | The characters of the line's `text` that the glyph shows: from `offset` (counted in characters from the start of the line's text), `length` of them. A ligature shows several characters; a glyph that stands for no character, such as a decorative one, shows none (`length` 0). Glyphs of one cluster, such as a base letter and a mark drawn from one character, share the same characters; any other two glyphs show none in common. |
| `x`, `y` | The glyph's origin: the point on its baseline where the pen stood. A superscript's `y` is above its line's baseline. |
| `advance` | How far the glyph moves the pen along the baseline: its width at its size, with character and word spacing, without kerning or other adjustments that move only the next glyph (in a PDF, `TJ` numbers). In a PDF that is `(w0 × Tfs + Tc + Tw) × Th` in text space (ISO 32000-2 §9.4.4), turned into page space. |
| `font` | Its font's position in `fonts`. |
| `size` | The font size: the height of the em square, measured perpendicular to the baseline. Horizontal scaling does not change it. |
| `gid` | The glyph identifier in the font, where known. The extractor knows it for composite TrueType-based fonts (CIDFontType2 with an identity or explicit `CIDToGIDMap`); it never reads font programs. |
| `x_blu`, `y_blu`, `advance_blu`, `size_blu` | The same lengths in BLU, from the engine. |

### An element

| Field | Meaning |
|---|---|
| `kind` | `image`, `rule` (a horizontal or vertical stroked line, or a thin filled rectangle) or `cell` (a table cell's box; the extractor cannot see cells in a PDF, the engine can). |
| `x0`, `y0`, `x1`, `y1` | Its box: left, top, right and bottom, with `x0 ≤ x1` and `y0 ≤ y1`. With `…_blu` from the engine. |

## Rules

`Layout::check` and `bayan-lab layout check` enforce these, besides the schema:

1. `page_count` equals the number of pages; widths and heights are positive.
2. `producer`, font names and families have 1 to 256 characters and no control characters.
3. A line's `text` has no control characters except the tab, and no noncharacters (U+FFFE, U+FDD0 and the like).
4. With `text_offsets: lines`: `line_coverage` is `all`; every line has `start`, `end` and `text`; the offsets run through the text stream as described above, and a page's `start`, where given, is where its lines start.
5. With `text_offsets: document`: a line's `end` is not before its `start`, and line starts do not go back within a page.
6. A line with `glyphs` has `text`; every glyph's characters lie within it; glyphs either show the same characters or none in common; every glyph names a font of `fonts`; sizes are positive.
7. An element's box has `x0 ≤ x1` and `y0 ≤ y1`.
8. Every BLU value matches its points (above).
9. A file from a PDF (`source: pdf`) counts offsets through its lines, gives every page its `elements` and `skipped_glyphs`, and every line its `direction`, `baseline` and `glyphs`.

## Text rules

A line's `text` holds what a reader sees on that line, so that both sides of a comparison produce the same characters for the same display:

1. **Reading order.** Characters come in logical order, as they are read, also in right-to-left lines; glyphs, which keep display order, point into that text.
2. **Displayed text only.** Fields show their results, never their codes (a page number field is `3`, a table of contents entry is its text and page number). Hidden text, paragraph marks, line, column, page and section breaks, and anchors of floating objects are not written: they are not characters on a line.
3. **Numbering labels** are written as displayed, as part of the line they begin: `1.`, `a)`, `iv.`, or the bullet's character. A bullet from a symbol font (Symbol, Wingdings) is the character the document uses, usually a private-use code point from U+F020 to U+F0FF (Word's standard bullet is U+F0B7), which is also what Word's PDFs map it to. The separator after a label is a tab (U+0009) in the engine's text and a gap in a PDF; comparisons ignore both (below).
4. **Spaces and tabs** that the line displays are written as the characters they are (U+0020, U+00A0, U+0009 …). A PDF may draw a space as a glyph or leave a gap; the extractor writes what the PDF draws and never invents a space for a gap.
5. **Hyphens.** An optional hyphen is written only where it is displayed, at the end of a line, as the character the producer has (U+00AD or U+002D).
6. **Ligatures and clusters.** A glyph that shows several characters, such as an `fi` ligature, has them all in `text`, and `length` says how many. A PDF's `ToUnicode` map may give a compatibility ligature character (U+FB01) instead of its letters; the extractor writes what the map gives.
7. **Unknown characters.** When nothing tells the extractor which characters a glyph shows (no `ToUnicode` entry and no usable encoding), it writes U+FFFD, counts the glyph, and `bayan-lab pdf-extract` warns. Control characters that a map gives are written as U+FFFD too.
8. **No normalization.** Text is written as the producer has it: no case folding and no Unicode normalization.

### The comparable form

Some differences remain between producers without the page looking any different. The comparisons of LAB-004 align the **comparable form** of each line's text, which the Rust function `bayan_lab_layout::text::comparable` computes:

1. whitespace (Unicode's `White_Space`: spaces, tabs, no-break spaces, line and paragraph separators) is removed;
2. characters that display nothing are removed: U+200B to U+200D, U+2060 to U+2064, U+FEFF and the bidirectional formatting characters (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to U+2069);
3. U+00AD, U+2010 and U+2011, which display as a hyphen, become U+002D;
4. the Latin compatibility ligatures U+FB00 to U+FB06 become their letters.

Everything else, including U+FFFD, is compared as it is.

## Lines from a PDF

A PDF has no lines: it draws glyphs at positions. The extractor forms lines in four steps, and the same PDF always gives the same lines:

1. **Grouping by baseline in drawing order.** Glyphs are taken in the order the content stream draws them. A glyph continues the current line when its baseline is within half an em of the line's: the em of the larger of the glyph and the line's largest glyph so far. Otherwise it starts a new line. PDF producers draw text line by line (LibreOffice and Chromium do in the tests; Word's PDFs are still to be tested, see below), so drawing order keeps columns, floating text and table cells apart, and superscripts and subscripts join their line.
2. **Display order.** A line's glyphs are sorted by the x of their origins; glyphs at the same x keep their drawing order.
3. **Reading order.** A line without right-to-left characters reads left to right. Otherwise its base direction is right to left when it has more strong right-to-left characters (Bidi_Class R or AL) than left-to-right ones (L), or as many and its rightmost strong character is right to left. The Unicode Bidirectional Algorithm (UAX #9, through the unicode-bidi crate with ICU4X's data, as BayanDocs' engine uses it, ADR-0009) then resolves the levels of the characters in display order and reverses their runs (rule L2), which turns display order back into reading order: Hebrew and Arabic words, numbers inside them and Latin words inside them come out as they are read. A glyph takes the level of its first character.
4. **Baseline.** The line's `baseline` is the y that most of its largest glyphs share (the smaller y on a tie), so a raised superscript or a large initial does not move it.

Known differences from the engine's lines, which LAB-004's comparisons have to allow for:

- Text on one baseline that the PDF draws in one go is one line, even where the engine has several: the cells of a table row whose cells each hold one line on the same baseline (`Alpha1` in the samples), or text on both sides of a tab. LAB-004 should join the engine's consecutive lines on one baseline the same way before it compares line breaks.
- A superscript or other piece of a line that a producer draws separately, after other lines, forms a line of its own on the same baseline.
- Only upright text is placed on lines in version 1; rotated and vertical text is counted in `skipped_glyphs`.

## The extractor

`bayan-lab pdf-extract [--output FILE] PDF` writes the layout JSON of a PDF: compact, with a final line feed. It treats every PDF as hostile (ADR-0006): it reads only what extraction needs and refuses everything else with an error that names what and where (object and page numbers), never the document's text or names; it never repairs a broken file. The exit status is 0 when it wrote the file, 1 when it refused the PDF, and 2 when it could not run (wrong arguments, an unreadable file); glyphs with unknown characters or off the lines only add warnings.

It reads classic cross-reference tables and cross-reference streams, incremental updates, hybrid files, object streams, and the filters FlateDecode (with the PNG and TIFF predictors), ASCIIHexDecode, ASCII85Decode and RunLengthDecode. It interprets every operator of PDF 2.0's content streams that places something: the graphics state stack and the current transformation matrix, text objects with their text and line matrices, `Tc`, `Tw`, `Tz`, `TL`, `Ts`, `Tr`, `TJ` adjustments, form XObjects with their matrices, image XObjects and inline images, and painted paths for rules. It measures glyphs with simple fonts' `Widths` (Type 1, TrueType, and Type 3 through its `FontMatrix`) and composite fonts' `W` and `DW` (Type 0 with `Identity-H`), and reads their characters from `ToUnicode` maps, then from the `Differences` and base encoding of simple fonts (StandardEncoding, WinAnsiEncoding, MacRomanEncoding).

It refuses, among others: encrypted files; LZW and the image codecs in streams it must read; composite fonts with other CMaps, vertical writing (`Identity-V`), and simple fonts without `Widths` (the standard 14 fonts, whose metrics it does not carry); optional content (layers), which could hide text it would report; user units other than 1; operators PDF does not define, outside `BX`/`EX`. Every size, count, depth and amount of work is limited (`bayan_lab_layout::pdf::Limits`: input size, objects, nesting, cross-reference sections, decoded bytes per stream and in total, pages, operators, glyphs, form XObjects, graphics states, fonts and their tables, path segments, elements). It never reads font programs, so it never needs to trust them.

Word's own PDFs have not been tested yet: the reference machine of LAB-002 is not ready. The tests use PDFs of LibreOffice 24.2 and Chromium 153, whose glyph positions agree with poppler's `pdftotext` to a thousandth of a point, and synthetic PDFs whose positions are worked out by hand.
