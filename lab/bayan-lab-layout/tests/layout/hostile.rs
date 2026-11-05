//! Hostile and broken PDFs: every limit of `Limits` is reached by a test; malformed structures and unsupported features are refused with an error, never repaired or guessed; and no error message repeats the document's text or names.

use bayan_lab_layout::pdf::{self, Error, Limit, Limits};

use crate::support::{
    Builder, cids, extract, flate_stream, one_page, one_page_mapped, one_page_with,
    one_page_with_font, refuse, replace_once, startxref, stream, test_sans,
};

/// Extracts with `limits` and returns the error.
fn refuse_with(pdf: &[u8], limits: &Limits) -> Error {
    match pdf::extract(pdf, limits) {
        Ok(_) => panic!("the extractor accepted a PDF it should refuse"),
        Err(error) => error,
    }
}

/// A document whose page draws `content` with TestSans as `/F1`, with the page tree `depth` levels deep and `pages` pages.
fn pages_document(content: &str, pages_count: usize) -> Vec<u8> {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let font = builder.add(test_sans());
    let contents = builder.add(flate_stream("", content.as_bytes()));
    let mut kids = Vec::new();
    for _ in 0..pages_count {
        kids.push(builder.add(format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {contents} 0 R >>"
        )));
    }
    let kids: Vec<String> = kids.iter().map(|kid| format!("{kid} 0 R")).collect();
    builder.set(
        pages,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages_count} >>",
            kids.join(" ")
        ),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    builder.finish(catalog)
}

#[test]
fn the_defaults_are_the_documented_ones() {
    let limits = Limits::default();
    assert_eq!(limits, Limits::DEFAULT);
    assert_eq!(limits.max_input_bytes, 256 << 20);
    assert_eq!(limits.max_objects, 4_194_304);
    assert_eq!(limits.max_nesting, 64);
    assert_eq!(limits.max_object_values, 2_097_152);
    assert_eq!(limits.max_operand_values, 65_536);
    assert_eq!(limits.max_xref_sections, 64);
    assert_eq!(limits.max_stream_bytes, 64 << 20);
    assert_eq!(limits.max_read_bytes, 512 << 20);
    assert_eq!(limits.max_pages, 65_536);
    assert_eq!(limits.max_operators, 100_000_000);
    assert_eq!(limits.max_glyphs, 4_194_304);
    assert_eq!(limits.max_form_depth, 16);
    assert_eq!(limits.max_graphics_depth, 256);
    assert_eq!(limits.max_fonts, 4_096);
    assert_eq!(limits.max_font_entries, 2_097_152);
    assert_eq!(limits.max_path_segments, 1_048_576);
    assert_eq!(limits.max_elements, 1_048_576);
    assert_eq!(pdf::MAX_TOKEN_BYTES, 255);
    assert_eq!(pdf::MAX_GLYPH_CHARACTERS, 32);
    assert_eq!(pdf::MAX_FILTERS, 8);
}

