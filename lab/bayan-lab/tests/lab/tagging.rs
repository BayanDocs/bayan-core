//! The tagging rules (`lab/corpus/tagging-rules.md`) on small synthetic documents, one behavior per test.

use bayan_lab::scan::features::Feature;
use bayan_lab::scan::scripts::Script;
use bayan_lab::scan::{Findings, Limits, scan};

use crate::support::{DOCUMENT_TYPE, Docx, ZipEntry, document_xml, rel, zip};

fn tag(docx: &Docx) -> Findings {
    scan(&docx.bytes(), &Limits::DEFAULT).unwrap()
}

fn tag_body(body: &str) -> Findings {
    tag(&Docx::new(body))
}

fn has(findings: &Findings, feature: Feature) -> bool {
    findings.features.contains(&feature)
}

fn script(findings: &Findings, script: Script) -> u64 {
    findings.scripts.get(&script).copied().unwrap_or(0)
}

/// A paragraph with one run of text.
fn paragraph(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

const STYLES: &str =
    r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;
const SETTINGS: &str =
    r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;

#[test]
fn a_minimal_document() {
    let found = tag_body(&paragraph("Hello"));
    // Without a `compatibilityMode` setting, Word lays a document out in Word 2007's mode, 12.
    assert_eq!(
        found.features.iter().copied().collect::<Vec<_>>(),
        [
            Feature::OpcPackage,
            Feature::TextTabsBreaksSymbols,
            Feature::OlderCompatibilityModes
        ]
    );
    assert_eq!(
        found.scripts.into_iter().collect::<Vec<_>>(),
        [(Script::Latin, 5)]
    );
    assert!(found.fonts.is_empty() && found.languages.is_empty());
    assert_eq!(found.compatibility_mode, None);
    assert_eq!(found.application, None);
    assert!(found.notes.is_empty());
}

/// A shape as Word 2010 and later write it: a DrawingML shape, with a VML copy for older readers.
const SHAPE_WITH_FALLBACK: &str = concat!(
    r#"<w:p><w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor>"#,
    r#"<wp:wrapSquare wrapText="bothSides"/><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">"#,
    r#"<wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>Inside</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp>"#,
    r#"</a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice>"#,
    r#"<mc:Fallback><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>Inside</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback>"#,
    r#"</mc:AlternateContent></w:r></w:p>"#
);

#[test]
fn markup_compatibility_takes_the_choice_that_word_takes() {
    // Regression test: `Requires` is an unqualified attribute; reading it in the MC namespace made every Choice look unreadable, so only the VML fallbacks were tagged.
    let found = tag_body(SHAPE_WITH_FALLBACK);
    for feature in [
        Feature::MarkupCompatibility,
        Feature::Shapes,
        Feature::TextBoxes,
        Feature::TextWrapping,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    assert!(
        !has(&found, Feature::VmlShapes),
        "the VML fallback was read"
    );
    // The text of the shape is counted once, not once per branch.
    assert_eq!(script(&found, Script::Latin), 6);
}

#[test]
fn a_choice_with_requirements_the_tagger_does_not_know_falls_back() {
    for requires in [
        // A namespace the tagger does not know.
        r#"<mc:Choice Requires="x" xmlns:x="urn:example:unknown">"#,
        // A prefix that is not declared.
        r#"<mc:Choice Requires="undeclared">"#,
        // No requirement at all.
        r#"<mc:Choice Requires="">"#,
        // More prefixes than the limit of 16.
        &format!(r#"<mc:Choice Requires="{}">"#, ["wps"; 17].join(" ")),
    ] {
        let body = SHAPE_WITH_FALLBACK.replace(r#"<mc:Choice Requires="wps">"#, requires);
        let found = tag_body(&body);
        assert!(has(&found, Feature::VmlShapes), "{requires}");
        assert!(!has(&found, Feature::Shapes), "{requires}");
        assert_eq!(script(&found, Script::Latin), 6, "{requires}");
    }
}

#[test]
fn ignorable_content_and_unknown_elements_are_extensions_that_are_not_read() {
    let body = concat!(
        r#"<w:p><w:r><w:t>Seen</w:t></w:r>"#,
        r#"<x:custom xmlns:x="urn:example:extension"><w:r><w:t>Unseen</w:t></w:r></x:custom></w:p>"#,
    );
    let found = tag_body(body);
    assert!(has(&found, Feature::UnknownPartsAndExtensions));
    assert_eq!(script(&found, Script::Latin), 4);
    let ignorable = Docx::with_main(
        DOCUMENT_TYPE,
        document_xml(&paragraph("x"))
            .replace("<w:document ", r#"<w:document mc:Ignorable="w14" "#)
            .as_bytes(),
    );
    assert!(has(&tag(&ignorable), Feature::MarkupCompatibility));
}

#[test]
fn strict_documents_are_recognized() {
    let document = r#"<w:document xmlns:w="http://purl.oclc.org/ooxml/wordprocessingml/main"><w:body><w:p><w:r><w:t>Strict</w:t></w:r></w:p></w:body></w:document>"#;
    let entries = vec![
        ZipEntry::stored(
            "[Content_Types].xml",
            format!(r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/word/document.xml" ContentType="{DOCUMENT_TYPE}"/></Types>"#).as_bytes(),
        ),
        ZipEntry::stored(
            "_rels/.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        ZipEntry::stored("word/document.xml", document.as_bytes()),
    ];
    let found = scan(&zip(&entries), &Limits::DEFAULT).unwrap();
    assert!(has(&found, Feature::StrictNamespaces));
    assert_eq!(script(&found, Script::Latin), 6);
    // Strict spells text directions differently: `tb` is horizontal, `lr` vertical, read from the bottom up.
    for (direction, vertical) in [
        ("tb", false),
        ("lr", true),
        ("rl", true),
        ("lrTb", false),
        ("tbRl", true),
    ] {
        let mut entries = entries.clone();
        let paragraph = format!(
            r#"<w:p><w:pPr><w:textDirection w:val="{direction}"/></w:pPr><w:r><w:t>Strict</w:t></w:r></w:p>"#
        );
        if let Some(entry) = entries.last_mut() {
            *entry = ZipEntry::stored(
                "word/document.xml",
                document
                    .replace("<w:p><w:r><w:t>Strict</w:t></w:r></w:p>", &paragraph)
                    .as_bytes(),
            );
        }
        let found = scan(&zip(&entries), &Limits::DEFAULT).unwrap();
        assert_eq!(has(&found, Feature::VerticalText), vertical, "{direction}");
    }
}

/// A complex field whose instruction is split over several runs, as Word often writes it.
fn complex_field(instruction_parts: &[&str]) -> String {
    let mut runs = String::from(r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#);
    for part in instruction_parts {
        runs.push_str(&format!(
            r#"<w:r><w:instrText xml:space="preserve">{part}</w:instrText></w:r>"#
        ));
    }
    runs.push_str(r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#);
    format!("<w:p>{runs}</w:p>")
}

#[test]
fn fields_are_classified_by_their_instruction() {
    let found = tag_body(&complex_field(&[" PA", "GE \\* MERGEFORMAT "]));
    for feature in [
        Feature::Fields,
        Feature::PageFields,
        Feature::FieldFormattingSwitches,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    for (instruction, feature) in [
        (
            " HYPERLINK \"https://example.invalid\" ",
            Feature::Hyperlinks,
        ),
        (" INCLUDETEXT \"c:\\\\x.docx\" ", Feature::ExternalFields),
        (" DATE \\@ \"d MMMM yyyy\" ", Feature::DateAndTimeFields),
        (" MERGEFIELD Name ", Feature::MailMergeFields),
        (" TOC \\o \"1-3\" ", Feature::TablesOfContents),
        (" = 2 + 2 ", Feature::FormulasAndConditions),
        (" EQ \\o\\ac(○,字) ", Feature::CombineCharacters),
    ] {
        let simple = format!(
            r#"<w:p><w:fldSimple w:instr="{}"><w:r><w:t>x</w:t></w:r></w:fldSimple></w:p>"#,
            instruction.replace('"', "&quot;")
        );
        assert!(has(&tag_body(&simple), feature), "{instruction}");
        assert!(
            has(&tag_body(&complex_field(&[instruction])), feature),
            "{instruction}"
        );
    }
    // A formula in a table cell is a table formula.
    let in_table = format!(
        "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>",
        complex_field(&[" = SUM(ABOVE) "])
    );
    let found = tag_body(&in_table);
    assert!(has(&found, Feature::TableFormulas));
    assert!(!has(&found, Feature::FormulasAndConditions));
    // Nested fields are each classified.
    let nested = concat!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> IF </w:instrText></w:r>"#,
        r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:r><w:instrText> = 1 "a" "b" </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
    );
    let found = tag_body(nested);
    assert!(has(&found, Feature::PageFields));
    assert!(has(&found, Feature::FormulasAndConditions));
}

#[test]
fn tables_layout_nesting_and_merges() {
    let inner = "<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>";
    let body = format!(
        concat!(
            r#"<w:tbl><w:tblPr><w:tblLayout w:type="fixed"/></w:tblPr><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#,
            "<w:tbl><w:tr><w:tc>{}<w:p/></w:tc></w:tr></w:tbl>"
        ),
        inner
    );
    let found = tag_body(&body);
    for feature in [
        Feature::TableGrid,
        Feature::NestedTables,
        Feature::CellMerges,
        Feature::TableAutofit,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    let fixed_only = tag_body(
        r#"<w:tbl><w:tblPr><w:tblLayout w:type="fixed"/></w:tblPr><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    assert!(!has(&fixed_only, Feature::TableAutofit));
}

#[test]
fn table_formatting_in_the_styles_counts_only_in_documents_with_tables() {
    // Every Word document's styles define "Normal Table" with cell margins.
    let styles = format!(
        r#"{STYLES}<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:tblPr><w:tblCellMar><w:left w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style></w:styles>"#
    );
    let without = tag(&Docx::new(&paragraph("x")).part("word/styles.xml", "styles", &styles));
    assert!(!has(&without, Feature::CellMarginsAndSpacing));
    let with = tag(
        &Docx::new("<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>").part(
            "word/styles.xml",
            "styles",
            &styles,
        ),
    );
    assert!(has(&with, Feature::CellMarginsAndSpacing));
}

#[test]
fn sections_headers_and_inheritance() {
    let header = format!(
        r#"<w:hdr {}>{}</w:hdr>"#,
        crate::support::NAMESPACES,
        paragraph("Header")
    );
    let body = concat!(
        r#"<w:p><w:pPr><w:sectPr><w:headerReference w:type="default" r:id="rId2"/><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr></w:p>"#,
        r#"<w:p><w:r><w:t>Body</w:t></w:r></w:p><w:sectPr><w:type w:val="continuous"/></w:sectPr>"#,
    );
    let found = tag(&Docx::new(body).part("word/header1.xml", "header", &header));
    for feature in [
        Feature::SectionBreaks,
        Feature::HeadersAndFooters,
        Feature::LinkToPrevious,
        Feature::PageSizeAndMargins,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    // The header's text is counted with the document's.
    assert_eq!(script(&found, Script::Latin), 10);
    // A single section breaks nothing and inherits nothing, whatever its type says.
    let single = tag_body(
        r#"<w:p/><w:sectPr><w:type w:val="nextPage"/><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>"#,
    );
    assert!(!has(&single, Feature::SectionBreaks));
    assert!(!has(&single, Feature::LinkToPrevious));
}

#[test]
fn languages_that_word_declares_everywhere_count_only_with_matching_text() {
    let styles = format!(
        r#"{STYLES}<w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="en-us" w:eastAsia="ja-JP" w:bidi="ar-SA"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
    );
    let languages = |text: &str| -> Vec<String> {
        let found = tag(&Docx::new(&paragraph(text)).part("word/styles.xml", "styles", &styles));
        assert!(has(&found, Feature::LanguageTags));
        assert!(has(&found, Feature::DocumentDefaults));
        found.languages.into_iter().collect()
    };
    assert_eq!(languages("Latin only"), ["en-US"]);
    assert_eq!(languages("日本語"), ["en-US", "ja-JP"]);
    assert_eq!(languages("مرحبا"), ["ar-SA", "en-US"]);
    let arabic = tag_body(&paragraph("مرحبا"));
    assert!(has(&arabic, Feature::ArabicAndHebrew));
    assert_eq!(script(&arabic, Script::Arabic), 5);
}

#[test]
fn compatibility_mode_and_legacy_options() {
    let settings = |compat: &str| {
        let xml = format!(r"{SETTINGS}<w:compat>{compat}</w:compat></w:settings>");
        tag(&Docx::new(&paragraph("x")).part("word/settings.xml", "settings", &xml))
    };
    let modern = settings(
        r#"<w:doNotExpandShiftReturn/><w:useFELayout w:val="0"/><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/>"#,
    );
    assert_eq!(modern.compatibility_mode, Some(15));
    assert!(has(&modern, Feature::CompatibilityMode15));
    assert!(has(&modern, Feature::LegacyCompatibilityOptions));
    // An option whose value is off is not switched on.
    assert_eq!(
        modern.compatibility_options.into_iter().collect::<Vec<_>>(),
        ["doNotExpandShiftReturn"]
    );
    let older = settings(
        r#"<w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="14"/>"#,
    );
    assert_eq!(older.compatibility_mode, Some(14));
    assert!(has(&older, Feature::OlderCompatibilityModes));
    assert!(!has(&older, Feature::LegacyCompatibilityOptions));
    // Only Word's own setting, with Word's namespace in `w:uri`, is the compatibility mode; without it, Word uses mode 12.
    for foreign in [
        r#"<w:compatSetting w:name="compatibilityMode" w:val="15"/>"#,
        r#"<w:compatSetting w:name="compatibilityMode" w:uri="urn:example:another-program" w:val="15"/>"#,
        "",
    ] {
        let found = settings(foreign);
        assert_eq!(found.compatibility_mode, None, "{foreign}");
        assert!(has(&found, Feature::OlderCompatibilityModes), "{foreign}");
        assert!(!has(&found, Feature::CompatibilityMode15), "{foreign}");
    }
}

#[test]
fn application_properties_name_the_program_and_hint_the_pages() {
    let app = r#"<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Application>LibreOffice/7.3.6.2$Linux_X86_64 LibreOffice_project/30$Build-2</Application><Pages>3</Pages></Properties>"#;
    let found = tag(&Docx::new(&paragraph("x")).part_from(
        "",
        "docProps/app.xml",
        &rel::of("extended-properties"),
        app,
    ));
    assert!(has(&found, Feature::DocumentProperties));
    assert_eq!(found.application.as_deref(), Some("LibreOffice/7.3.6.2"));
    assert_eq!(found.pages_hint, Some(3));
    let overlong = app.replace("LibreOffice", &"L".repeat(200));
    let found = tag(&Docx::new(&paragraph("x")).part_from(
        "",
        "docProps/app.xml",
        &rel::of("extended-properties"),
        &overlong,
    ));
    assert_eq!(found.application, None);
}

#[test]
fn theme_fonts_count_only_when_the_document_uses_them() {
    let theme = concat!(
        r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office"><a:themeElements><a:fontScheme name="Office">"#,
        r#"<a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>"#,
        r#"<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>"#,
        r#"</a:fontScheme><a:fmtScheme name="Office"><a:effectStyleLst><a:effectStyle><a:effectLst><a:outerShdw/></a:effectLst></a:effectStyle></a:effectStyleLst></a:fmtScheme></a:themeElements></a:theme>"#,
    );
    let body = r#"<w:p><w:r><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi" w:eastAsia="SimSun"/></w:rPr><w:t>x</w:t></w:r></w:p>"#;
    let found = tag(&Docx::new(body).part("word/theme/theme1.xml", "theme", theme));
    assert_eq!(
        found.fonts.iter().map(String::as_str).collect::<Vec<_>>(),
        ["Calibri", "SimSun"]
    );
    assert!(has(&found, Feature::ThemeFontsAndColors));
    assert!(has(&found, Feature::FontsPerScript));
    // The theme's own effect styles are not effects the document uses.
    assert!(!has(&found, Feature::ShapeEffects));
}

#[test]
fn numbering_definitions_and_picture_bullets() {
    let numbering = format!(
        concat!(
            r#"<w:numbering {}><w:numPicBullet w:numPicBulletId="0"><w:pict><v:shape><v:imagedata r:id="rId1"/></v:shape></w:pict></w:numPicBullet>"#,
            r#"<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/><w:lvlPicBulletId w:val="0"/></w:lvl></w:abstractNum>"#,
            r#"<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
        ),
        crate::support::NAMESPACES
    );
    let found =
        tag(&Docx::new(&paragraph("x")).part("word/numbering.xml", "numbering", &numbering));
    for feature in [
        Feature::AbstractNumbering,
        Feature::NumberFormats,
        Feature::LevelTextAndSuffix,
        Feature::PictureBullets,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    // The bullet's picture defines a bullet; it is not a shape in the document.
    assert!(!has(&found, Feature::VmlShapes));
    assert!(!has(&found, Feature::ImageFormats));
}

#[test]
fn numbering_styles_need_numbering_properties() {
    let styles = |numbered: &str| {
        format!(
            r#"{STYLES}<w:style w:type="numbering" w:styleId="List"><w:name w:val="List"/>{numbered}</w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:pPr>{numbered}</w:pPr></w:style></w:styles>"#
        )
    };
    let none = tag(&Docx::new(&paragraph("x")).part("word/styles.xml", "styles", &styles("")));
    assert!(!has(&none, Feature::NumberingStyles));
    assert!(!has(&none, Feature::HeadingNumbering));
    assert!(has(&none, Feature::ParagraphAndCharacterStyles));
    let numbered = tag(&Docx::new(&paragraph("x")).part(
        "word/styles.xml",
        "styles",
        &styles(r#"<w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr>"#),
    ));
    assert!(has(&numbered, Feature::NumberingStyles));
    assert!(has(&numbered, Feature::HeadingNumbering));
}

#[test]
fn relationships_alone_reveal_parts_of_the_package() {
    let docx = Docx::new(&paragraph("x"))
        .part_from("", "customXml/item1.xml", &rel::of("customXml"), "<data/>")
        .part_from("", "_xmlsignatures/origin.sigs", rel::DIGITAL_SIGNATURE, "")
        .part_from("", "docProps/core.xml", rel::CORE_PROPERTIES, "<core/>")
        // The glossary document is noted but never read: this one is not even XML.
        .part(
            "word/glossary/document.xml",
            "glossaryDocument",
            "<<not xml",
        )
        .part("word/fonts/font1.odttf", "font", "")
        .part("word/media/image1.png", "image", "")
        .relationship(
            "word/document.xml",
            &rel::of("hyperlink"),
            "https://example.invalid/",
            true,
        );
    let found = tag(&docx);
    for feature in [
        Feature::CustomXmlParts,
        Feature::DigitalSignatures,
        Feature::DocumentProperties,
        Feature::GlossaryDocument,
        Feature::EmbeddedFonts,
        Feature::ImageFormats,
        Feature::Hyperlinks,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    let unknown = tag(&Docx::new(&paragraph("x")).part_from(
        "",
        "custom/thing.xml",
        "urn:example:relationship",
        "<thing/>",
    ));
    assert!(has(&unknown, Feature::UnknownPartsAndExtensions));
}

#[test]
fn a_relationship_to_a_missing_part_is_noted() {
    let docx = Docx::new(&paragraph("x")).relationship(
        "word/document.xml",
        &rel::of("styles"),
        "styles.xml",
        false,
    );
    let found = tag(&docx);
    assert_eq!(
        found.notes.into_iter().collect::<Vec<_>>(),
        ["a relationship points to a part that is missing"]
    );
}

#[test]
fn east_asian_and_vertical_layout() {
    let body = concat!(
        r#"<w:p><w:pPr><w:jc w:val="lowKashida"/><w:kinsoku w:val="0"/><w:textDirection w:val="tbRl"/></w:pPr>"#,
        r#"<w:r><w:rPr><w:eastAsianLayout w:id="1" w:combine="1"/><w:em w:val="dot"/></w:rPr><w:t>漢字</w:t></w:r></w:p>"#,
        r#"<w:sectPr><w:docGrid w:type="linesAndChars" w:linePitch="360"/></w:sectPr>"#,
    );
    let found = tag_body(body);
    for feature in [
        Feature::Alignment,
        Feature::Kashida,
        Feature::Kinsoku,
        Feature::EastAsianLineBreaking,
        Feature::VerticalText,
        Feature::CombineCharacters,
        Feature::EmphasisMarks,
        Feature::DocumentGrid,
        Feature::CharacterGrid,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    assert_eq!(script(&found, Script::Han), 2);
}

#[test]
fn scripts_are_counted_by_character() {
    let found = tag_body(&paragraph("ab αβ אב مر क ไ 中 あ ア 한 1+2"));
    for (expected, count) in [
        (Script::Latin, 2),
        (Script::Greek, 2),
        (Script::Hebrew, 2),
        (Script::Arabic, 2),
        (Script::Devanagari, 1),
        (Script::Thai, 1),
        (Script::Han, 1),
        (Script::Hiragana, 1),
        (Script::Katakana, 1),
        (Script::Hangul, 1),
    ] {
        assert_eq!(script(&found, expected), count, "{}", expected.name());
    }
    // Digits, punctuation and spaces belong to no script.
    assert_eq!(found.scripts.values().sum::<u64>(), 14);
    assert!(has(&found, Feature::ArabicAndHebrew));
    assert!(has(&found, Feature::IndicAndSoutheastAsian));
}

#[test]
fn font_names_are_recorded_within_limits() {
    let mut fonts = String::new();
    for number in 0..250 {
        fonts.push_str(&format!(r#"<w:font w:name="Font {number:03}"/>"#));
    }
    fonts.push_str(&format!(r#"<w:font w:name="{}"/>"#, "N".repeat(65)));
    let table = format!(
        r#"<w:fonts xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{fonts}</w:fonts>"#
    );
    let found = tag(&Docx::new(&paragraph("x")).part("word/fontTable.xml", "fontTable", &table));
    assert_eq!(found.fonts.len(), 200);
    assert_eq!(
        found.notes.into_iter().collect::<Vec<_>>(),
        [
            "a font name longer than 64 characters was not recorded",
            "more fonts than the limit of 200; the list is cut",
        ]
    );
}

#[test]
fn charts_their_types_and_fonts() {
    let chart = concat!(
        r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
        r#"<c:chart><c:plotArea><c:bar3DChart/></c:plotArea></c:chart><c:txPr><a:p><a:r><a:rPr><a:latin typeface="Arial"/></a:rPr></a:r></a:p></c:txPr></c:chartSpace>"#,
    );
    let found = tag(&Docx::new(&paragraph("x")).part("word/charts/chart1.xml", "chart", chart));
    assert!(has(&found, Feature::Charts));
    assert!(has(&found, Feature::Charts3d));
    assert!(found.fonts.contains("Arial"));
}

#[test]
fn review_comments_and_content_controls() {
    let body = concat!(
        r#"<w:p><w:commentRangeStart w:id="0"/><w:ins w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z"><w:r><w:t>new</w:t></w:r></w:ins>"#,
        r#"<w:del w:id="2" w:author="A"><w:r><w:delText>old</w:delText></w:r></w:del>"#,
        r#"<w:r><w:rPr><w:b/><w:rPrChange w:id="3" w:author="A"><w:rPr/></w:rPrChange></w:rPr><w:t>b</w:t></w:r></w:p>"#,
        r#"<w:sdt><w:sdtPr><w:dataBinding w:xpath="/a" w:storeItemID="{00000000-0000-0000-0000-000000000000}"/></w:sdtPr><w:sdtContent><w:p/></w:sdtContent></w:sdt>"#,
    );
    let found = tag_body(body);
    for feature in [
        Feature::Comments,
        Feature::TrackedInsertionsAndDeletions,
        Feature::TrackedFormatting,
        Feature::ContentControls,
        Feature::XmlDataBinding,
        Feature::SizeBoldItalic,
    ] {
        assert!(has(&found, feature), "{} missing", feature.name());
    }
    // Deleted text is not document text.
    assert_eq!(script(&found, Script::Latin), 4);
}

#[test]
fn the_same_document_is_always_tagged_the_same() {
    let docx = Docx::new(SHAPE_WITH_FALLBACK).part(
        "word/styles.xml",
        "styles",
        &format!(r#"{STYLES}<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#),
    );
    let bytes = docx.bytes();
    let first = scan(&bytes, &Limits::DEFAULT).unwrap();
    for _ in 0..3 {
        assert_eq!(scan(&bytes, &Limits::DEFAULT).unwrap(), first);
    }
}

#[test]
fn comments_count_only_where_there_is_a_comment() {
    let ns = crate::support::NAMESPACES;
    // The people part lists the authors of comments or of tracked changes; on its own, it holds no comment.
    let people = Docx::new(&paragraph("x")).part_from(
        "word/document.xml",
        "word/people.xml",
        "http://schemas.microsoft.com/office/2011/relationships/people",
        &format!(
            r#"<w15:people xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" {ns}/>"#
        ),
    );
    assert!(!has(&tag(&people), Feature::Comments));
    let comments = |content: &str| {
        tag(&Docx::new(&paragraph("x")).part(
            "word/comments.xml",
            "comments",
            &format!(r"<w:comments {ns}>{content}</w:comments>"),
        ))
    };
    assert!(!has(&comments(""), Feature::Comments));
    assert!(has(
        &comments(&format!(
            r#"<w:comment w:id="0" w:author="A">{}</w:comment>"#,
            paragraph("Note")
        )),
        Feature::Comments
    ));
    assert!(has(
        &tag_body(r#"<w:p><w:r><w:commentReference w:id="0"/></w:r></w:p>"#),
        Feature::Comments
    ));
}

#[test]
fn wordart_needs_a_text_warp() {
    let shape = |warp: &str| {
        format!(
            concat!(
                r#"<w:p><w:r><w:drawing><wp:inline><a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">"#,
                r#"<wps:wsp><wps:bodyPr><a:prstTxWarp prst="{}"><a:avLst/></a:prstTxWarp></wps:bodyPr></wps:wsp>"#,
                r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
            ),
            warp
        )
    };
    // Word writes "no transform" into the text of ordinary shapes.
    let plain = tag_body(&shape("textNoShape"));
    assert!(has(&plain, Feature::Shapes));
    assert!(!has(&plain, Feature::WordArt));
    assert!(has(&tag_body(&shape("textArchUp")), Feature::WordArt));
}

#[test]
fn properties_count_only_where_they_switch_something_on() {
    let run =
        |properties: &str| format!("<w:p><w:r><w:rPr>{properties}</w:rPr><w:t>x</w:t></w:r></w:p>");
    let paragraph_with =
        |properties: &str| format!("<w:p><w:pPr>{properties}</w:pPr><w:r><w:t>x</w:t></w:r></w:p>");
    let section = |properties: &str| format!("<w:p/><w:sectPr>{properties}</w:sectPr>");
    let table = |table: &str, cell: &str| {
        format!(
            "<w:tbl><w:tblPr>{table}</w:tblPr><w:tr><w:tc><w:tcPr>{cell}</w:tcPr><w:p/></w:tc></w:tr></w:tbl>"
        )
    };
    let cases: Vec<(String, Feature, bool)> = vec![
        (
            run(r#"<w:caps w:val="0"/><w:strike w:val="false"/><w:vanish w:val="off"/>"#),
            Feature::StrikeCapsHidden,
            false,
        ),
        (run("<w:caps/>"), Feature::StrikeCapsHidden, true),
        (
            run(r#"<w:b w:val="0"/><w:i w:val="0"/>"#),
            Feature::SizeBoldItalic,
            false,
        ),
        (run("<w:b/>"), Feature::SizeBoldItalic, true),
        (
            run(r#"<w:outline w:val="0"/><w:emboss w:val="0"/>"#),
            Feature::TextEffects,
            false,
        ),
        (run("<w:emboss/>"), Feature::TextEffects, true),
        (run(r#"<w:u w:val="none"/>"#), Feature::Underline, false),
        (run(r#"<w:u w:val="single"/>"#), Feature::Underline, true),
        (
            run(r#"<w:em w:val="none"/>"#),
            Feature::EmphasisMarks,
            false,
        ),
        (run(r#"<w:em w:val="dot"/>"#), Feature::EmphasisMarks, true),
        (
            run(r#"<w:vertAlign w:val="baseline"/><w:position w:val="0"/>"#),
            Feature::VerticalPosition,
            false,
        ),
        (
            run(r#"<w:vertAlign w:val="superscript"/>"#),
            Feature::VerticalPosition,
            true,
        ),
        (
            run(r#"<w:position w:val="6"/>"#),
            Feature::VerticalPosition,
            true,
        ),
        (
            run(r#"<w:rtl w:val="0"/><w:cs w:val="0"/>"#),
            Feature::RightToLeftRuns,
            false,
        ),
        (run("<w:rtl/>"), Feature::RightToLeftRuns, true),
        (run(r#"<w:color w:val="auto"/>"#), Feature::Color, false),
        (run(r#"<w:color w:val="FF0000"/>"#), Feature::Color, true),
        (
            run(
                r#"<w:highlight w:val="none"/><w:shd w:val="clear" w:color="auto" w:fill="auto"/>"#,
            ),
            Feature::HighlightAndShading,
            false,
        ),
        (
            run(r#"<w:highlight w:val="yellow"/>"#),
            Feature::HighlightAndShading,
            true,
        ),
        (
            run(r#"<w:shd w:val="clear" w:color="auto" w:fill="FFFF00"/>"#),
            Feature::HighlightAndShading,
            true,
        ),
        (
            run(r#"<w:spacing w:val="0"/><w:w w:val="100"/><w:kern w:val="0"/>"#),
            Feature::CharacterSpacing,
            false,
        ),
        (
            run(r#"<w:spacing w:val="20"/>"#),
            Feature::CharacterSpacing,
            true,
        ),
        (
            paragraph_with(r#"<w:bidi w:val="0"/>"#),
            Feature::BidirectionalParagraphs,
            false,
        ),
        (
            paragraph_with("<w:bidi/>"),
            Feature::BidirectionalParagraphs,
            true,
        ),
        (
            paragraph_with(r#"<w:jc w:val="left"/>"#),
            Feature::Alignment,
            false,
        ),
        (
            paragraph_with(r#"<w:jc w:val="center"/>"#),
            Feature::Alignment,
            true,
        ),
        (
            paragraph_with(r#"<w:keepNext w:val="0"/><w:widowControl w:val="0"/>"#),
            Feature::KeepAndWidowControl,
            false,
        ),
        (
            paragraph_with("<w:widowControl/>"),
            Feature::KeepAndWidowControl,
            true,
        ),
        (
            paragraph_with(r#"<w:outlineLvl w:val="9"/>"#),
            Feature::OutlineLevel,
            false,
        ),
        (
            paragraph_with(r#"<w:outlineLvl w:val="1"/>"#),
            Feature::OutlineLevel,
            true,
        ),
        // East Asian line breaking is on unless switched off.
        (
            paragraph_with("<w:kinsoku/><w:autoSpaceDE/>"),
            Feature::EastAsianLineBreaking,
            false,
        ),
        (
            paragraph_with(r#"<w:autoSpaceDE w:val="0"/>"#),
            Feature::AsianAutoSpacing,
            true,
        ),
        (
            paragraph_with(r#"<w:textDirection w:val="lrTb"/>"#),
            Feature::TextAlignmentAndDirection,
            false,
        ),
        (
            section(
                r#"<w:textDirection w:val="lrTb"/><w:bidi w:val="0"/><w:rtlGutter w:val="0"/>"#,
            ),
            Feature::SectionTextDirection,
            false,
        ),
        (
            section(r#"<w:textDirection w:val="tb"/>"#),
            Feature::SectionTextDirection,
            false,
        ),
        (
            section(r#"<w:textDirection w:val="tbRl"/>"#),
            Feature::SectionTextDirection,
            true,
        ),
        (
            section("<w:rtlGutter/>"),
            Feature::SectionTextDirection,
            true,
        ),
        (section("<w:bidi/>"), Feature::SectionTextDirection, true),
        (
            section(r#"<w:vAlign w:val="top"/>"#),
            Feature::PageVerticalAlignment,
            false,
        ),
        (
            section(r#"<w:vAlign w:val="center"/>"#),
            Feature::PageVerticalAlignment,
            true,
        ),
        (
            table(r#"<w:bidiVisual w:val="0"/>"#, ""),
            Feature::RightToLeftTables,
            false,
        ),
        (
            table("<w:bidiVisual/>", ""),
            Feature::RightToLeftTables,
            true,
        ),
        (
            table(
                "",
                r#"<w:vAlign w:val="top"/><w:textDirection w:val="lrTb"/>"#,
            ),
            Feature::CellTextDirection,
            false,
        ),
        (
            table("", r#"<w:vAlign w:val="bottom"/>"#),
            Feature::CellTextDirection,
            true,
        ),
    ];
    for (body, feature, expected) in cases {
        assert_eq!(
            has(&tag_body(&body), feature),
            expected,
            "{}: {body}",
            feature.name()
        );
    }
    // Vertical text is any direction but horizontal.
    let vertical = tag_body(&section(r#"<w:textDirection w:val="tbRl"/>"#));
    assert!(has(&vertical, Feature::VerticalText));
    assert!(!has(
        &tag_body(&section(r#"<w:textDirection w:val="lrTb"/>"#)),
        Feature::VerticalText
    ));
}

#[test]
fn equation_fields_are_told_apart_by_their_form() {
    let field = |instruction: &str| {
        tag_body(&format!(
            r#"<w:p><w:fldSimple w:instr="{}"><w:r><w:t>x</w:t></w:r></w:fldSimple></w:p>"#,
            instruction.replace('"', "&quot;")
        ))
    };
    // A formula built with EQ is a field, and nothing more the coverage matrix names.
    let formula = field(r" EQ \f(1,2) ");
    assert!(has(&formula, Feature::Fields));
    assert!(!has(&formula, Feature::CombineCharacters));
    assert!(!has(&formula, Feature::Ruby));
    // Word's Enclose Characters, and older versions' Phonetic Guide.
    assert!(has(&field(r" eq \o\ac(○,字) "), Feature::CombineCharacters));
    let ruby = field(r#" EQ \* jc2 \* "Font:MS Mincho" \* hps10 \o\ad(\s\up 9(かん),漢) "#);
    assert!(has(&ruby, Feature::Ruby));
    assert!(!has(&ruby, Feature::CombineCharacters));
}

#[test]
fn heading_numbering_needs_a_heading_style() {
    let ns = crate::support::NAMESPACES;
    let numbering = |style: &str| {
        format!(
            r#"<w:numbering {ns}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:pStyle w:val="{style}"/></w:lvl></w:abstractNum></w:numbering>"#
        )
    };
    let styles = format!(
        concat!(
            r#"{}<w:style w:type="paragraph" w:styleId="ListBullet"><w:name w:val="List Bullet"/></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/></w:style>"#,
            r#"<w:style w:type="paragraph" w:styleId="Chapter"><w:name w:val="Chapter"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style></w:styles>"#
        ),
        STYLES
    );
    let linked = |style: &str, with_styles: bool| {
        let mut docx =
            Docx::new(&paragraph("x")).part("word/numbering.xml", "numbering", &numbering(style));
        if with_styles {
            docx = docx.part("word/styles.xml", "styles", &styles);
        }
        has(&tag(&docx), Feature::HeadingNumbering)
    };
    assert!(!linked("ListBullet", true));
    assert!(linked("Heading1", true));
    assert!(linked("Chapter", true));
    // A level linked to a style that the document does not define numbers nothing.
    assert!(!linked("Heading1", false));
}

#[test]
fn sections_inherit_only_from_earlier_sections() {
    let header = format!(
        r#"<w:hdr {}>{}</w:hdr>"#,
        crate::support::NAMESPACES,
        paragraph("Header")
    );
    // The first section has no header, the second has one: nothing is inherited.
    let body = concat!(
        r#"<w:p><w:pPr><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr></w:pPr></w:p>"#,
        r#"<w:p/><w:sectPr><w:headerReference w:type="default" r:id="rId2"/></w:sectPr>"#,
    );
    let found = tag(&Docx::new(body).part("word/header1.xml", "header", &header));
    assert!(has(&found, Feature::SectionBreaks));
    assert!(!has(&found, Feature::LinkToPrevious));
}

#[test]
fn smaller_rules_of_the_second_review() {
    // A note numbering format is not a list's.
    let notes = tag_body(
        r#"<w:p/><w:sectPr><w:footnotePr><w:numFmt w:val="lowerRoman"/></w:footnotePr></w:sectPr>"#,
    );
    assert!(!has(&notes, Feature::NumberFormats));
    // A symbol's font is a font the document uses.
    let symbol = tag_body(r#"<w:p><w:r><w:sym w:font="Wingdings" w:char="F0E8"/></w:r></w:p>"#);
    assert!(symbol.fonts.contains("Wingdings"));
    // Strict documents name an embedded object's program on `w:objectEmbed`.
    let equation = tag_body(
        r#"<w:p><w:r><w:object><w:objectEmbed w:progId="Equation.3" r:id="rId9"/></w:object></w:r></w:p>"#,
    );
    assert!(has(&equation, Feature::OleObjects));
    assert!(has(&equation, Feature::LegacyEquations));
    // A table in a text box in a table cell is not a table nested in a table.
    let boxed = tag_body(concat!(
        r#"<w:tbl><w:tr><w:tc><w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent>"#,
        r#"<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
        r#"</w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p></w:tc></w:tr></w:tbl>"#,
    ));
    assert!(has(&boxed, Feature::TextBoxes));
    assert!(!has(&boxed, Feature::NestedTables));
}

#[test]
fn language_tags_are_recorded_only_in_their_simple_shape() {
    let styles = |language: &str| {
        format!(
            r#"{STYLES}<w:docDefaults><w:rPrDefault><w:rPr><w:lang w:val="{language}"/></w:rPr></w:rPrDefault></w:docDefaults></w:styles>"#
        )
    };
    let languages = |language: &str| {
        let found =
            tag(&Docx::new(&paragraph("x")).part("word/styles.xml", "styles", &styles(language)));
        (
            found.languages.into_iter().collect::<Vec<_>>(),
            found.notes.into_iter().collect::<Vec<_>>(),
        )
    };
    for kept in ["en-US", "sr-Latn-RS", "es-419", "x-none"] {
        assert_eq!(languages(kept).0, [kept]);
    }
    // Variants, extensions and private-use parts can carry any short text a document chooses.
    for refused in [
        "en-US-x-twain",
        "de-DE-1996",
        "en-u-ca-gregory",
        "qaa-x-johnsmit",
    ] {
        assert_eq!(
            languages(refused),
            (
                Vec::<String>::new(),
                vec![
                    "a language tag with variants, extensions or private-use parts was not recorded"
                ]
            ),
            "{refused}"
        );
    }
}
