//! The limits on memory and work that do not show in a file's size: the values parsed from the file's objects and those waiting for one operator, the bytes read at every use, the number of filters of a stream, the characters of glyphs and the segments of paths. Each limit is reached with small limits, as in `hostile.rs`.

use bayan_lab_layout::pdf::{self, Error, Limit, Limits};

use crate::support::{
    Builder, extract, one_page, one_page_with, one_page_with_font, refuse, stream,
};

/// The default limits with one changed.
fn limited(change: impl FnOnce(&mut Limits)) -> Limits {
    let mut limits = Limits::DEFAULT;
    change(&mut limits);
    limits
}

/// Extracts with `limits` and returns the error.
fn refuse_with(pdf: &[u8], limits: &Limits) -> Error {
    match pdf::extract(pdf, limits) {
        Ok(_) => panic!("the extractor accepted a PDF it should refuse"),
        Err(error) => error,
    }
}

/// A one-page document whose page has the content stream `contents` (an object body) and draws the form XObject `/Fm`, whose body is `form`.
fn page_with_contents(contents: &[u8], form: &[u8]) -> Vec<u8> {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let font = builder.add(crate::support::test_sans());
    let form = builder.add(form);
    let contents = builder.add(contents);
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 {font} 0 R >> /XObject << /Fm {form} 0 R >> >> /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    builder.finish(catalog)
}

/// A form XObject of unfiltered `data`.
fn form(data: &[u8]) -> Vec<u8> {
    stream("/Type /XObject /Subtype /Form /BBox [0 0 10 10]", data)
}

#[test]
fn the_values_waiting_for_an_operator_are_limited() {
    // A hundred operands before an operator.
    let operands = one_page(&format!("{}n", "/a ".repeat(100)));
    assert_eq!(
        refuse_with(&operands, &limited(|limits| limits.max_operand_values = 99)),
        Error::Limit(Limit::OperandValues)
    );
    // The items of an array among the operands count too: the array and its hundred strings.
    let array = format!("BT /F1 10 Tf 100 700 Td [{}] TJ ET", "(a) ".repeat(100));
    assert_eq!(
        refuse_with(
            &one_page(&array),
            &limited(|limits| limits.max_operand_values = 100)
        ),
        Error::Limit(Limit::OperandValues)
    );
    let fits = pdf::extract(
        &one_page(&array),
        &limited(|limits| limits.max_operand_values = 101),
    )
    .unwrap();
    assert_eq!(
        fits.layout.pages[0].lines[0].text.as_deref(),
        Some(&*"a".repeat(100))
    );
    // The count starts again at every operator.
    let many = one_page(&"1 w ".repeat(1000));
    assert!(pdf::extract(&many, &limited(|limits| limits.max_operand_values = 1)).is_ok());
    // So do the values of an inline image's dictionary: four of them here.
    let inline = one_page("BI /W 1 /H 1 /BPC 8 /CS /G ID \u{0} EI");
    assert_eq!(
        refuse_with(&inline, &limited(|limits| limits.max_operand_values = 3)),
        Error::Limit(Limit::OperandValues)
    );
    assert!(pdf::extract(&inline, &limited(|limits| limits.max_operand_values = 4)).is_ok());
}

#[test]
fn the_values_of_the_files_objects_are_limited_in_total() {
    // The plain one-page document parses fewer than 400 values; a page entry of an array of 1,000 more passes the same limit.
    let plain = one_page("");
    let wide = one_page_with("", "", &format!("/X [{}]", "0 ".repeat(1000)));
    let limits = limited(|limits| limits.max_object_values = 400);
    assert!(pdf::extract(&plain, &limits).is_ok());
    assert_eq!(
        refuse_with(&wide, &limits),
        Error::Limit(Limit::ObjectValues)
    );
    // Values at any depth count: here, dictionaries inside an array.
    let deep = one_page_with("", "", &format!("/X [{}]", "<< /A [0 0] >> ".repeat(200)));
    assert_eq!(
        refuse_with(&deep, &limits),
        Error::Limit(Limit::ObjectValues)
    );
    assert!(extract(&wide).layout.pages.len() == 1);
}