#[test]
fn every_limit_stops_the_extraction() {
    let text = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let limited = |change: fn(&mut Limits)| {
        let mut limits = Limits::DEFAULT;
        change(&mut limits);
        limits
    };
    let size = text.len();
    let mut exact = Limits::DEFAULT;
    exact.max_input_bytes = size;
    assert!(
        pdf::extract(&text, &exact).is_ok(),
        "a file of exactly the limit is read"
    );
    exact.max_input_bytes = size - 1;
    assert_eq!(refuse_with(&text, &exact), Error::Limit(Limit::InputSize));
    // The one-page document has ten objects and their entries.
    assert_eq!(
        refuse_with(&text, &limited(|limits| limits.max_objects = 5)),
        Error::Limit(Limit::Objects)
    );
    assert_eq!(
        refuse_with(&text, &limited(|limits| limits.max_stream_bytes = 20)),
        Error::Limit(Limit::StreamSize)
    );
    assert_eq!(
        refuse_with(&text, &limited(|limits| limits.max_read_bytes = 30)),
        Error::Limit(Limit::ReadTotal)
    );
    assert_eq!(
        refuse_with(
            &pages_document("", 3),
            &limited(|limits| limits.max_pages = 2)
        ),
        Error::Limit(Limit::Pages)
    );
    assert_eq!(
        refuse_with(&text, &limited(|limits| limits.max_operators = 4)),
        Error::Limit(Limit::Operators)
    );
    assert_eq!(
        refuse_with(&text, &limited(|limits| limits.max_glyphs = 4)),
        Error::Limit(Limit::Glyphs)
    );
    let two_fonts = one_page(&format!(
        "BT /F1 10 Tf (a) Tj /F2 10 Tf {} Tj ET",
        cids(&[1])
    ));
    assert_eq!(
        refuse_with(&two_fonts, &limited(|limits| limits.max_fonts = 1)),
        Error::Limit(Limit::Fonts)
    );
    // TestSans has 95 widths.
    assert_eq!(
        refuse_with(&text, &limited(|limits| limits.max_font_entries = 94)),
        Error::Limit(Limit::FontEntries)
    );
    let path = one_page("0 0 m 1 1 l 2 2 l 3 3 l 4 4 l 5 5 l S");
    assert_eq!(
        refuse_with(&path, &limited(|limits| limits.max_path_segments = 4)),
        Error::Limit(Limit::PathSegments)
    );
    let rules = one_page("0 0 m 100 0 l S 0 10 m 100 10 l S 0 20 m 100 20 l S");
    assert_eq!(
        refuse_with(&rules, &limited(|limits| limits.max_elements = 2)),
        Error::Limit(Limit::Elements)
    );
}

#[test]
fn nesting_is_limited_in_objects_content_and_the_page_tree() {
    let deep = format!("{}{} pop", "[".repeat(65), "]".repeat(65));
    assert_eq!(refuse(&one_page(&deep)), Error::Limit(Limit::Nesting));
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let mut parent = builder.reserve();
    builder.set(catalog, format!("<< /Type /Catalog /Pages {parent} 0 R >>"));
    for _ in 0..70 {
        let child = builder.reserve();
        builder.set(
            parent,
            format!("<< /Type /Pages /Kids [{child} 0 R] /Count 1 >>"),
        );
        parent = child;
    }
    builder.set(parent, "<< /Type /Page /MediaBox [0 0 600 800] >>");
    assert_eq!(
        refuse(&builder.finish(catalog)),
        Error::Limit(Limit::Nesting)
    );
}

#[test]
fn graphics_states_and_form_xobjects_cannot_nest_without_end() {
    assert_eq!(
        refuse(&one_page(&"q ".repeat(257))),
        Error::Limit(Limit::GraphicsDepth)
    );
    assert!(
        pdf::extract(
            &one_page(&format!("{}{}", "q ".repeat(256), "Q ".repeat(256))),
            &Limits::DEFAULT
        )
        .is_ok()
    );
    assert_eq!(
        refuse(&one_page(&"/Tag BMC ".repeat(257))),
        Error::Limit(Limit::GraphicsDepth)
    );
    // A form that draws itself.
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let form = builder.reserve();
    builder.set(form, stream(&format!("/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /XObject << /Me {form} 0 R >> >>"), b"/Me Do"));
    let contents = builder.add(stream("", b"/Me Do"));
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Me {form} 0 R >> >> /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    assert_eq!(
        refuse(&builder.finish(catalog)),
        Error::Limit(Limit::FormDepth)
    );
}

/// `pdf` with an incremental update appended that changes nothing but adds a section whose `Prev` is the previous one, or `prev` if given.
fn with_update(pdf: &[u8], prev: Option<usize>) -> Vec<u8> {
    let previous = startxref(pdf);
    let mut out = pdf.to_vec();
    let xref = out.len();
    out.extend_from_slice(
        format!(
            "xref\n0 0\ntrailer\n<< /Size 11 /Root 1 0 R /Prev {} >>\nstartxref\n{xref}\n%%EOF\n",
            prev.unwrap_or(previous)
        )
        .as_bytes(),
    );
    out
}

