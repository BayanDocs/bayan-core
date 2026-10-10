//! The features of the Word feature coverage matrix, by the names the matrix uses ([coverage matrix v1][matrix]).
//!
//! Feature tags in the manifest are these names, exactly as the matrix writes them, so that frequencies can be read against the matrix and its priorities recalibrated from them. [`FEATURES`] lists them in the matrix's order, with the section each belongs to; the tests check that every [`Feature`] appears once, in order, under a unique name.
//!
//! The list was taken from `specs/coverage-matrix.md` in the docs repository at commit `e9b045d` (2026-10-09). When the matrix gains, renames or removes a row, change this list in the same way and raise [`super::TAGGER_VERSION`], so that every document is tagged again.
//!
//! [matrix]: https://github.com/BayanDocs/docs/blob/HEAD/specs/coverage-matrix.md

/// A row of the coverage matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Feature {
    /// OPC package, content types, relationships.
    OpcPackage,
    /// Markup Compatibility (AlternateContent, Ignorable).
    MarkupCompatibility,
    /// Strict OOXML namespace variant.
    StrictNamespaces,
    /// Unknown parts and extensions preserved.
    UnknownPartsAndExtensions,
    /// Core, app and custom properties.
    DocumentProperties,
    /// Custom XML parts.
    CustomXmlParts,
    /// Glossary document (building blocks).
    GlossaryDocument,
    /// Embedded fonts.
    EmbeddedFonts,
    /// Password encryption (Agile).
    PasswordEncryption,
    /// Digital signatures.
    DigitalSignatures,
    /// VBA project.
    VbaProject,
    /// Text, tabs, breaks, symbols.
    TextTabsBreaksSymbols,
    /// Fonts per script slot and hints.
    FontsPerScript,
    /// Size, bold, italic (incl. complex-script variants).
    SizeBoldItalic,
    /// Underline styles and colors.
    Underline,
    /// Strike, double strike, caps, small caps, hidden.
    StrikeCapsHidden,
    /// Color (incl. theme colors, tint, shade).
    Color,
    /// Highlight and shading.
    HighlightAndShading,
    /// Superscript, subscript, position.
    VerticalPosition,
    /// Character spacing, scaling, kerning threshold.
    CharacterSpacing,
    /// Borders around text.
    TextBorders,
    /// Emphasis marks, East Asian layout options.
    EmphasisMarks,
    /// Fit text.
    FitText,
    /// Language tags.
    LanguageTags,
    /// Right-to-left and complex-script flags.
    RightToLeftRuns,
    /// Text effects (glow, shadow, outline, reflection, fill).
    TextEffects,
    /// OpenType features.
    OpenTypeFeatures,
    /// Hyperlinks.
    Hyperlinks,
    /// Ruby (phonetic guide).
    Ruby,
    /// Alignment (incl. distribute, Thai, kashida variants).
    Alignment,
    /// Indentation (left, right, first line, hanging, chars).
    Indentation,
    /// Spacing before/after, auto spacing, contextual spacing.
    ParagraphSpacing,
    /// Line spacing (auto, exact, at least).
    LineSpacing,
    /// Tab stops (all alignments and leaders, bar tabs).
    TabStops,
    /// Keep with next, keep lines together, page break before, widow control.
    KeepAndWidowControl,
    /// Paragraph borders and shading (with border merging).
    ParagraphBordersAndShading,
    /// Outline level.
    OutlineLevel,
    /// Drop caps and frames.
    DropCapsAndFrames,
    /// Bidirectional paragraphs.
    BidirectionalParagraphs,
    /// Snap to grid, East Asian line-breaking options.
    EastAsianLineBreaking,
    /// Text alignment on line, text direction.
    TextAlignmentAndDirection,
    /// Suppress line numbers, suppress auto hyphens.
    SuppressLineNumbersAndHyphens,
    /// Paragraph mark formatting.
    ParagraphMarkFormatting,
    /// Document defaults.
    DocumentDefaults,
    /// Paragraph, character, linked styles and inheritance.
    ParagraphAndCharacterStyles,
    /// Table styles with conditional formatting.
    TableStyles,
    /// Numbering styles.
    NumberingStyles,
    /// Latent styles.
    LatentStyles,
    /// Toggle properties semantics.
    ToggleProperties,
    /// Theme fonts and colors.
    ThemeFontsAndColors,
    /// Style auto-redefinition (honored, but never silently).
    StyleAutoRedefinition,
    /// Abstract numbering and instances, level overrides.
    AbstractNumbering,
    /// Number formats (decimal, roman, letters, ordinal, cardinal text, East Asian, Hebrew, Arabic and more).
    NumberFormats,
    /// Level text, suffix, justification, legal numbering.
    LevelTextAndSuffix,
    /// Restart rules.
    RestartRules,
    /// Picture bullets.
    PictureBullets,
    /// Legacy numbering.
    LegacyNumbering,
    /// Heading outline numbering.
    HeadingNumbering,
    /// Page size, orientation, margins, gutter.
    PageSizeAndMargins,
    /// Section breaks (next page, continuous, even, odd).
    SectionBreaks,
    /// Columns (equal, unequal, separator) and column balancing.
    Columns,
    /// Vertical alignment of page.
    PageVerticalAlignment,
    /// Page borders (including art borders).
    PageBorders,
    /// Line numbering.
    LineNumbering,
    /// Page numbering format and restart.
    PageNumbering,
    /// Document grid (East Asian).
    DocumentGrid,
    /// Text direction of section, right-to-left gutter.
    SectionTextDirection,
    /// Mirror margins, book fold, gutter at top.
    MirrorMarginsAndBookFold,
    /// Default, first-page, even headers and footers.
    HeadersAndFooters,
    /// Link to previous (inheritance across sections).
    LinkToPrevious,
    /// Header/footer distances and growth into body.
    HeaderFooterDistances,
    /// Watermarks (VML/WordArt in header).
    Watermarks,
    /// Grid, widths, fixed layout.
    TableGrid,
    /// Autofit layout algorithm.
    TableAutofit,
    /// Horizontal and vertical merges.
    CellMerges,
    /// Borders (with conflict resolution) and shading.
    TableBordersAndShading,
    /// Cell margins and spacing.
    CellMarginsAndSpacing,
    /// Row height rules, cannot split, header rows.
    RowHeightAndHeaderRows,
    /// Rows splitting across pages.
    RowsSplittingAcrossPages,
    /// Nested tables.
    NestedTables,
    /// Floating tables.
    FloatingTables,
    /// Right-to-left tables.
    RightToLeftTables,
    /// Cell text direction, vertical alignment.
    CellTextDirection,
    /// Table formulas.
    TableFormulas,
    /// Complex and simple fields, nesting.
    Fields,
    /// Page fields.
    PageFields,
    /// Date and time with picture switches and calendars.
    DateAndTimeFields,
    /// Cross-references and sequences.
    CrossReferences,
    /// Document information.
    DocumentInformationFields,
    /// Tables of contents and figures.
    TablesOfContents,
    /// Formulas and conditions.
    FormulasAndConditions,
    /// Mail merge fields.
    MailMergeFields,
    /// Indexes and authorities.
    IndexesAndAuthorities,
    /// Citations and bibliography.
    Citations,
    /// Legacy form fields.
    LegacyFormFields,
    /// External and active fields (blocked by default).
    ExternalFields,
    /// Formatting switches.
    FieldFormattingSwitches,
    /// Footnotes and endnotes (placement, numbering, separators, continuation).
    FootnotesAndEndnotes,
    /// Comments with threads, resolution, people.
    Comments,
    /// Tracked insertions and deletions.
    TrackedInsertionsAndDeletions,
    /// Tracked moves.
    TrackedMoves,
    /// Tracked formatting and property changes.
    TrackedFormatting,
    /// Markup display modes and balloons.
    MarkupDisplayModes,
    /// Compare and combine documents.
    CompareAndCombine,
    /// Restrict editing, permissions.
    RestrictEditing,
    /// Inline and anchored pictures.
    Pictures,
    /// Image formats (PNG, JPEG, GIF, BMP, TIFF, WebP, SVG, EMF, WMF, EMF+).
    ImageFormats,
    /// Cropping, recoloring, transparency, basic effects.
    PictureAdjustments,
    /// Text wrapping (square, tight, through, top and bottom, behind, in front).
    TextWrapping,
    /// Positioning relative to page, margin, column, paragraph, line, character.
    ObjectPositioning,
    /// Shapes: preset and custom geometry, fills, lines, arrowheads.
    Shapes,
    /// Text boxes and linked text boxes.
    TextBoxes,
    /// Groups and drawing canvases.
    GroupsAndCanvases,
    /// Shape effects (shadow, glow, soft edges, 3D).
    ShapeEffects,
    /// WordArt.
    WordArt,
    /// Legacy VML shapes and pictures.
    VmlShapes,
    /// OLE objects (preview only; never activated).
    OleObjects,
    /// Ink.
    Ink,
    /// 3D models.
    Models3d,
    /// Equations (OMML).
    Equations,
    /// Legacy equations (Equation Editor 3, MathType as OLE with WMF preview).
    LegacyEquations,
    /// Charts (bar, column, line, pie, area, scatter, combo).
    Charts,
    /// Newer chart types (waterfall, treemap, sunburst, histogram, box and whisker, funnel, map).
    NewerChartTypes,
    /// 3D charts.
    Charts3d,
    /// SmartArt (from Word's drawing cache).
    SmartArt,
    /// Rich text, plain text, picture, checkbox, combo box, drop-down, date controls.
    ContentControls,
    /// Repeating sections, building block galleries.
    RepeatingSections,
    /// XML data binding.
    XmlDataBinding,
    /// Arabic and Hebrew shaping and bidi.
    ArabicAndHebrew,
    /// Kashida justification.
    Kashida,
    /// Kinsoku (line-breaking rules) incl. custom lists.
    Kinsoku,
    /// Auto-spacing between Asian and Latin text and numbers.
    AsianAutoSpacing,
    /// Character grid.
    CharacterGrid,
    /// Vertical text.
    VerticalText,
    /// Combine characters, two lines in one, enclosed characters.
    CombineCharacters,
    /// Indic and Southeast Asian shaping and breaking.
    IndicAndSoutheastAsian,
    /// Compatibility mode 15 (Word 2013+).
    CompatibilityMode15,
    /// Compatibility modes 14, 12, 11.
    OlderCompatibilityModes,
    /// Individual legacy compatibility options (dozens).
    LegacyCompatibilityOptions,
}

