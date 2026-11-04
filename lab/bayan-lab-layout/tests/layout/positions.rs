//! AC-1: extracted glyph origins match known positions in synthetic PDFs within 0.01 pt.
//!
//! Each test draws a few glyphs with one feature of PDF's text model and compares every glyph's origin, advance and size with values worked out by hand from ISO 32000-2 §9.4.4: the origin is (0, rise) through the text matrix, the transformation matrix and the page (whose y runs down from the top of the 800-point page), and after each glyph the text matrix moves by ((w0 × Tfs) + Tc + Tw) × Th, where w0 is the glyph's width in thousandths of an em (`test_sans_width`, `test_cid_width`).

use crate::support::{
    Builder, assert_glyph, cids, extract, flate_stream, glyph_texts, lines, one_page,
    one_page_with, test_sans,
};

#[test]
fn text_origins_follow_td_and_the_widths() {
    let layout = extract(&one_page("BT /F1 10 Tf 100 700 Td (Him) Tj ET")).layout;
    let [line] = lines(&layout) else {
        panic!("one line")
    };
    assert_eq!(line.text.as_deref(), Some("Him"));
    let glyphs = line.glyphs.as_deref().unwrap();
    // H is 500 wide, i 250, m 750, at 10 points.
    assert_glyph(&glyphs[0], 100.0, 100.0, 5.0, 10.0);
    assert_glyph(&glyphs[1], 105.0, 100.0, 2.5, 10.0);
    assert_glyph(&glyphs[2], 107.5, 100.0, 7.5, 10.0);
    assert_eq!(layout.fonts[0].name, "TestSans");
    assert_eq!(layout.fonts[0].family, None);
}

#[test]
fn character_and_word_spacing_and_horizontal_scaling_move_the_next_glyph() {
    let layout = extract(&one_page(
        "BT /F1 10 Tf 2 Tc 3 Tw 50 Tz 100 700 Td (a b) Tj ET",
    ))
    .layout;
    let glyphs = lines(&layout)[0].glyphs.as_deref().unwrap();
    // a: (5 + 2) × 0.5 = 3.5; the space: (2.5 + 2 + 3) × 0.5 = 3.75; b: 3.5. Scaling does not change the size.
    assert_glyph(&glyphs[0], 100.0, 100.0, 3.5, 10.0);
    assert_glyph(&glyphs[1], 103.5, 100.0, 3.75, 10.0);
    assert_glyph(&glyphs[2], 107.25, 100.0, 3.5, 10.0);
}

#[test]
fn tj_adjustments_and_rise_move_glyphs() {
    let layout = extract(&one_page(
        "BT /F1 12 Tf 50 600 Td [(A) -250 (V) 120 (W)] TJ 4 Ts (x) Tj ET",
    ))
    .layout;
    let [line] = lines(&layout) else {
        panic!("one line")
    };
    assert_eq!(line.text.as_deref(), Some("AVWx"));
    let glyphs = line.glyphs.as_deref().unwrap();
    assert_glyph(&glyphs[0], 50.0, 200.0, 6.0, 12.0);
    // -250 moves the next glyph 250/1000 × 12 = 3 points right; 120 moves it 1.44 points left.
    assert_glyph(&glyphs[1], 59.0, 200.0, 6.0, 12.0);
    assert_glyph(&glyphs[2], 63.56, 200.0, 10.8, 12.0);
    // A rise of 4 points raises the glyph, which stays on the line.
    assert_glyph(&glyphs[3], 74.36, 196.0, 6.0, 12.0);
    assert_eq!(line.baseline.unwrap().to_points(), 200.0);
}