#[test]
fn cross_reference_chains_are_limited_and_may_not_loop() {
    let mut pdf = one_page("BT /F1 10 Tf 100 700 Td (Hi) Tj ET");
    for _ in 0..3 {
        pdf = with_update(&pdf, None);
    }
    assert!(pdf::extract(&pdf, &Limits::DEFAULT).is_ok());
    let limits = Limits {
        max_xref_sections: 3,
        ..Limits::DEFAULT
    };
    assert_eq!(
        refuse_with(&pdf, &limits),
        Error::Limit(Limit::XrefSections)
    );
    // A section whose Prev points to itself.
    let looped = {
        let base = one_page("");
        let xref = base.len();
        let mut out = base;
        out.extend_from_slice(format!("xref\n0 0\ntrailer\n<< /Size 11 /Root 1 0 R /Prev {xref} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes());
        out
    };
    assert_eq!(
        refuse(&looped),
        Error::Malformed("the cross-reference sections form a loop".to_owned())
    );
}

#[test]
fn a_decompression_bomb_stops_at_the_stream_limit() {
    // 2 MiB of zeros compress to about 2 KiB.
    let bomb = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0_u8; 2 << 20], 9);
    assert!(bomb.len() < 4096);
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let contents = builder.add(stream("/Filter /FlateDecode", &bomb));
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let limits = Limits {
        max_stream_bytes: 1 << 20,
        ..Limits::DEFAULT
    };
    assert_eq!(
        refuse_with(&builder.finish(catalog), &limits),
        Error::Limit(Limit::StreamSize)
    );
}

#[test]
fn malformed_structures_are_refused() {
    let content = "BT /F1 10 Tf 100 700 Td (Hi) Tj ET";
    let good = one_page(content);
    // Changes of the file that keep every offset where it was.
    let same_length = |from: &str, to: &str| -> Vec<u8> {
        assert_eq!(from.len(), to.len(), "{from}");
        replace_once(&good, from.as_bytes(), to.as_bytes())
    };
    // Changes of one object, with the cross-reference table written for the changed file.
    let object = |from: &'static str, to: &'static str| {
        one_page_mapped(content, "", "", move |body| body.replacen(from, to, 1))
    };
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "not a PDF",
            [b"%!PS-Adobe-3.0\n".as_slice(), &good[9..]].concat(),
            "does not start with a PDF header",
        ),
        (
            "a cut-off file",
            good[..good.len() - 20].to_vec(),
            "does not end with `startxref`",
        ),
        (
            "an entry that points at another object",
            same_length("2 0 obj", "3 0 obj"),
            "is not where its cross-reference entry points",
        ),
        (
            "an object without endobj",
            same_length("endobj\n2 0 obj", "endobX\n2 0 obj"),
            "has no `endobj`",
        ),
        (
            "a trailer without a catalog",
            same_length("/Root 1 0 R", "           "),
            "no reference to the catalog",
        ),
        (
            "a dictionary with a key twice",
            object("/Type /Pages", "/Type /Pages /Type /Pages"),
            "the same key twice",
        ),
        (
            "a page tree node of no known type",
            object("/Type /Pages", "/Type /Nodes"),
            "neither of type `Pages` nor of type `Page`",
        ),
        (
            "a catalog without a type",
            object("/Type /Catalog", ""),
            "of type `Catalog`",
        ),
        (
            "a page without a media box",
            object("/MediaBox [0 0 600 800]", ""),
            "has no media box",
        ),
        (
            "a crop box outside the media box",
            object(
                "/MediaBox [0 0 600 800]",
                "/MediaBox [0 0 600 800] /CropBox [700 900 800 1000]",
            ),
            "crop box lies outside its media box",
        ),
        (
            "a rotation that is not a multiple of 90",
            object(
                "/MediaBox [0 0 600 800]",
                "/MediaBox [0 0 600 800] /Rotate 45",
            ),
            "not a multiple of 90",
        ),
        (
            "a font of no known type",
            object("/Subtype /TrueType", "/Subtype /OpenType"),
            "unknown `Subtype`",
        ),
        (
            "widths that do not match their codes",
            object("/LastChar 126", "/LastChar 125"),
            "do not match its `FirstChar` and `LastChar`",
        ),
    ];
    for (what, pdf, message) in cases {
        let error = pdf::extract(&pdf, &Limits::DEFAULT).expect_err(what);
        assert!(
            matches!(&error, Error::Malformed(text) if text.contains(message)),
            "{what}: {error}"
        );
    }
    // A stream whose Length does not end at endstream.
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let contents = builder.add(b"<< /Length 3 >>\nstream\nBT ET\nendstream");
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    assert!(
        matches!(refuse(&builder.finish(catalog)), Error::Malformed(message) if message.contains("does not end at `endstream`"))
    );
}