/// A feature with its place in the coverage matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    /// The feature.
    pub feature: Feature,
    /// The matrix section it belongs to, such as `Tables`.
    pub section: &'static str,
    /// Its name in the matrix: the tag recorded in the manifest.
    pub name: &'static str,
}

/// Every feature of the coverage matrix, in the matrix's order.
pub const FEATURES: [Info; 139] = [
    Info {
        feature: Feature::OpcPackage,
        section: "Package and document structure",
        name: "OPC package, content types, relationships",
    },
    Info {
        feature: Feature::MarkupCompatibility,
        section: "Package and document structure",
        name: "Markup Compatibility (AlternateContent, Ignorable)",
    },
    Info {
        feature: Feature::StrictNamespaces,
        section: "Package and document structure",
        name: "Strict OOXML namespace variant",
    },
    Info {
        feature: Feature::UnknownPartsAndExtensions,
        section: "Package and document structure",
        name: "Unknown parts and extensions preserved",
    },
    Info {
        feature: Feature::DocumentProperties,
        section: "Package and document structure",
        name: "Core, app and custom properties",
    },
    Info {
        feature: Feature::CustomXmlParts,
        section: "Package and document structure",
        name: "Custom XML parts",
    },
    Info {
        feature: Feature::GlossaryDocument,
        section: "Package and document structure",
        name: "Glossary document (building blocks)",
    },
    Info {
        feature: Feature::EmbeddedFonts,
        section: "Package and document structure",
        name: "Embedded fonts",
    },
    Info {
        feature: Feature::PasswordEncryption,
        section: "Package and document structure",
        name: "Password encryption (Agile)",
    },
    Info {
        feature: Feature::DigitalSignatures,
        section: "Package and document structure",
        name: "Digital signatures",
    },
    Info {
        feature: Feature::VbaProject,
        section: "Package and document structure",
        name: "VBA project",
    },
    Info {
        feature: Feature::TextTabsBreaksSymbols,
        section: "Characters and runs",
        name: "Text, tabs, breaks, symbols",
    },
    Info {
        feature: Feature::FontsPerScript,
        section: "Characters and runs",
        name: "Fonts per script slot and hints",
    },
    Info {
        feature: Feature::SizeBoldItalic,
        section: "Characters and runs",
        name: "Size, bold, italic (incl. complex-script variants)",
    },
    Info {
        feature: Feature::Underline,
        section: "Characters and runs",
        name: "Underline styles and colors",
    },
    Info {
        feature: Feature::StrikeCapsHidden,
        section: "Characters and runs",
        name: "Strike, double strike, caps, small caps, hidden",
    },
    Info {
        feature: Feature::Color,
        section: "Characters and runs",
        name: "Color (incl. theme colors, tint, shade)",
    },
    Info {
        feature: Feature::HighlightAndShading,
        section: "Characters and runs",
        name: "Highlight and shading",
    },
    Info {
        feature: Feature::VerticalPosition,
        section: "Characters and runs",
        name: "Superscript, subscript, position",
    },
    Info {
        feature: Feature::CharacterSpacing,
        section: "Characters and runs",
        name: "Character spacing, scaling, kerning threshold",
    },
    Info {
        feature: Feature::TextBorders,
        section: "Characters and runs",
        name: "Borders around text",
    },
    Info {
        feature: Feature::EmphasisMarks,
        section: "Characters and runs",
        name: "Emphasis marks, East Asian layout options",
    },
    Info {
        feature: Feature::FitText,
        section: "Characters and runs",
        name: "Fit text",
    },
    Info {
        feature: Feature::LanguageTags,
        section: "Characters and runs",
        name: "Language tags",
    },
    Info {
        feature: Feature::RightToLeftRuns,
        section: "Characters and runs",
        name: "Right-to-left and complex-script flags",
    },
    Info {
        feature: Feature::TextEffects,
        section: "Characters and runs",
        name: "Text effects (glow, shadow, outline, reflection, fill)",
    },
    Info {
        feature: Feature::OpenTypeFeatures,
        section: "Characters and runs",
        name: "OpenType features",
    },
    Info {
        feature: Feature::Hyperlinks,
        section: "Characters and runs",
        name: "Hyperlinks",
    },
    Info {
        feature: Feature::Ruby,
        section: "Characters and runs",
        name: "Ruby (phonetic guide)",
    },
    Info {
        feature: Feature::Alignment,
        section: "Paragraphs",
        name: "Alignment (incl. distribute, Thai, kashida variants)",
    },
    Info {
        feature: Feature::Indentation,
        section: "Paragraphs",
        name: "Indentation (left, right, first line, hanging, chars)",
    },
    Info {
        feature: Feature::ParagraphSpacing,
        section: "Paragraphs",
        name: "Spacing before/after, auto spacing, contextual spacing",
    },
    Info {
        feature: Feature::LineSpacing,
        section: "Paragraphs",
        name: "Line spacing (auto, exact, at least)",
    },
    Info {
        feature: Feature::TabStops,
        section: "Paragraphs",
        name: "Tab stops (all alignments and leaders, bar tabs)",
    },
    Info {
        feature: Feature::KeepAndWidowControl,
        section: "Paragraphs",
        name: "Keep with next, keep lines together, page break before, widow control",
    },
    Info {
        feature: Feature::ParagraphBordersAndShading,
        section: "Paragraphs",
        name: "Paragraph borders and shading (with border merging)",
    },
    Info {
        feature: Feature::OutlineLevel,
        section: "Paragraphs",
        name: "Outline level",
    },
    Info {
        feature: Feature::DropCapsAndFrames,
        section: "Paragraphs",
        name: "Drop caps and frames",
    },
    Info {
        feature: Feature::BidirectionalParagraphs,
        section: "Paragraphs",
        name: "Bidirectional paragraphs",
    },
    Info {
        feature: Feature::EastAsianLineBreaking,
        section: "Paragraphs",
        name: "Snap to grid, East Asian line-breaking options",
    },
    Info {
        feature: Feature::TextAlignmentAndDirection,
        section: "Paragraphs",
        name: "Text alignment on line, text direction",
    },
    Info {
        feature: Feature::SuppressLineNumbersAndHyphens,
        section: "Paragraphs",
        name: "Suppress line numbers, suppress auto hyphens",
    },
    Info {
        feature: Feature::ParagraphMarkFormatting,
        section: "Paragraphs",
        name: "Paragraph mark formatting",
    },
    Info {
        feature: Feature::DocumentDefaults,
        section: "Styles and themes",
        name: "Document defaults",
    },
    Info {
        feature: Feature::ParagraphAndCharacterStyles,
        section: "Styles and themes",
        name: "Paragraph, character, linked styles and inheritance",
    },
    Info {
        feature: Feature::TableStyles,
        section: "Styles and themes",
        name: "Table styles with conditional formatting",
    },
    Info {
        feature: Feature::NumberingStyles,
        section: "Styles and themes",
        name: "Numbering styles",
    },
    Info {
        feature: Feature::LatentStyles,
        section: "Styles and themes",
        name: "Latent styles",
    },
    Info {
        feature: Feature::ToggleProperties,
        section: "Styles and themes",
        name: "Toggle properties semantics",
    },
    Info {
        feature: Feature::ThemeFontsAndColors,
        section: "Styles and themes",
        name: "Theme fonts and colors",
    },
    Info {
        feature: Feature::StyleAutoRedefinition,
        section: "Styles and themes",
        name: "Style auto-redefinition (honored, but never silently)",
    },
    Info {
        feature: Feature::AbstractNumbering,
        section: "Numbering and lists",
        name: "Abstract numbering and instances, level overrides",
    },
    Info {
        feature: Feature::NumberFormats,
        section: "Numbering and lists",
        name: "Number formats (decimal, roman, letters, ordinal, cardinal text, East Asian, Hebrew, Arabic and more)",
    },
    Info {
        feature: Feature::LevelTextAndSuffix,
        section: "Numbering and lists",
        name: "Level text, suffix, justification, legal numbering",
    },
    Info {
        feature: Feature::RestartRules,
        section: "Numbering and lists",
        name: "Restart rules",
    },
    Info {
        feature: Feature::PictureBullets,
        section: "Numbering and lists",
        name: "Picture bullets",
    },
    Info {
        feature: Feature::LegacyNumbering,
        section: "Numbering and lists",
        name: "Legacy numbering",
    },
    Info {
        feature: Feature::HeadingNumbering,
        section: "Numbering and lists",
        name: "Heading outline numbering",
    },
    Info {
        feature: Feature::PageSizeAndMargins,
        section: "Sections and page layout",
        name: "Page size, orientation, margins, gutter",
    },
    Info {
        feature: Feature::SectionBreaks,
        section: "Sections and page layout",
        name: "Section breaks (next page, continuous, even, odd)",
    },
    Info {
        feature: Feature::Columns,
        section: "Sections and page layout",
        name: "Columns (equal, unequal, separator) and column balancing",
    },
    Info {
        feature: Feature::PageVerticalAlignment,
        section: "Sections and page layout",
        name: "Vertical alignment of page",
    },
    Info {
        feature: Feature::PageBorders,
        section: "Sections and page layout",
        name: "Page borders (including art borders)",
    },
    Info {
        feature: Feature::LineNumbering,
        section: "Sections and page layout",
        name: "Line numbering",
    },
    Info {
        feature: Feature::PageNumbering,
        section: "Sections and page layout",
        name: "Page numbering format and restart",
    },
    Info {
        feature: Feature::DocumentGrid,
        section: "Sections and page layout",
        name: "Document grid (East Asian)",
    },
    Info {
        feature: Feature::SectionTextDirection,
        section: "Sections and page layout",
        name: "Text direction of section, right-to-left gutter",
    },
    Info {
        feature: Feature::MirrorMarginsAndBookFold,
        section: "Sections and page layout",
        name: "Mirror margins, book fold, gutter at top",
    },
    Info {
        feature: Feature::HeadersAndFooters,
        section: "Headers and footers",
        name: "Default, first-page, even headers and footers",
    },
    Info {
        feature: Feature::LinkToPrevious,
        section: "Headers and footers",
        name: "Link to previous (inheritance across sections)",
    },
    Info {
        feature: Feature::HeaderFooterDistances,
        section: "Headers and footers",
        name: "Header/footer distances and growth into body",
    },
    Info {
        feature: Feature::Watermarks,
        section: "Headers and footers",
        name: "Watermarks (VML/WordArt in header)",
    },
    Info {
        feature: Feature::TableGrid,
        section: "Tables",
        name: "Grid, widths, fixed layout",
    },
    Info {
        feature: Feature::TableAutofit,
        section: "Tables",
        name: "Autofit layout algorithm",
    },
    Info {
        feature: Feature::CellMerges,
        section: "Tables",
        name: "Horizontal and vertical merges",
    },
    Info {
        feature: Feature::TableBordersAndShading,
        section: "Tables",
        name: "Borders (with conflict resolution) and shading",
    },
    Info {
        feature: Feature::CellMarginsAndSpacing,
        section: "Tables",
        name: "Cell margins and spacing",
    },
    Info {
        feature: Feature::RowHeightAndHeaderRows,
        section: "Tables",
        name: "Row height rules, cannot split, header rows",
    },
    Info {
        feature: Feature::RowsSplittingAcrossPages,
        section: "Tables",
        name: "Rows splitting across pages",
    },
    Info {
        feature: Feature::NestedTables,
        section: "Tables",
        name: "Nested tables",
    },
    Info {
        feature: Feature::FloatingTables,
        section: "Tables",
        name: "Floating tables",
    },
    Info {
        feature: Feature::RightToLeftTables,
        section: "Tables",
        name: "Right-to-left tables",
    },
    Info {
        feature: Feature::CellTextDirection,
        section: "Tables",
        name: "Cell text direction, vertical alignment",
    },
    Info {
        feature: Feature::TableFormulas,
        section: "Tables",
        name: "Table formulas",
    },
    Info {
        feature: Feature::Fields,
        section: "Fields",
        name: "Complex and simple fields, nesting",
    },
    Info {
        feature: Feature::PageFields,
        section: "Fields",
        name: "Page fields",
    },
    Info {
        feature: Feature::DateAndTimeFields,
        section: "Fields",
        name: "Date and time with picture switches and calendars",
    },
    Info {
        feature: Feature::CrossReferences,
        section: "Fields",
        name: "Cross-references and sequences",
    },
    Info {
        feature: Feature::DocumentInformationFields,
        section: "Fields",
        name: "Document information",
    },
    Info {
        feature: Feature::TablesOfContents,
        section: "Fields",
        name: "Tables of contents and figures",
    },
    Info {
        feature: Feature::FormulasAndConditions,
        section: "Fields",
        name: "Formulas and conditions",
    },
    Info {
        feature: Feature::MailMergeFields,
        section: "Fields",
        name: "Mail merge fields",
    },
    Info {
        feature: Feature::IndexesAndAuthorities,
        section: "Fields",
        name: "Indexes and authorities",
    },
    Info {
        feature: Feature::Citations,
        section: "Fields",
        name: "Citations and bibliography",
    },
    Info {
        feature: Feature::LegacyFormFields,
        section: "Fields",
        name: "Legacy form fields",
    },
    Info {
        feature: Feature::ExternalFields,
        section: "Fields",
        name: "External and active fields (blocked by default)",
    },
    Info {
        feature: Feature::FieldFormattingSwitches,
        section: "Fields",
        name: "Formatting switches",
    },
    Info {
        feature: Feature::FootnotesAndEndnotes,
        section: "Notes, comments and review",
        name: "Footnotes and endnotes (placement, numbering, separators, continuation)",
    },
    Info {
        feature: Feature::Comments,
        section: "Notes, comments and review",
        name: "Comments with threads, resolution, people",
    },
    Info {
        feature: Feature::TrackedInsertionsAndDeletions,
        section: "Notes, comments and review",
        name: "Tracked insertions and deletions",
    },
    Info {
        feature: Feature::TrackedMoves,
        section: "Notes, comments and review",
        name: "Tracked moves",
    },
    Info {
        feature: Feature::TrackedFormatting,
        section: "Notes, comments and review",
        name: "Tracked formatting and property changes",
    },
    Info {
        feature: Feature::MarkupDisplayModes,
        section: "Notes, comments and review",
        name: "Markup display modes and balloons",
    },
    Info {
        feature: Feature::CompareAndCombine,
        section: "Notes, comments and review",
        name: "Compare and combine documents",
    },
    Info {
        feature: Feature::RestrictEditing,
        section: "Notes, comments and review",
        name: "Restrict editing, permissions",
    },
    Info {
        feature: Feature::Pictures,
        section: "Drawings and objects",
        name: "Inline and anchored pictures",
    },
    Info {
        feature: Feature::ImageFormats,
        section: "Drawings and objects",
        name: "Image formats (PNG, JPEG, GIF, BMP, TIFF, WebP, SVG, EMF, WMF, EMF+)",
    },
    Info {
        feature: Feature::PictureAdjustments,
        section: "Drawings and objects",
        name: "Cropping, recoloring, transparency, basic effects",
    },
    Info {
        feature: Feature::TextWrapping,
        section: "Drawings and objects",
        name: "Text wrapping (square, tight, through, top and bottom, behind, in front)",
    },
    Info {
        feature: Feature::ObjectPositioning,
        section: "Drawings and objects",
        name: "Positioning relative to page, margin, column, paragraph, line, character",
    },
    Info {
        feature: Feature::Shapes,
        section: "Drawings and objects",
        name: "Shapes: preset and custom geometry, fills, lines, arrowheads",
    },
    Info {
        feature: Feature::TextBoxes,
        section: "Drawings and objects",
        name: "Text boxes and linked text boxes",
    },
    Info {
        feature: Feature::GroupsAndCanvases,
        section: "Drawings and objects",
        name: "Groups and drawing canvases",
    },
    Info {
        feature: Feature::ShapeEffects,
        section: "Drawings and objects",
        name: "Shape effects (shadow, glow, soft edges, 3D)",
    },
    Info {
        feature: Feature::WordArt,
        section: "Drawings and objects",
        name: "WordArt",
    },
    Info {
        feature: Feature::VmlShapes,
        section: "Drawings and objects",
        name: "Legacy VML shapes and pictures",
    },
    Info {
        feature: Feature::OleObjects,
        section: "Drawings and objects",
        name: "OLE objects (preview only; never activated)",
    },
    Info {
        feature: Feature::Ink,
        section: "Drawings and objects",
        name: "Ink",
    },
    Info {
        feature: Feature::Models3d,
        section: "Drawings and objects",
        name: "3D models",
    },
    Info {
        feature: Feature::Equations,
        section: "Equations, charts and diagrams",
        name: "Equations (OMML)",
    },
    Info {
        feature: Feature::LegacyEquations,
        section: "Equations, charts and diagrams",
        name: "Legacy equations (Equation Editor 3, MathType as OLE with WMF preview)",
    },
    Info {
        feature: Feature::Charts,
        section: "Equations, charts and diagrams",
        name: "Charts (bar, column, line, pie, area, scatter, combo)",
    },
    Info {
        feature: Feature::NewerChartTypes,
        section: "Equations, charts and diagrams",
        name: "Newer chart types (waterfall, treemap, sunburst, histogram, box and whisker, funnel, map)",
    },
    Info {
        feature: Feature::Charts3d,
        section: "Equations, charts and diagrams",
        name: "3D charts",
    },
    Info {
        feature: Feature::SmartArt,
        section: "Equations, charts and diagrams",
        name: "SmartArt (from Word's drawing cache)",
    },
    Info {
        feature: Feature::ContentControls,
        section: "Content controls and forms",
        name: "Rich text, plain text, picture, checkbox, combo box, drop-down, date controls",
    },
    Info {
        feature: Feature::RepeatingSections,
        section: "Content controls and forms",
        name: "Repeating sections, building block galleries",
    },
    Info {
        feature: Feature::XmlDataBinding,
        section: "Content controls and forms",
        name: "XML data binding",
    },
    Info {
        feature: Feature::ArabicAndHebrew,
        section: "East Asian and complex scripts",
        name: "Arabic and Hebrew shaping and bidi",
    },
    Info {
        feature: Feature::Kashida,
        section: "East Asian and complex scripts",
        name: "Kashida justification",
    },
    Info {
        feature: Feature::Kinsoku,
        section: "East Asian and complex scripts",
        name: "Kinsoku (line-breaking rules) incl. custom lists",
    },
    Info {
        feature: Feature::AsianAutoSpacing,
        section: "East Asian and complex scripts",
        name: "Auto-spacing between Asian and Latin text and numbers",
    },
    Info {
        feature: Feature::CharacterGrid,
        section: "East Asian and complex scripts",
        name: "Character grid",
    },
    Info {
        feature: Feature::VerticalText,
        section: "East Asian and complex scripts",
        name: "Vertical text",
    },
    Info {
        feature: Feature::CombineCharacters,
        section: "East Asian and complex scripts",
        name: "Combine characters, two lines in one, enclosed characters",
    },
    Info {
        feature: Feature::IndicAndSoutheastAsian,
        section: "East Asian and complex scripts",
        name: "Indic and Southeast Asian shaping and breaking",
    },
    Info {
        feature: Feature::CompatibilityMode15,
        section: "Compatibility settings",
        name: "Compatibility mode 15 (Word 2013+)",
    },
    Info {
        feature: Feature::OlderCompatibilityModes,
        section: "Compatibility settings",
        name: "Compatibility modes 14, 12, 11",
    },
    Info {
        feature: Feature::LegacyCompatibilityOptions,
        section: "Compatibility settings",
        name: "Individual legacy compatibility options (dozens)",
    },
];