#[test]
fn text_matrix_leading_and_the_line_operators_start_new_lines() {
    let layout = extract(&one_page("BT /F1 1 Tf 12 0 0 12 72 720 Tm 1.2 TL (ab) Tj T* (cd) Tj 0 -2 TD (ef) Tj (gh) ' 1 0.5 (ij) \" ET")).layout;
    let texts: Vec<_> = lines(&layout)
        .iter()
        .map(|line| line.text.clone().unwrap())
        .collect();
    assert_eq!(texts, ["ab", "cd", "ef", "gh", "ij"]);
    // The text matrix scales a 1-point font to 12 points; T* moves down by the leading in text space (1.2 × 12), TD sets the leading to 2 (24 points).
    let expected = [
        (80.0, 6.0, 6.0),
        (94.4, 6.0, 6.0),
        (118.4, 6.0, 6.0),
        (142.4, 6.0, 6.0),
        (166.4, 9.0, 12.0),
    ];
    for (line, (y, first_advance, second_advance)) in lines(&layout).iter().zip(expected) {
        let glyphs = line.glyphs.as_deref().unwrap();
        assert_glyph(&glyphs[0], 72.0, y, first_advance, 12.0);
        assert_glyph(&glyphs[1], 72.0 + first_advance, y, second_advance, 12.0);
    }
}

#[test]
fn the_transformation_matrix_and_saved_states_apply_to_text() {
    let layout = extract(&one_page(
        "q 2 0 0 2 10 20 cm BT /F1 10 Tf 30 300 Td (A) Tj ET Q BT /F1 10 Tf 30 300 Td (B) Tj ET",
    ))
    .layout;
    let [scaled, plain] = lines(&layout) else {
        panic!("two lines")
    };
    // (30, 300) × 2 + (10, 20) = (70, 620), 180 from the top; twice the size.
    assert_glyph(
        &scaled.glyphs.as_deref().unwrap()[0],
        70.0,
        180.0,
        10.0,
        20.0,
    );
    assert_glyph(&plain.glyphs.as_deref().unwrap()[0], 30.0, 500.0, 5.0, 10.0);
}

#[test]
fn two_byte_fonts_take_widths_from_w_and_ignore_word_spacing() {
    let layout = extract(&one_page(&format!(
        "BT /F2 10 Tf 5 Tw 100 400 Td {} Tj ET",
        cids(&[1, 40, 2])
    )))
    .layout;
    let [line] = lines(&layout) else {
        panic!("one line")
    };
    let glyphs = line.glyphs.as_deref().unwrap();
    // Word spacing applies only to the one-byte code 32, so not to CID 40.
    assert_glyph(&glyphs[0], 100.0, 400.0, 6.0, 10.0);
    assert_glyph(&glyphs[1], 106.0, 400.0, 3.0, 10.0);
    assert_glyph(&glyphs[2], 109.0, 400.0, 6.0, 10.0);
    assert_eq!(
        glyphs.iter().map(|glyph| glyph.gid).collect::<Vec<_>>(),
        [Some(1), Some(40), Some(2)]
    );
    assert_eq!(layout.fonts[0].name, "TestCID");
    assert_eq!(layout.fonts[0].family.as_deref(), Some("Test CID"));
}

#[test]
fn type3_fonts_scale_widths_by_their_font_matrix() {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let procedure = builder.add(flate_stream("", b"250 0 0 0 250 250 d1 0 0 250 250 re f"));
    let font = builder.add(format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 500 500] /FontMatrix [0.002 0 0 0.002 0 0] /CharProcs << /A {procedure} 0 R /B {procedure} 0 R >> /Encoding << /Type /Encoding /Differences [65 /A /B] >> /FirstChar 65 /LastChar 66 /Widths [250 500] >>"
    ));
    let contents = builder.add(flate_stream("", b"BT /F3 10 Tf 100 700 Td (AB) Tj ET"));
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F3 {font} 0 R >> >> /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let layout = extract(&builder.finish(catalog)).layout;
    let glyphs = lines(&layout)[0].glyphs.as_deref().unwrap();
    // 250 × 0.002 × 10 = 5, and 500 × 0.002 × 10 = 10.
    assert_glyph(&glyphs[0], 100.0, 100.0, 5.0, 10.0);
    assert_glyph(&glyphs[1], 105.0, 100.0, 10.0, 10.0);
    assert_eq!(lines(&layout)[0].text.as_deref(), Some("AB"));
    assert_eq!(layout.fonts[0].name, "Type3");
}