#[test]
fn every_use_of_a_stream_counts_its_bytes() {
    // A form of 100 unfiltered bytes: drawn once it fits a budget of 1,000 bytes, drawn twenty times it does not, although it is decoded only once.
    let spaces = form(&[b' '; 100]);
    let budget = limited(|limits| limits.max_read_bytes = 1000);
    let once = page_with_contents(&stream("", b"/Fm Do"), &spaces);
    assert!(pdf::extract(&once, &budget).is_ok());
    let twenty = page_with_contents(&stream("", "/Fm Do ".repeat(20).as_bytes()), &spaces);
    assert_eq!(
        refuse_with(&twenty, &budget),
        Error::Limit(Limit::ReadTotal)
    );
    assert!(pdf::extract(&twenty, &limited(|limits| limits.max_read_bytes = 3000)).is_ok());
    // An unfiltered page content counts as well as a filtered one.
    let content = format!("BT /F1 10 Tf 100 700 Td (Hello) Tj ET{}", " ".repeat(1000));
    let plain = page_with_contents(&stream("", content.as_bytes()), &spaces);
    assert_eq!(refuse_with(&plain, &budget), Error::Limit(Limit::ReadTotal));
}

#[test]
fn a_stream_names_at_most_max_filters_filters() {
    let hex = |data: &[u8]| -> Vec<u8> {
        data.iter()
            .flat_map(|byte| format!("{byte:02x}").into_bytes())
            .chain(*b">")
            .collect()
    };
    let content = b"BT /F1 10 Tf 100 700 Td (Hi) Tj ET".to_vec();
    let chained = |count: usize| {
        let mut data = content.clone();
        for _ in 0..count {
            data = hex(&data);
        }
        let filters = vec!["/ASCIIHexDecode"; count].join(" ");
        page_with_contents(&stream(&format!("/Filter [{filters}]"), &data), &form(b""))
    };
    let most = extract(&chained(pdf::MAX_FILTERS));
    assert_eq!(most.layout.pages[0].lines[0].text.as_deref(), Some("Hi"));
    assert_eq!(
        refuse(&chained(pdf::MAX_FILTERS + 1)),
        Error::Malformed(format!(
            "page 1: a stream has more than {} filters",
            pdf::MAX_FILTERS
        ))
    );
}

#[test]
fn a_differences_array_may_not_count_past_code_255() {
    let font = |differences: &str| {
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 97 /LastChar 97 /Widths [500] /Encoding << /Differences [{differences}] >> >>"
        )
    };
    let fits = one_page_with_font("BT /F9 10 Tf 100 700 Td (a) Tj ET", &font("254 /a /b"));
    assert!(pdf::extract(&fits, &Limits::DEFAULT).is_ok());
    let past = one_page_with_font("BT /F9 10 Tf 100 700 Td (a) Tj ET", &font("255 /a /b"));
    assert_eq!(
        refuse(&past),
        Error::Malformed("page 1: an encoding's `Differences` runs past code 255".to_owned())
    );
}

#[test]
fn a_glyph_counts_once_for_each_of_its_characters() {
    // The code 97 shows the ligature of `f_f_i`, three characters: two of them count six.
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Test /FirstChar 97 /LastChar 97 /Widths [500] /Encoding << /Differences [97 /f_f_i] >> >>";
    let ligatures = one_page_with_font("BT /F9 10 Tf 100 700 Td (aa) Tj ET", font);
    let glyphs = |count: u64| limited(|limits| limits.max_glyphs = count);
    assert_eq!(
        refuse_with(&ligatures, &glyphs(5)),
        Error::Limit(Limit::Glyphs)
    );
    let fits = pdf::extract(&ligatures, &glyphs(6)).unwrap();
    assert_eq!(
        fits.layout.pages[0].lines[0].text.as_deref(),
        Some("ffiffi")
    );
}

#[test]
fn a_path_counts_its_subpaths_and_the_paths_held_around_forms() {
    let segments = |count: usize| limited(|limits| limits.max_path_segments = count);
    // Moves alone never grow the path: an empty subpath is reused.
    let moves = one_page(&format!("{}n", "0 0 m ".repeat(1000)));
    assert!(pdf::extract(&moves, &segments(1)).is_ok());
    // Three subpaths of one segment each: six in all.
    let three = one_page("0 0 m 1 1 l 0 0 m 1 1 l 0 0 m 1 1 l S");
    assert_eq!(
        refuse_with(&three, &segments(5)),
        Error::Limit(Limit::PathSegments)
    );
    assert!(pdf::extract(&three, &segments(6)).is_ok());
    // A form drawn while a path is being built: the page's three and the form's three are in memory at once.
    let nested = page_with_contents(
        &stream("", b"0 0 m 1 1 l 2 2 l /Fm Do 3 3 l S"),
        &form(b"0 0 m 1 1 l 2 2 l S"),
    );
    assert_eq!(
        refuse_with(&nested, &segments(5)),
        Error::Limit(Limit::PathSegments)
    );
    assert!(pdf::extract(&nested, &segments(6)).is_ok());
}