/// Features that scanning a package cannot detect, with the reason. They are never tagged; reports list them as not detectable.
pub const NOT_DETECTABLE: [(Feature, &str); 4] = [
    (
        Feature::PasswordEncryption,
        "an encrypted document is not a ZIP package but an OLE compound file; the scanner refuses it as such",
    ),
    (
        Feature::RowsSplittingAcrossPages,
        "an outcome of layout, which only a layout engine or Word's ground truth shows",
    ),
    (
        Feature::MarkupDisplayModes,
        "a way of viewing tracked changes, not something a document contains",
    ),
    (
        Feature::CompareAndCombine,
        "an editing command, not something a document contains",
    ),
];

impl Feature {
    /// The feature's place in the matrix.
    #[must_use]
    pub fn info(self) -> &'static Info {
        // `FEATURES` lists every variant at its own index (checked by a test).
        &FEATURES[self as usize]
    }

    /// The feature's name in the coverage matrix.
    #[must_use]
    pub fn name(self) -> &'static str {
        self.info().name
    }

    /// The feature with this coverage-matrix name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        FEATURES
            .iter()
            .find(|info| info.name == name)
            .map(|info| info.feature)
    }

    /// Why scanning cannot detect this feature, if it cannot.
    #[must_use]
    pub fn not_detectable(self) -> Option<&'static str> {
        NOT_DETECTABLE
            .iter()
            .find(|(feature, _)| *feature == self)
            .map(|(_, reason)| *reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_is_listed_once_in_order_under_a_unique_name() {
        let mut names = std::collections::BTreeSet::new();
        for (index, info) in FEATURES.iter().enumerate() {
            assert_eq!(
                info.feature as usize, index,
                "{} is out of place",
                info.name
            );
            assert!(names.insert(info.name), "{} appears twice", info.name);
            assert_eq!(Feature::from_name(info.name), Some(info.feature));
            assert!(!info.name.is_empty() && info.name.trim() == info.name);
        }
        assert_eq!(Feature::from_name("No such feature"), None);
    }

    #[test]
    fn the_sections_follow_the_matrix() {
        let sections: Vec<&str> =
            FEATURES
                .iter()
                .map(|info| info.section)
                .fold(Vec::new(), |mut sections, section| {
                    if sections.last() != Some(&section) {
                        sections.push(section);
                    }
                    sections
                });
        assert_eq!(
            sections,
            [
                "Package and document structure",
                "Characters and runs",
                "Paragraphs",
                "Styles and themes",
                "Numbering and lists",
                "Sections and page layout",
                "Headers and footers",
                "Tables",
                "Fields",
                "Notes, comments and review",
                "Drawings and objects",
                "Equations, charts and diagrams",
                "Content controls and forms",
                "East Asian and complex scripts",
                "Compatibility settings",
            ]
        );
    }
}