#[test]
fn form_xobjects_place_text_through_their_matrix() {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let sans = builder.add(test_sans());
    let form = builder.add(flate_stream(
        &format!("/Type /XObject /Subtype /Form /BBox [0 0 100 100] /Matrix [1 0 0 1 200 300] /Resources << /Font << /F1 {sans} 0 R >> >>"),
        b"BT /F1 10 Tf 0 0 Td (F) Tj ET",
    ));
    let contents = builder.add(flate_stream("", b"q 1 0 0 1 10 10 cm /Fm1 Do Q"));
    let page = builder.add(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /XObject << /Fm1 {form} 0 R >> >> /Contents {contents} 0 R >>"
    ));
    builder.set(
        pages,
        format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>"),
    );
    builder.set(catalog, format!("<< /Type /Catalog /Pages {pages} 0 R >>"));
    let layout = extract(&builder.finish(catalog)).layout;
    // (0, 0) through the form's matrix (200, 300) and the page's cm (10, 10): (210, 310), 490 from the top.
    assert_glyph(
        &lines(&layout)[0].glyphs.as_deref().unwrap()[0],
        210.0,
        490.0,
        5.0,
        10.0,
    );
}

#[test]
fn rotated_pages_and_crop_boxes_give_the_page_as_displayed() {
    // The crop box is 500 wide and 600 high; turned 90 degrees clockwise, the page is 600 wide and 500 high. Text that is upright on the displayed page runs up the unrotated page.
    let pdf = one_page_with(
        "BT /F1 10 Tf 0 1 -1 0 300 200 Tm (Ro) Tj ET",
        "",
        "/CropBox [50 100 550 700] /Rotate 90",
    );
    let layout = extract(&pdf).layout;
    let page = &layout.pages[0];
    assert_eq!(
        (page.width.to_points(), page.height.to_points()),
        (600.0, 500.0)
    );
    let glyphs = page.lines[0].glyphs.as_deref().unwrap();
    // The origin (300, 200) is 200 - 100 = 100 from the displayed left edge and 300 - 50 = 250 from its top.
    assert_glyph(&glyphs[0], 100.0, 250.0, 5.0, 10.0);
    assert_glyph(&glyphs[1], 105.0, 250.0, 5.0, 10.0);
    // On the unrotated page, the same text would be vertical, and is left off the lines.
    let layout = extract(&one_page("BT /F1 10 Tf 0 1 -1 0 300 200 Tm (Ro) Tj ET")).layout;
    assert!(lines(&layout).is_empty());
    assert_eq!(layout.pages[0].skipped_glyphs, Some(2));
}

#[test]
fn invisible_text_is_counted_and_left_off_the_lines() {
    let extraction = extract(&one_page(
        "BT /F1 10 Tf 100 700 Td 3 Tr (hidden) Tj 0 Tr (shown) Tj ET",
    ));
    assert_eq!(lines(&extraction.layout)[0].text.as_deref(), Some("shown"));
    // "shown" starts after the six invisible glyphs: h, i, d, d, e, n are 500, 250, 500, 500, 500, 500 thousandths.
    assert_glyph(
        &lines(&extraction.layout)[0].glyphs.as_deref().unwrap()[0],
        127.5,
        100.0,
        5.0,
        10.0,
    );
    assert_eq!(extraction.skipped_glyphs, 6);
    assert_eq!(extraction.layout.pages[0].skipped_glyphs, Some(6));
}

#[test]
fn glyph_texts_and_offsets_follow_the_text() {
    let layout = extract(&one_page(&format!(
        "BT /F2 10 Tf 100 400 Td {} Tj ET",
        cids(&[50, 51, 30])
    )))
    .layout;
    let line = &lines(&layout)[0];
    // CID 50 is a ligature of f and i (two characters), CID 51 shows none, CID 30 is the digit 0.
    assert_eq!(line.text.as_deref(), Some("fi0"));
    assert_eq!(glyph_texts(line), ["fi", "", "0"]);
    let glyphs = line.glyphs.as_deref().unwrap();
    assert_eq!((glyphs[0].offset, glyphs[0].length), (0, 2));
    assert_eq!((glyphs[1].offset, glyphs[1].length), (2, 0));
    assert_eq!((glyphs[2].offset, glyphs[2].length), (2, 1));
}

#[test]
fn extraction_is_deterministic() {
    // Two extractions of the same file give the same layout, byte for byte.
    let pdf = one_page("BT /F1 10 Tf 100 700 Td (Same) Tj ET");
    assert_eq!(
        extract(&pdf).layout.to_json().unwrap(),
        extract(&pdf).layout.to_json().unwrap()
    );
}