#[test]
fn malformed_content_is_refused() {
    let cases = [
        (
            "text outside a text object",
            "/F1 10 Tf (Hi) Tj",
            "` outside a text object",
        ),
        (
            "a text object inside another",
            "BT BT ET ET",
            "`BT` inside a text object",
        ),
        ("ET without BT", "ET", "`ET` outside a text object"),
        (
            "Q without q",
            "Q",
            "`Q` restores a graphics state that was not saved",
        ),
        ("EMC without BMC", "EMC", "`EMC` without `BMC` or `BDC`"),
        ("EX without BX", "EX", "`EX` without `BX`"),
        (
            "an operator PDF does not define",
            "BT /F1 10 Tf (Hi) Tj ET xyz",
            "an operator that PDF 2.0 does not define",
        ),
        (
            "operands of the wrong kind",
            "BT /F1 (ten) Tf ET",
            "`Tf` has the wrong operands",
        ),
        (
            "too few operands",
            "BT 1 Td ET",
            "`Td` has the wrong operands",
        ),
        (
            "operands that no operator takes",
            "BT ET 1 2 3",
            "operands that no operator takes",
        ),
        (
            "text before a font",
            "BT (Hi) Tj ET",
            "before `Tf` chooses a font",
        ),
        (
            "a font the resources do not name",
            "BT /F9 10 Tf ET",
            "a font that the resources do not name",
        ),
        (
            "an odd string for a two-byte font",
            "BT /F2 10 Tf <000100> Tj ET",
            "an odd number of bytes",
        ),
        (
            "a Type 3 glyph operator on a page",
            "500 0 d0",
            "(`d0` or `d1`)",
        ),
        (
            "a line without a current point",
            "10 10 l S",
            "`l` without a current point",
        ),
        (
            "an inline image cut short",
            "BI /W 4 /H 4 /BPC 8 /CS /G ID \x01\x02 EI",
            "shorter than its size",
        ),
        (
            "an inline image without EI",
            "BI /W 1 /H 1 /BPC 8 /CS /G ID \x01 XX",
            "does not end with `EI`",
        ),
    ];
    for (what, content, expected) in cases {
        let error = pdf::extract(&one_page(content), &Limits::DEFAULT).expect_err(what);
        assert!(
            matches!(&error, Error::Malformed(message) if message.starts_with("page 1: ") && message.contains(expected)),
            "{what}: {error}"
        );
    }
    // Inside BX … EX, unknown operators are ignored.
    let layout = extract(&one_page(
        "BX 1 2 xyz EX BT /F1 10 Tf 100 700 Td (Hi) Tj ET",
    ))
    .layout;
    assert_eq!(layout.pages[0].lines[0].text.as_deref(), Some("Hi"));
}

