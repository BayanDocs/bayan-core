# Tagging rules

The tagger (`lab/bayan-lab/src/scan/`) reads a `.docx` package and reports which features of the [coverage matrix](https://github.com/BayanDocs/docs/blob/HEAD/specs/coverage-matrix.md) the document uses, by their exact names in the matrix, together with its fonts, languages, scripts and compatibility settings. The results go into the manifest's `scan` entry ([manifest-schema.md](manifest-schema.md)) and the frequency report. This page says, in plain words, what each tag means; the code in `scan/tagger.rs` and `scan/mod.rs` is the exact definition, and its tests (`lab/bayan-lab/tests/lab/tagging.rs`) pin the behavior described here.

A tag says that the document's markup **uses** a feature, not that the feature changes the document's appearance. A document that turns bold on in a style has the tag "Size, bold, italic" even if no text uses that style. The tags are meant for counting how often features occur in real documents, so that the coverage matrix can be prioritized, and for finding documents that exercise a feature.

Whenever a rule changes what a document is tagged with, `TAGGER_VERSION` in `scan/mod.rs` rises. `bayan-lab corpus verify` then reports every document tagged by an older version, and `bayan-lab corpus tag` tags them again (it needs the documents, so the store).

## Safety first

The scanner treats every package as hostile ([Fidelity Lab specification §3](https://github.com/BayanDocs/docs/blob/HEAD/specs/fidelity-lab.md#3-corpus-tiers), threat model T2 and T19). It reads only the ZIP structure and the XML of the parts listed below; it never decodes images, fonts or embedded objects, never runs anything, and never fetches anything a document refers to outside the package. Every resource is bounded (`Limits::DEFAULT` in `scan/mod.rs`):

| Limit | Value | Why |
|---|---|---|
| Package size | 512 MiB | the whole file is held in memory |
| ZIP entries | 10,000 | Word writes a few dozen |
| Entry name | 1,024 bytes | |
| One entry, uncompressed | 256 MiB, checked before decompressing | compression bombs |
| All entries, uncompressed | 1 GiB, checked before decompressing | |
| Parts read | 2,000 | relationship chains |
| XML nesting depth | 1,024 elements | the scanner keeps its own stack, so no input exhausts the call stack |
| Attributes on one element | 256 | the public corpus never exceeds 76 |
| Namespace declarations in scope | 256 | the public corpus never exceeds 74 |
| XML names | 256 bytes | the public corpus never exceeds 37 |
| Items in `[Content_Types].xml` or one relationships part | 10,000 | |
| Fonts, languages recorded | 200, 100 | the lists say so in a note when cut |
| Nested fields followed | 64 | |

A package is refused, with an error that names the problem, when it is an OLE compound file (an encrypted document or a binary `.doc`), when its ZIP structure is ambiguous or damaged (the end-of-central-directory record must end the file; local headers must agree with the central directory; entries must not overlap; CRC-32 and sizes must match; names must be relative paths inside the archive, unique even when compared case-insensitively and after percent-decoding; no encryption, no symbolic links, no spanning several disks, only stored and DEFLATE entries), when an XML part is not well-formed or has a document type declaration (so no entity can be declared or expanded), or when a limit is exceeded. The tests in `lab/bayan-lab/tests/lab/hostile_packages.rs` build a crafted file for each case.

## What is read

- `[Content_Types].xml` and the package relationships (`_rels/.rels`). The main part must have the content type of a `.docx` document; templates (`.dotx`) and macro-enabled files (`.docm`, `.dotm`) are refused.
- The **main document part**, and the parts it relates to: styles, numbering, settings, font table, theme, headers, footers, footnotes, endnotes, comments and comment extensions, and charts. The parts that headers, footers, notes and comments relate to are followed too, each part read once, so relationship cycles end.
- The **application properties** (`docProps/app.xml`), for the name of the program that saved the document and its page count.
- The **glossary document** (building blocks) is noted from its relationship but not read: its content is not part of the document.

Relationship targets that leave the package (`TargetMode="External"`), that climb above its root, or that name a missing part are never followed; a missing part is noted.

### Markup Compatibility

Word writes a modern feature together with a fallback for older readers, for example a DrawingML shape with a VML copy, inside `mc:AlternateContent`. The tagger applies Markup Compatibility the way Word does (ECMA-376 Part 3 §10): it takes the first `mc:Choice` whose `Requires` attribute names only prefixes of namespaces it knows (at most 16 of them), or else the `mc:Fallback`, and ignores the other branches. So a modern shape is tagged as a shape, not also as a legacy VML shape, and its text is counted once. Any `mc:AlternateContent` or `mc:Ignorable` tags "Markup Compatibility (AlternateContent, Ignorable)".

An element in a namespace the tagger does not know (`scan/vocabulary.rs` lists the Transitional and Strict namespaces of ECMA-376 and the Microsoft extensions Word writes) tags "Unknown parts and extensions preserved" and is skipped with all its content; so is a relationship of an unknown type.

### Stories

Drawings, objects and equations count only in the document's **stories**: the main document, headers, footers, footnotes, endnotes and comments. The numbering part's picture bullets, for example, hold a `w:pict` that defines a bullet, not a picture in the document. The text of the stories (`w:t`, and `m:t` in equations) is counted by script; deleted text (`w:delText`) and field instructions are not text.

## Features

### Package and document structure

| Feature | Tagged when |
|---|---|
| OPC package, content types, relationships | always (every accepted package) |
| Markup Compatibility (AlternateContent, Ignorable) | `mc:AlternateContent` or an `mc:Ignorable` attribute |
| Strict OOXML namespace variant | any element in a Strict (ISO/IEC 29500 Strict) namespace |
| Unknown parts and extensions preserved | an element in an unknown namespace, or a relationship of an unknown type |
| Core, app and custom properties | a core, extended or custom properties relationship |
| Custom XML parts | a custom XML relationship |
| Glossary document (building blocks) | a glossary document relationship |
| Embedded fonts | a font relationship, or `w:embedRegular`, `w:embedBold`, `w:embedItalic`, `w:embedBoldItalic` in the font table |
| Digital signatures | a digital signature relationship |
| VBA project | a VBA project relationship (`.docx` files cannot hold one, so normally never) |
| Password encryption (Agile) | never: an encrypted document is an OLE compound file, which the scanner refuses without decrypting |

### Characters and runs

| Feature | Tagged when |
|---|---|
| Text, tabs, breaks, symbols | `w:t`, `w:br`, `w:cr`, `w:sym`, a `w:tab` in a run, or a `SYMBOL` field |
| Fonts per script slot and hints | `w:rFonts` |
| Size, bold, italic (incl. complex-script variants) | `w:sz`, `w:szCs`, `w:b`, `w:bCs`, `w:i`, `w:iCs` |
| Underline styles and colors | `w:u` |
| Strike, double strike, caps, small caps, hidden | `w:strike`, `w:dstrike`, `w:caps`, `w:smallCaps`, `w:vanish` |
| Color (incl. theme colors, tint, shade) | `w:color` in run properties |
| Highlight and shading | `w:highlight`, or `w:shd` in run properties |
| Superscript, subscript, position | `w:vertAlign`, `w:position` |
| Character spacing, scaling, kerning threshold | `w:spacing`, `w:w` or `w:kern` in run properties |
| Borders around text | `w:bdr` |
| Emphasis marks, East Asian layout options | `w:em`, `w:eastAsianLayout` |
| Fit text | `w:fitText` |
| Language tags | `w:lang` |
| Right-to-left and complex-script flags | `w:rtl`, `w:cs` |
| Text effects (glow, shadow, outline, reflection, fill) | `w:outline`, `w:shadow`, `w:emboss`, `w:imprint`, or the Word 2010 effects (`w14:glow`, `w14:shadow`, `w14:textOutline`, `w14:textFill`, `w14:reflection`, `w14:props3d`, `w14:scene3d`) |
| OpenType features | `w14:ligatures`, `w14:numForm`, `w14:numSpacing`, `w14:stylisticSets`, `w14:cntxtAlts` |
| Hyperlinks | `w:hyperlink`, a hyperlink relationship, or a `HYPERLINK` field |
| Ruby (phonetic guide) | `w:ruby` |

### Paragraphs

| Feature | Tagged when |
|---|---|
| Alignment (incl. distribute, Thai, kashida variants) | `w:jc` in paragraph properties |
| Indentation (left, right, first line, hanging, chars) | `w:ind` in paragraph properties |
| Spacing before/after, auto spacing, contextual spacing | `w:spacing` in paragraph properties with `before`, `after`, `beforeLines`, `afterLines` or an auto-spacing attribute, or `w:contextualSpacing` |
| Line spacing (auto, exact, at least) | `w:spacing` in paragraph properties with `line` |
| Tab stops (all alignments and leaders, bar tabs) | `w:tabs` |
| Keep with next, keep lines together, page break before, widow control | `w:keepNext`, `w:keepLines`, `w:pageBreakBefore`, `w:widowControl` |
| Paragraph borders and shading (with border merging) | `w:pBdr`, or `w:shd` in paragraph properties |
| Outline level | `w:outlineLvl` |
| Drop caps and frames | `w:framePr` |
| Bidirectional paragraphs | `w:bidi` (outside section properties) |
| Snap to grid, East Asian line-breaking options | `w:snapToGrid`, `w:wordWrap`, `w:overflowPunct`, `w:topLinePunct`, `w:kinsoku`, `w:autoSpaceDE`, `w:autoSpaceDN` |
| Text alignment on line, text direction | `w:textAlignment`, or `w:textDirection` outside sections and cells |
| Suppress line numbers, suppress auto hyphens | `w:suppressLineNumbers`, `w:suppressAutoHyphens` |
| Paragraph mark formatting | run properties inside paragraph properties |

### Styles and themes

| Feature | Tagged when |
|---|---|
| Document defaults | `w:docDefaults` |
| Paragraph, character, linked styles and inheritance | a paragraph or character `w:style` |
| Table styles with conditional formatting | `w:tblStylePr`, in a document that has a table |
| Numbering styles | a numbering style with numbering properties (`w:numPr`); Word's built-in "No List" style has none |
| Latent styles | `w:latentStyles` |
| Toggle properties semantics | a toggle property (bold, italic, caps, small caps, strike, double strike, outline, shadow, emboss, imprint, hidden) set in a style |
| Theme fonts and colors | a theme font reference in `w:rFonts`, or a theme color or fill attribute |
| Style auto-redefinition (honored, but never silently) | `w:autoRedefine` |

Every Word document's styles define the "Normal Table" style with cell margins, so table formatting found only in the styles (cell margins, borders and shading, merges, row heights, floating, right-to-left, cell direction, table styles) counts only if the document has a table.

### Numbering and lists

| Feature | Tagged when |
|---|---|
| Abstract numbering and instances, level overrides | `w:abstractNum`, `w:num`, `w:lvlOverride` |
| Number formats (…) | `w:numFmt` |
| Level text, suffix, justification, legal numbering | `w:lvlText`, `w:suff`, `w:lvlJc`, `w:isLgl` |
| Restart rules | `w:lvlRestart`, `w:startOverride` |
| Picture bullets | `w:numPicBullet`, `w:lvlPicBulletId` |
| Legacy numbering | `w:legacy` |
| Heading outline numbering | a numbering level linked to a style (`w:pStyle` in `w:lvl`), or a heading style with numbering properties |

### Sections and page layout

| Feature | Tagged when |
|---|---|
| Page size, orientation, margins, gutter | `w:pgSz`, `w:pgMar` |
| Section breaks (next page, continuous, even, odd) | the main document has at least two sections |
| Columns (equal, unequal, separator) and column balancing | `w:cols` with more than one column or a separator, or two `w:col` |
| Vertical alignment of page | `w:vAlign` in section properties |
| Page borders (including art borders) | `w:pgBorders` |
| Line numbering | `w:lnNumType` |
| Page numbering format and restart | `w:pgNumType` with `fmt`, `start`, `chapStyle` or `chapSep` |
| Document grid (East Asian) | `w:docGrid` of type `lines`, `linesAndChars` or `snapToChars` |
| Text direction of section, right-to-left gutter | `w:textDirection` or `w:bidi` in section properties, `w:rtlGutter` |
| Mirror margins, book fold, gutter at top | `w:mirrorMargins`, `w:bookFoldPrinting`, `w:bookFoldRevPrinting`, `w:gutterAtTop`, switched on |

Section properties count only in the main document, and not inside a tracked section change.

### Headers and footers

| Feature | Tagged when |
|---|---|
| Default, first-page, even headers and footers | a header or footer reference, or `w:titlePg` or `w:evenAndOddHeaders` switched on |
| Link to previous (inheritance across sections) | some sections refer to headers or footers and others do not, so they inherit them |
| Header/footer distances and growth into body | a header or footer reference |
| Watermarks (VML/WordArt in header) | VML WordArt in a header, or a VML shape whose identifier Word gives watermarks (`PowerPlusWaterMarkObject…`, `WordPictureWatermark…`) |

### Tables

| Feature | Tagged when |
|---|---|
| Grid, widths, fixed layout | `w:tbl` |
| Autofit layout algorithm | a table without `w:tblLayout w:type="fixed"` |
| Horizontal and vertical merges | `w:gridSpan`, `w:vMerge`, `w:hMerge` |
| Borders (with conflict resolution) and shading | `w:tblBorders`, `w:tcBorders`, or `w:shd` in table or cell properties |
| Cell margins and spacing | `w:tblCellMar`, `w:tcMar`, `w:tblCellSpacing` |
| Row height rules, cannot split, header rows | `w:trHeight`, `w:cantSplit`, `w:tblHeader` |
| Rows splitting across pages | never: it depends on layout, not markup |
| Nested tables | a table inside a table |
| Floating tables | `w:tblpPr` |
| Right-to-left tables | `w:bidiVisual` |
| Cell text direction, vertical alignment | `w:textDirection` or `w:vAlign` in cell properties |
| Table formulas | a `=` field inside a table cell |

### Fields

A field's instruction (from `w:fldSimple`, or from the `w:instrText` of a complex field, which may be split over several runs) is classified by its first word. Any field tags "Complex and simple fields, nesting".

| Feature | Field types |
|---|---|
| Page fields | `PAGE`, `NUMPAGES`, `SECTIONPAGES`, `PAGEREF` |
| Date and time with picture switches and calendars | `DATE`, `TIME`, `CREATEDATE`, `SAVEDATE`, `PRINTDATE` |
| Cross-references and sequences | `REF`, `NOTEREF`, `SEQ`, `STYLEREF` |
| Document information | `AUTHOR`, `TITLE`, `FILENAME`, `DOCPROPERTY`, `DOCVARIABLE`, `SUBJECT`, `KEYWORDS`, `COMMENTS`, `LASTSAVEDBY`, `NUMWORDS`, `NUMCHARS`, `FILESIZE`, `INFO`, `TEMPLATE`, `USERNAME`, `USERINITIALS`, `USERADDRESS`, `EDITTIME`, `REVNUM` |
| Tables of contents and figures | `TOC`, `TC` |
| Formulas and conditions | `=` (outside tables), `IF`, `COMPARE` |
| Mail merge fields | `MERGEFIELD`, `NEXT`, `NEXTIF`, `SKIPIF`, `ASK`, `FILLIN`, `MERGEREC`, `MERGESEQ`, `ADDRESSBLOCK`, `GREETINGLINE`, `DATABASE`, or `w:mailMerge` in the settings |
| Indexes and authorities | `XE`, `INDEX`, `TA`, `TOA` |
| Citations and bibliography | `CITATION`, `BIBLIOGRAPHY` |
| Legacy form fields | `FORMTEXT`, `FORMCHECKBOX`, `FORMDROPDOWN`, or `w:ffData` |
| External and active fields (blocked by default) | `INCLUDETEXT`, `INCLUDEPICTURE`, `INCLUDE`, `IMPORT`, `LINK`, `DDE`, `DDEAUTO` (only counted: nothing is ever fetched or run) |
| Formatting switches | `\*`, `\#` or `\@` anywhere in the instruction, or `MERGEFORMAT` |

An `EQ` field tags "Combine characters, two lines in one, enclosed characters", and a `HYPERLINK` field "Hyperlinks".

### Notes, comments and review

| Feature | Tagged when |
|---|---|
| Footnotes and endnotes (…) | `w:footnoteReference`, `w:endnoteReference` |
| Comments with threads, resolution, people | `w:commentReference`, `w:commentRangeStart`, a comment, `w15:commentEx`, or a comments or comment-extension relationship |
| Tracked insertions and deletions | `w:ins`, `w:del` |
| Tracked moves | `w:moveFrom`, `w:moveTo`, and their range starts |
| Tracked formatting and property changes | `w:rPrChange`, `w:pPrChange`, `w:tblPrChange`, `w:tcPrChange`, `w:trPrChange`, `w:tblGridChange`, `w:tblPrExChange`, `w:numberingChange`, `w:sectPrChange` |
| Restrict editing, permissions | `w:documentProtection` with enforcement on, `w:writeProtection`, `w:permStart`, `w:permEnd` |
| Markup display modes and balloons; Compare and combine documents | never: these are features of the application, not of the file |

### Drawings and objects (stories only)

| Feature | Tagged when |
|---|---|
| Inline and anchored pictures | `pic:pic` |
| Image formats (…) | `a:blip`, an SVG blip, VML `v:imagedata`, or an image relationship |
| Cropping, recoloring, transparency, basic effects | `a:srcRect` with a non-zero edge, or a color or transparency effect inside `a:blip` |
| Text wrapping (…) | `wp:wrapSquare`, `wp:wrapTight`, `wp:wrapThrough`, `wp:wrapTopAndBottom`, `wp:wrapNone`, or `w10:wrap` |
| Positioning relative to page, margin, column, paragraph, line, character | `wp:positionH`, `wp:positionV` |
| Shapes: preset and custom geometry, fills, lines, arrowheads | `wps:wsp` |
| Text boxes and linked text boxes | `wps:txbx`, `wps:linkedTxbx`, `w:txbxContent`, or VML `v:textbox` |
| Groups and drawing canvases | `wpg:wgp`, `wpc:wpc`, or VML `v:group` |
| Shape effects (shadow, glow, soft edges, 3D) | an effect inside `a:effectLst`, `a:scene3d`, `a:sp3d` |
| WordArt | `a:prstTxWarp`, or VML `v:textpath` |
| Legacy VML shapes and pictures | any VML element, or `w:pict` |
| OLE objects (preview only; never activated) | `w:object`, `o:OLEObject`, or an OLE object relationship |
| Ink | `w14:contentPart`, `w:contentPart`, or the ink namespaces |
| 3D models | the 3D model namespace, or a 3D model relationship |

### Equations, charts and diagrams

| Feature | Tagged when |
|---|---|
| Equations (OMML) | `m:oMath`, `m:oMathPara` |
| Legacy equations (…) | an `o:OLEObject` whose program is `Equation.…` or MathType |
| Charts (…) | `c:chart` in a story, or a chart relationship |
| Newer chart types (…) | `cx:chart` in a story, or a `chartEx` relationship |
| 3D charts | `c:bar3DChart`, `c:line3DChart`, `c:pie3DChart`, `c:area3DChart`, `c:surface3DChart` in a chart part |
| SmartArt (from Word's drawing cache) | `dgm:relIds` in a story, or a diagram relationship |

### Content controls and forms

| Feature | Tagged when |
|---|---|
| Rich text, plain text, picture, checkbox, combo box, drop-down, date controls | `w:sdt` |
| Repeating sections, building block galleries | `w15:repeatingSection`, `w15:repeatingSectionItem`, `w:docPartObj`, `w:docPartList` |
| XML data binding | `w:dataBinding`, `w15:dataBinding` |

### East Asian and complex scripts

| Feature | Tagged when |
|---|---|
| Arabic and Hebrew shaping and bidi | the text has Arabic or Hebrew characters |
| Kashida justification | a paragraph alignment value with "Kashida" |
| Kinsoku (line-breaking rules) incl. custom lists | `w:kinsoku`, `w:noLineBreaksBefore`, `w:noLineBreaksAfter` |
| Auto-spacing between Asian and Latin text and numbers | `w:autoSpaceDE`, `w:autoSpaceDN` |
| Character grid | `w:docGrid` of type `linesAndChars` or `snapToChars` |
| Vertical text | a text direction other than left to right, top to bottom |
| Combine characters, two lines in one, enclosed characters | `w:eastAsianLayout` with `combine` or `vert`, or an `EQ` field |
| Indic and Southeast Asian shaping and breaking | the text has characters of an Indic or Southeast Asian script (Devanagari to Malayalam, Sinhala, Thai, Lao, Tibetan, Myanmar, Khmer) |

### Compatibility settings

| Feature | Tagged when |
|---|---|
| Compatibility mode 15 (Word 2013+) | `compatibilityMode` 15 or higher |
| Compatibility modes 14, 12, 11 | `compatibilityMode` 14, 12 or 11 |
| Individual legacy compatibility options (dozens) | any child of `w:compat` other than `w:compatSetting`, switched on; the options themselves are listed in `compatibility.options` |

## Fonts, languages and scripts

- **Fonts:** the names in the font table (`w:font w:name`), in `w:rFonts` (`ascii`, `hAnsi`, `eastAsia`, `cs`), and in DrawingML text and charts (`a:latin`, `a:ea`, `a:cs`, `a:sym`; references to the theme such as `+mn-lt` are not names), plus the theme's fonts for the slots that `w:rFonts` refers to (`minorHAnsi` and so on). Names are trimmed; empty names and names with control characters are ignored.
- **Languages:** `w:val` of `w:lang` and `w:themeFontLang`, put in canonical case. Their `w:eastAsia` and `w:bidi` tags count only if the text holds East Asian or complex-script characters respectively, because Word declares them in nearly every document whatever its text. A tag that is not well-formed is left out with a note.
- **Scripts:** each character of the stories' text is assigned a script by its Unicode block (`scan/scripts.rs`, 35 scripts); characters that all scripts share, such as digits, punctuation, spaces and symbols, are not counted.
