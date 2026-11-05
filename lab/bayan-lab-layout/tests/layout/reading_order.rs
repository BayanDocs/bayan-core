//! AC-2: right-to-left and mixed-direction lines are grouped and ordered correctly.
//!
//! PDFs draw right-to-left text as it appears, left to right or in any other order; the extractor must group the glyphs by baseline and give each line's text in reading order. The test font's CIDs 1 to 27 are the Hebrew letters and final forms from א to ת, 30 to 39 the digits, 40 the space and 41 the full stop; 60 to 63 are Arabic, 61 a lam-alef ligature (`support::TEST_CID_TO_UNICODE`).

use bayan_lab_layout::layout::Direction;

use crate::support::{cids, extract, glyph_texts, lines, one_page, test_cid_width};

/// A content stream that draws CIDs with the two-byte font at 10 points from `x` on the baseline `y` (PDF space), left to right as they appear.
fn draw(cids_shown: &[u16], x: f64, y: f64) -> String {
    format!("BT /F2 10 Tf {x} {y} Td {} Tj ET\n", cids(cids_shown))
}

/// A content stream that draws the same glyphs at the same places, but one by one from the right: the drawing order of a producer that writes right-to-left text in reading order.
fn draw_from_the_right(cids_shown: &[u16], x: f64, y: f64) -> String {
    let mut positions = Vec::new();
    let mut pen = x;
    for &cid in cids_shown {
        positions.push((cid, pen));
        pen += test_cid_width(cid) / 100.0;
    }
    let mut content = String::from("BT /F2 10 Tf\n");
    for (cid, position) in positions.into_iter().rev() {
        content.push_str(&format!("1 0 0 1 {position} {y} Tm {} Tj\n", cids(&[cid])));
    }
    content.push_str("ET\n");
    content
}

/// Hebrew letters by their position in the alphabet: 1 is א.
fn hebrew(letters: &[u16]) -> String {
    letters
        .iter()
        .map(|&letter| char::from_u32(0x5CF + u32::from(letter)).unwrap())
        .collect()
}

#[test]
fn a_hebrew_line_reads_right_to_left_whatever_the_drawing_order() {
    // שלום עולם: ש ל ו ם, space, ע ו ל ם; shin is CID 26, lamed 13, vav 6, final mem 14, ayin 19.
    let reading = [26_u16, 13, 6, 14, 40, 19, 6, 13, 14];
    let shown: Vec<u16> = reading.iter().rev().copied().collect();
    let expected = format!("{} {}", hebrew(&[26, 13, 6, 14]), hebrew(&[19, 6, 13, 14]));
    for content in [
        draw(&shown, 100.0, 700.0),
        draw_from_the_right(&shown, 100.0, 700.0),
    ] {
        let layout = extract(&one_page(&content)).layout;
        let [line] = lines(&layout) else {
            panic!("one line")
        };
        assert_eq!(line.text.as_deref(), Some(expected.as_str()));
        assert_eq!(line.direction, Some(Direction::Rtl));
        // The glyphs stay in display order: the leftmost shows the last letter.
        let glyphs = line.glyphs.as_deref().unwrap();
        assert_eq!(glyphs[0].x.to_points(), 100.0);
        assert_eq!(glyph_texts(line)[0], hebrew(&[14]));
        assert_eq!(glyphs[0].offset, 8);
    }
}

#[test]
fn numbers_and_latin_words_keep_their_order_inside_right_to_left_lines() {
    // בשנת 2026 נכתב: displayed as בתכנ 2026 תנשב, with the year's digits left to right.
    let shown = [
        2_u16,
        27,
        12,
        17,
        40,
        30 + 2,
        30,
        30 + 2,
        30 + 6,
        40,
        27,
        17,
        26,
        2,
    ];
    let content = draw(&shown, 100.0, 700.0);
    // On the same baseline, the Latin word "PDF" drawn with the one-byte font, left of the Hebrew.
    let content = format!("{content}BT /F1 10 Tf 50 700 Td (PDF ) Tj ET\n");
    let layout = extract(&one_page(&content)).layout;
    let [line] = lines(&layout) else {
        panic!("one line: {:?}", lines(&layout))
    };
    let expected = format!(
        "{} 2026 {} PDF",
        hebrew(&[2, 26, 17, 27]),
        hebrew(&[17, 12, 27, 2])
    );
    // Reading order: the rightmost word first; the digits and PDF in their own order. The space drawn after PDF sits between it and the Hebrew.
    assert_eq!(line.text.as_deref(), Some(expected.as_str()));
    assert_eq!(line.direction, Some(Direction::Rtl));
}