#[test]
fn unsupported_features_are_refused_with_what_they_are() {
    let font = |font: &str| one_page_with_font("BT /F9 10 Tf 100 700 Td (Hi) Tj ET", font);
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "encryption",
            replace_once(
                &one_page(""),
                b"/Root 1 0 R",
                b"/Root 1 0 R /Encrypt << /Filter /Standard /V 1 /R 2 >>",
            ),
            "encrypted files",
        ),
        (
            "LZW compression",
            {
                let mut builder = Builder::new();
                let catalog = builder.reserve();
                let pages = builder.reserve();
                let contents = builder.add(stream(
                    "/Filter /LZWDecode",
                    b"\x80\x0b\x60\x50\x22\x0c\x0c\x85\x01",
                ));
                let page = builder.add(format!("<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Contents {contents} 0 R >>"));
                builder.set(
                    pages,
                    format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
                );
                builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
                builder.finish(catalog)
            },
            "LZWDecode",
        ),
        (
            "optional content",
            one_page_mapped("", "", "", |body| {
                body.replacen(
                    "/Type /Catalog",
                    "/Type /Catalog /OCProperties << /OCGs [] /D << >> >>",
                    1,
                )
            }),
            "optional content",
        ),
        (
            "vertical writing",
            font(
                "<< /Type /Font /Subtype /Type0 /BaseFont /V /Encoding /Identity-V /DescendantFonts [] >>",
            ),
            "vertical writing",
        ),
        (
            "another CMap",
            font(
                "<< /Type /Font /Subtype /Type0 /BaseFont /J /Encoding /90ms-RKSJ-H /DescendantFonts [] >>",
            ),
            "other than `Identity-H`",
        ),
        (
            "a standard font without widths",
            font("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
            "without `Widths`",
        ),
        (
            "MacExpertEncoding",
            font(
                "<< /Type /Font /Subtype /Type1 /BaseFont /X /FirstChar 32 /LastChar 32 /Widths [250] /Encoding /MacExpertEncoding >>",
            ),
            "MacExpertEncoding",
        ),
        (
            "a skewed Type 3 font",
            font(
                "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [0.001 0.001 0 0.001 0 0] /CharProcs << >> /FirstChar 72 /LastChar 72 /Widths [500] >>",
            ),
            "FontMatrix",
        ),
        (
            "a user unit",
            one_page_with("", "", "/UserUnit 2"),
            "UserUnit",
        ),
        (
            "an inline image whose end only its decoder knows",
            one_page("BI /W 1 /H 1 /BPC 8 /CS /G /F /DCT ID \x01 EI"),
            "does not mark its end",
        ),
    ];
    for (what, pdf, message) in cases {
        match pdf::extract(&pdf, &Limits::DEFAULT) {
            Err(Error::Unsupported(text)) => assert!(text.contains(message), "{what}: {text}"),
            other => panic!("{what}: {other:?}"),
        }
    }
}

#[test]
fn messages_never_repeat_the_document() {
    // Distinctive text where a document puts its own words: an operator, a font name, a resource name, a filter name.
    let secret = "Zanzibar";
    let cases = [
        one_page(&format!("BT /F1 10 Tf 100 700 Td (x) Tj ET {secret}")),
        one_page(&format!("BT /{secret} 10 Tf ET")),
        one_page_with_font(
            "BT /F9 10 Tf ET",
            &format!("<< /Type /Font /Subtype /Type1 /BaseFont /{secret} >>"),
        ),
        one_page_with_font(
            "BT /F9 10 Tf ET",
            &format!("<< /Type /Font /Subtype /{secret} >>"),
        ),
        {
            let mut builder = Builder::new();
            let catalog = builder.reserve();
            let pages = builder.reserve();
            let contents = builder.add(stream(&format!("/Filter /{secret}"), b"x"));
            let page = builder.add(format!("<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Contents {contents} 0 R >>"));
            builder.set(
                pages,
                format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
            );
            builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
            builder.finish(catalog)
        },
    ];
    let expected = [
        "an operator that PDF 2.0 does not define",
        "a font that the resources do not name",
        "without `Widths`",
        "unknown `Subtype`",
        "unknown filter",
    ];
    for (pdf, expected) in cases.iter().zip(expected) {
        let error = refuse(pdf).to_string();
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains(secret), "{error}");
    }
}

#[test]
fn garbage_never_panics() {
    // Every prefix of a valid file, and the file with every byte changed in turn, gives a layout or an error.
    let pdf = one_page(&format!(
        "BT /F1 10 Tf 100 700 Td (Hi) Tj /F2 12 Tf {} Tj ET 0 0 m 10 0 l S",
        cids(&[1, 2, 61])
    ));
    for end in (0..pdf.len()).step_by(7) {
        let _ = pdf::extract(&pdf[..end], &Limits::DEFAULT);
    }
    for index in (0..pdf.len()).step_by(3) {
        let mut changed = pdf.clone();
        changed[index] = changed[index].wrapping_add(0x55);
        let _ = pdf::extract(&changed, &Limits::DEFAULT);
    }
}
