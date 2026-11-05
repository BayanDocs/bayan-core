//! The file structures the reader supports: cross-reference tables and streams, object streams, incremental updates, hybrid files, inherited page attributes, nested page trees, content arrays, indirect lengths and filter chains.

use crate::support::{
    Builder, extract, find, flate_stream, lines, one_page, startxref, stream, test_sans,
};

/// The text of every line of every page.
fn texts(pdf: &[u8]) -> Vec<Vec<String>> {
    extract(pdf)
        .layout
        .pages
        .iter()
        .map(|page| {
            page.lines
                .iter()
                .map(|line| line.text.clone().unwrap())
                .collect()
        })
        .collect()
}

#[test]
fn reads_cross_reference_streams() {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let font = builder.add(test_sans());
    let contents = builder.add(flate_stream("", b"BT /F1 10 Tf 100 700 Td (Stream) Tj ET"));
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 {font} 0 R >> >> /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    assert_eq!(
        texts(&builder.finish_with_xref_stream(catalog)),
        [["Stream"]]
    );
}

/// A file whose catalog, page tree, page and font are members of an object stream, found through a cross-reference stream.
fn object_stream_file() -> Vec<u8> {
    let members = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
        test_sans(),
    ];
    let mut header = String::new();
    let mut body = String::new();
    for (index, member) in members.iter().enumerate() {
        header.push_str(&format!("{} {} ", index + 1, body.len()));
        body.push_str(member);
        body.push('\n');
    }
    let data = format!("{header}\n{body}");
    let first = header.len() + 1;
    let mut out = b"%PDF-1.7\n".to_vec();
    let content_offset = out.len();
    out.extend_from_slice(b"5 0 obj\n");
    out.extend_from_slice(&flate_stream("", b"BT /F1 10 Tf 100 700 Td (Packed) Tj ET"));
    out.extend_from_slice(b"\nendobj\n");
    let object_stream_offset = out.len();
    out.extend_from_slice(b"6 0 obj\n");
    out.extend_from_slice(&flate_stream(
        &format!("/Type /ObjStm /N 4 /First {first}"),
        data.as_bytes(),
    ));
    out.extend_from_slice(b"\nendobj\n");
    let xref_offset = out.len();
    let entry = |kind: u8, field: usize, index: u8| -> Vec<u8> {
        let field = u16::try_from(field).unwrap().to_be_bytes();
        vec![kind, field[0], field[1], index]
    };
    let mut rows = entry(0, 0, 255);
    for index in 0..4 {
        rows.extend(entry(2, 6, index));
    }
    rows.extend(entry(1, content_offset, 0));
    rows.extend(entry(1, object_stream_offset, 0));
    rows.extend(entry(1, xref_offset, 0));
    out.extend_from_slice(b"7 0 obj\n");
    out.extend_from_slice(&stream("/Type /XRef /Size 8 /W [1 2 1] /Root 1 0 R", &rows));
    out.extend_from_slice(format!("\nendobj\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes());
    out
}

#[test]
fn reads_objects_inside_object_streams() {
    assert_eq!(texts(&object_stream_file()), [["Packed"]]);
}

#[test]
fn an_incremental_update_replaces_the_objects_it_lists() {
    let mut pdf = one_page("BT /F1 10 Tf 100 700 Td (Original) Tj ET");
    let previous = startxref(&pdf);
    // Object 9 is the page (catalog 1, pages 2, TestSans 3, the two-byte font 4 to 7, contents 8); the update gives it new contents, object 10.
    let contents = pdf.len();
    pdf.extend_from_slice(b"10 0 obj\n");
    pdf.extend_from_slice(&flate_stream(
        "",
        b"BT /F1 10 Tf 100 700 Td (Updated) Tj ET",
    ));
    pdf.extend_from_slice(b"\nendobj\n");
    let page = pdf.len();
    pdf.extend_from_slice(b"9 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 3 0 R >> >> /Contents 10 0 R >>\nendobj\n");
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n9 2\n{page:010} 00000 n \n{contents:010} 00000 n \ntrailer\n<< /Size 11 /Root 1 0 R /Prev {previous} >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    assert_eq!(texts(&pdf), [["Updated"]]);
}

#[test]
fn a_hybrid_file_finds_object_stream_members_through_xrefstm() {
    // A file readable without cross-reference streams too: its table lists the content stream and the object stream; the page tree and font are in the object stream, which only the stream that `XRefStm` names lists.
    let original = object_stream_file();
    let content = find(&original, b"5 0 obj").unwrap();
    let object_stream = find(&original, b"6 0 obj").unwrap();
    let xref_stream = find(&original, b"7 0 obj").unwrap();
    let mut pdf = original[..find(&original, b"startxref").unwrap()].to_vec();
    let table = pdf.len();
    pdf.extend_from_slice(
        format!(
            "xref\n0 8\n0000000000 65535 f \n0000000000 00000 f \n0000000000 00000 f \n0000000000 00000 f \n0000000000 00000 f \n{content:010} 00000 n \n{object_stream:010} 00000 n \n{xref_stream:010} 00000 n \ntrailer\n<< /Size 8 /Root 1 0 R /XRefStm {xref_stream} >>\nstartxref\n{table}\n%%EOF\n"
        )
        .as_bytes(),
    );
    assert_eq!(texts(&pdf), [["Packed"]]);
}

#[test]
fn pages_inherit_resources_and_boxes_through_nested_page_trees() {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let root = builder.reserve();
    let middle = builder.reserve();
    let font = builder.add(test_sans());
    let first = builder.add(flate_stream("", b"BT /F1 10 Tf 100 700 Td (One) Tj ET"));
    let second = builder.add(flate_stream("", b"BT /F1 10 Tf 100 700 Td (Two) Tj ET"));
    let third = builder.add(flate_stream("", b"BT /F1 10 Tf 100 300 Td (Three) Tj ET"));
    let page_one = builder.add(format!(
        "<< /Type /Page /Parent {middle} 0 R /Contents {first} 0 R >>"
    ));
    let page_two = builder.add(format!(
        "<< /Type /Page /Parent {middle} 0 R /Contents {second} 0 R >>"
    ));
    let page_three = builder.add(format!(
        "<< /Type /Page /Parent {root} 0 R /MediaBox [0 0 400 500] /Contents {third} 0 R >>"
    ));
    builder.set(
        middle,
        format!(
            "<< /Type /Pages /Parent {root} 0 R /Kids [{page_one} 0 R {page_two} 0 R] /Count 2 >>"
        ),
    );
    builder.set(
        root,
        format!("<< /Type /Pages /Kids [{middle} 0 R {page_three} 0 R] /Count 3 /MediaBox [0 0 600 800] /Resources << /Font << /F1 {font} 0 R >> >> >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {root} 0 R >>"));
    let layout = extract(&builder.finish(catalog)).layout;
    assert_eq!(layout.page_count, 3);
    let sizes: Vec<_> = layout
        .pages
        .iter()
        .map(|page| (page.width.to_points(), page.height.to_points()))
        .collect();
    assert_eq!(sizes, [(600.0, 800.0), (600.0, 800.0), (400.0, 500.0)]);
    let texts: Vec<_> = layout
        .pages
        .iter()
        .map(|page| page.lines[0].text.clone().unwrap())
        .collect();
    assert_eq!(texts, ["One", "Two", "Three"]);
    // Text offsets run through the whole document: each line is followed by one line feed.
    let starts: Vec<_> = layout
        .pages
        .iter()
        .map(|page| (page.start, page.lines[0].start, page.lines[0].end))
        .collect();
    assert_eq!(
        starts,
        [
            (Some(0), Some(0), Some(3)),
            (Some(4), Some(4), Some(7)),
            (Some(8), Some(8), Some(13))
        ]
    );
    assert_eq!(
        layout.pages[2].lines[0].baseline.unwrap().to_points(),
        200.0
    );
}

#[test]
fn content_arrays_indirect_lengths_and_filter_chains_are_read() {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let font = builder.add(test_sans());
    // The first stream ends between two tokens of the text object that the second one finishes.
    let first = builder.add(stream("", b"BT /F1 10 Tf 100 700 Td"));
    let data = miniz_oxide::deflate::compress_to_vec_zlib(b"(Joined) Tj ET", 6);
    let hex: String = data
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<String>()
        + ">";
    let length = builder.add(format!("{}", hex.len()));
    let mut second_stream =
        format!("<< /Filter [/ASCIIHexDecode /FlateDecode] /Length {length} 0 R >>\nstream\n")
            .into_bytes();
    second_stream.extend_from_slice(hex.as_bytes());
    second_stream.extend_from_slice(b"\nendstream");
    let second = builder.add(second_stream);
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 {font} 0 R >> >> /Contents [{first} 0 R {second} 0 R] >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    assert_eq!(texts(&builder.finish(catalog)), [["Joined"]]);
}

#[test]
fn a_page_without_contents_is_an_empty_page() {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let layout = extract(&builder.finish(catalog)).layout;
    assert_eq!(layout.page_count, 1);
    assert!(lines(&layout).is_empty());
    assert_eq!(layout.pages[0].elements.as_deref(), Some(&[][..]));
}

#[test]
fn images_and_rules_become_elements() {
    let pdf = crate::support::one_page_with(
        "q 100 0 0 50 200 600 cm /Im1 Do Q 0.5 w 100 500 m 300 500 l S 100 400 200 2 re f 50 50 100 100 re f 1 0 0 RG 10 10 m 20 30 l S BI /W 2 /H 2 /BPC 8 /CS /G ID \x01\x02\x03\x04 EI",
        "/XObject << /Im1 << /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Length 1 >> >>",
        "",
    );
    // The image above is a direct dictionary, which the format does not allow for streams: the reader refuses it.
    assert!(matches!(
        crate::support::refuse(&pdf),
        bayan_lab_layout::pdf::Error::Malformed(_)
    ));
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let image = builder.add(stream("/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray", b"\x80"));
    let contents = builder.add(stream(
        "",
        b"q 100 0 0 50 200 600 cm /Im1 Do Q 0.5 w 100 500 m 300 500 l S 100 400 200 2 re f 50 50 100 100 re f 10 10 m 20 30 l S q 20 0 0 10 400 100 cm BI /W 2 /H 2 /BPC 8 /CS /G ID \x01\x02\x03\x04 EI Q",
    ));
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Im1 {image} 0 R >> >> /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let layout = extract(&builder.finish(catalog)).layout;
    let elements: Vec<_> = layout.pages[0]
        .elements
        .as_deref()
        .unwrap()
        .iter()
        .map(|element| {
            (
                element.kind,
                element.x0.to_points(),
                element.y0.to_points(),
                element.x1.to_points(),
                element.y1.to_points(),
            )
        })
        .collect();
    use bayan_lab_layout::layout::ElementKind::{Image, Rule};
    assert_eq!(
        elements,
        [
            // The image fills the unit square scaled to 100 by 50 at (200, 600): 150 to 200 from the top.
            (Image, 200.0, 150.0, 300.0, 200.0),
            // A stroked horizontal segment, half a point thick.
            (Rule, 100.0, 299.75, 300.0, 300.25),
            // A filled rectangle two points high is a rule; a 100-point square is not, and neither is a diagonal segment.
            (Rule, 100.0, 398.0, 300.0, 400.0),
            // The inline image, 20 by 10 at (400, 100).
            (Image, 400.0, 690.0, 420.0, 700.0),
        ]
    );
}