#[test]
fn a_hebrew_word_inside_an_english_line_reads_right_to_left() {
    // "the word שלום here": the Hebrew word is drawn as it appears, ם ו ל ש.
    let content = format!(
        "BT /F1 10 Tf 100 700 Td (the word ) Tj ET\n{}BT /F1 10 Tf 164 700 Td ( here) Tj ET\n",
        draw(&[14, 6, 13, 26], 140.0, 700.0)
    );
    let layout = extract(&one_page(&content)).layout;
    let [line] = lines(&layout) else {
        panic!("one line")
    };
    assert_eq!(
        line.text.as_deref(),
        Some(format!("the word {} here", hebrew(&[26, 13, 6, 14])).as_str())
    );
    assert_eq!(line.direction, Some(Direction::Ltr));
}

#[test]
fn arabic_ligatures_keep_their_letters_in_reading_order() {
    // سلام drawn as it appears: م, the lam-alef ligature (one glyph, two letters), س, then an Arabic comma on the left of it all.
    let layout = extract(&one_page(&draw(&[63, 60, 61, 62], 100.0, 700.0))).layout;
    let [line] = lines(&layout) else {
        panic!("one line")
    };
    assert_eq!(
        line.text.as_deref(),
        Some("\u{633}\u{644}\u{627}\u{645}\u{60C}")
    );
    assert_eq!(line.direction, Some(Direction::Rtl));
    assert_eq!(
        glyph_texts(line),
        ["\u{60C}", "\u{645}", "\u{644}\u{627}", "\u{633}"]
    );
    let ligature = &line.glyphs.as_deref().unwrap()[2];
    assert_eq!((ligature.offset, ligature.length), (1, 2));
}

#[test]
fn right_to_left_paragraphs_are_grouped_into_lines_by_baseline() {
    // Two lines of Hebrew, the second 14 points lower, each drawn from the right, and a superscript digit raised on the first.
    let first = [1_u16, 2, 3];
    let second = [4_u16, 5];
    let content = format!(
        "{}{}BT /F2 6 Tf 4 Ts 90 700 Td {} Tj ET\n",
        draw_from_the_right(&first, 100.0, 700.0),
        draw_from_the_right(&second, 100.0, 686.0),
        cids(&[31])
    );
    let layout = extract(&one_page(&content)).layout;
    let texts: Vec<_> = lines(&layout)
        .iter()
        .map(|line| (line.text.clone().unwrap(), line.direction.unwrap()))
        .collect();
    // The superscript is drawn last, after the second line, so it starts a line of its own on the first line's baseline: lines follow the drawing order.
    assert_eq!(
        texts,
        [
            (hebrew(&[3, 2, 1]), Direction::Rtl),
            (hebrew(&[5, 4]), Direction::Rtl),
            ("1".to_owned(), Direction::Ltr),
        ]
    );
    assert_eq!(lines(&layout)[0].baseline.unwrap().to_points(), 100.0);
    assert_eq!(lines(&layout)[1].baseline.unwrap().to_points(), 114.0);
}

#[test]
fn a_line_whose_display_does_not_tell_its_reading_order() {
    // The left-to-right paragraph "רחוב 15 in" displays as "15 בוחר in", and so does the right-to-left text "in רחוב 15": the same glyphs at the same places. The PDF does not say which was written; the extractor reads the line by its majority of right-to-left letters, so it returns the second (`layout-json.md`, "Lines from a PDF", and its known differences). Only the document, or a tagged PDF's structure, could tell.
    let shown = [31_u16, 35, 40, 2, 6, 8, 25];
    let content = format!(
        "{}BT /F1 10 Tf 160 700 Td ( in) Tj ET\n",
        draw(&shown, 100.0, 700.0)
    );
    let layout = extract(&one_page(&content)).layout;
    let [line] = lines(&layout) else {
        panic!("one line")
    };
    // רחוב: resh, het, vav, bet.
    let street = hebrew(&[25, 8, 6, 2]);
    assert_eq!(
        line.text.as_deref(),
        Some(format!("in {street} 15").as_str())
    );
    assert_eq!(line.direction, Some(Direction::Rtl));
}
