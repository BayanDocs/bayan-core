//! LAB-004 AC-1: synthetic pairs whose differences are known give exactly the expected metric values. Word's side is always a layout as the PDF extractor writes it, BayanDocs' side one as the engine will.

use bayan_lab_compare::metrics::{
    Breaks, Comparison, GlyphError, Limits, PageCount, Place, Spread, TextAgreement, compare,
};
use bayan_lab_layout::layout::{Direction, Layout};

use crate::support::{
    ADVANCE, MARGIN, at, empty_layout, engine_layout, glyph, line, lines, pdf_layout, points,
};

fn compared(reference: &Layout, candidate: &Layout) -> Comparison {
    compare(reference, candidate, &Limits::DEFAULT)
        .unwrap()
        .comparison
}

fn breaks(total: u64, agreed: &[[u64; 2]], first_difference: Option<Place>) -> Breaks {
    Breaks {
        total,
        agreed: agreed.iter().map(|range| range[1] - range[0]).sum(),
        agreed_breaks: agreed.to_vec(),
        first_difference,
    }
}

const fn place(page: u32, line: Option<u32>) -> Place {
    Place { page, line }
}

fn spread(median: i64, p95: i64, max: i64) -> Option<Spread> {
    Some(Spread {
        median: points(median),
        p95: points(p95),
        max: points(max),
    })
}

fn zero() -> Option<Spread> {
    spread(0, 0, 0)
}

#[test]
fn identical_layouts_agree_everywhere() {
    let pages = || {
        vec![
            lines(&[at("Alpha beta gamma", 100), at("delta epsilon", 114)]),
            lines(&[at("zeta eta", 100)]),
        ]
    };
    let comparison = compared(&pdf_layout(pages()), &engine_layout(pages()));
    assert_eq!(
        comparison,
        Comparison {
            text: TextAgreement {
                reference: 33,
                candidate: 33,
                matched: 33,
                complete: true,
            },
            m1: PageCount {
                reference: 2,
                candidate: 2,
            },
            m2: breaks(2, &[[0, 2]], None),
            m3: breaks(3, &[[0, 3]], None),
            m4: GlyphError {
                lines: 3,
                glyphs: 33,
                x: zero(),
                y: zero(),
            },
        }
    );
}

#[test]
fn shifted_page_break() {
    // Word ends page 1 after "seven eight"; BayanDocs moves that line to the top of page 2.
    let reference = pdf_layout(vec![
        lines(&[
            at("One two three", 100),
            at("four five six", 114),
            at("seven eight", 128),
        ]),
        lines(&[at("nine ten", 100), at("eleven", 114)]),
    ]);
    let candidate = engine_layout(vec![
        lines(&[at("One two three", 100), at("four five six", 114)]),
        lines(&[
            at("seven eight", 100),
            at("nine ten", 114),
            at("eleven", 128),
        ]),
    ]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(
        comparison.m1,
        PageCount {
            reference: 2,
            candidate: 2
        }
    );
    assert!(comparison.m1.matches());
    // Page 2 starts at "nine" in Word and at "seven" in BayanDocs.
    assert_eq!(comparison.m2, breaks(2, &[[0, 1]], Some(place(2, None))));
    // Every line still starts at the same word.
    assert_eq!(comparison.m3, breaks(5, &[[0, 5]], None));
    // Only the lines of page 1 that stay there are measured: "seven eight" moved to another page, and page 2 starts elsewhere.
    assert_eq!(
        comparison.m4,
        GlyphError {
            lines: 2,
            glyphs: 11 + 11,
            x: zero(),
            y: zero(),
        }
    );
}

#[test]
fn changed_line_break() {
    // BayanDocs breaks the first line one word earlier.
    let reference = pdf_layout(vec![lines(&[
        at("The quick brown fox", 100),
        at("jumps over the lazy dog", 114),
        at("Next paragraph", 128),
    ])]);
    let candidate = engine_layout(vec![lines(&[
        at("The quick brown", 100),
        at("fox jumps over the lazy dog", 114),
        at("Next paragraph", 128),
    ])]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m2, breaks(1, &[[0, 1]], None));
    // Lines 1 and 3 start at the same words; line 2 starts at "jumps" in Word and at "fox" in BayanDocs.
    assert_eq!(
        comparison.m3,
        breaks(3, &[[0, 1], [2, 3]], Some(place(1, Some(2))))
    );
    // Line 1 starts alike but ends elsewhere, line 2 starts elsewhere: only line 3's glyphs are measured.
    assert_eq!(
        comparison.m4,
        GlyphError {
            lines: 1,
            glyphs: 13,
            x: zero(),
            y: zero(),
        }
    );
    assert_eq!(comparison.text.matched, comparison.text.reference);
}

#[test]
fn offset_glyphs() {
    let reference = pdf_layout(vec![lines(&[at("abcdefghij", 100), at("klmnopqrst", 114)])]);
    let mut candidate = engine_layout(vec![lines(&[at("abcdefghij", 100), at("klmnopqrst", 114)])]);
    // Four glyphs of line 1 half a point to the right; the last glyph of line 2 two points to the right and a quarter point down.
    let first = candidate.pages[0].lines[0].glyphs.as_mut().unwrap();
    for glyph in &mut first[..4] {
        glyph.x = points(glyph.x.millis() + 500);
    }
    let second = candidate.pages[0].lines[1].glyphs.as_mut().unwrap();
    second[9].x = points(second[9].x.millis() + 2000);
    second[9].y = points(second[9].y.millis() + 250);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m3, breaks(2, &[[0, 2]], None));
    // x: fifteen at 0, four at 0.5 pt, one at 2 pt. By nearest rank, the median is the 10th (0), the 95th percentile the 19th (0.5 pt).
    // y: nineteen at 0, one at 0.25 pt; the 19th is 0.
    assert_eq!(
        comparison.m4,
        GlyphError {
            lines: 2,
            glyphs: 20,
            x: spread(0, 500, 2000),
            y: spread(0, 0, 250),
        }
    );
}

#[test]
fn missing_text() {
    // BayanDocs lacks the line "Section two". The texts of its neighbours begin alike, so `diff` alone would put the deletion across the start of "Section three".
    let reference = pdf_layout(vec![lines(&[
        at("Section one", 100),
        at("Section two", 114),
        at("Section three", 128),
    ])]);
    let candidate = engine_layout(vec![lines(&[
        at("Section one", 100),
        at("Section three", 114),
    ])]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(
        comparison.text,
        TextAgreement {
            reference: 32,
            candidate: 22,
            matched: 22,
            complete: true,
        }
    );
    assert_eq!(
        comparison.m1,
        PageCount {
            reference: 1,
            candidate: 1
        }
    );
    assert_eq!(comparison.m2, breaks(1, &[[0, 1]], None));
    // The missing line does not agree; the line after it, which both have, does.
    assert_eq!(
        comparison.m3,
        breaks(3, &[[0, 1], [2, 3]], Some(place(1, Some(2))))
    );
    // "Section three" sits one line higher in BayanDocs: its 12 glyphs are 14 pt off vertically, the 10 of "Section one" not at all.
    assert_eq!(
        comparison.m4,
        GlyphError {
            lines: 2,
            glyphs: 22,
            x: zero(),
            y: spread(14_000, 14_000, 14_000),
        }
    );
}

#[test]
fn missing_first_line() {
    // BayanDocs lacks the first line, "Section one". The next line begins like it, so `diff` alone would keep "Section" at the start and put the deletion across the start of "Section two".
    let reference = pdf_layout(vec![lines(&[
        at("Section one", 100),
        at("Section two", 114),
        at("Section three", 128),
    ])]);
    let candidate = engine_layout(vec![lines(&[
        at("Section two", 100),
        at("Section three", 114),
    ])]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(
        comparison.text,
        TextAgreement {
            reference: 32,
            candidate: 22,
            matched: 22,
            complete: true,
        }
    );
    assert_eq!(comparison.m2, breaks(1, &[[0, 1]], None));
    // The missing first line does not agree; the two lines after it, which both have, do.
    assert_eq!(comparison.m3, breaks(3, &[[1, 3]], Some(place(1, Some(1)))));
    // Both sit one line higher in BayanDocs: the 10 glyphs of "Section two" and the 12 of "Section three" are 14 pt off vertically.
    let expected = GlyphError {
        lines: 2,
        glyphs: 22,
        x: zero(),
        y: spread(14_000, 14_000, 14_000),
    };
    assert_eq!(comparison.m4, expected);
    // A first line that begins otherwise gives the same result.
    let reference = pdf_layout(vec![lines(&[
        at("Alpha one", 100),
        at("Section two", 114),
        at("Section three", 128),
    ])]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m3, breaks(3, &[[1, 3]], Some(place(1, Some(1)))));
    assert_eq!(comparison.m4, expected);
}

#[test]
fn without_bayandocs_layout_nothing_agrees() {
    let reference = pdf_layout(vec![
        lines(&[at("Alpha", 100), at("Beta", 114)]),
        lines(&[at("Gamma", 100)]),
    ]);
    let comparison = compared(&reference, &empty_layout());
    assert_eq!(
        comparison,
        Comparison {
            text: TextAgreement {
                reference: 14,
                candidate: 0,
                matched: 0,
                complete: true,
            },
            m1: PageCount {
                reference: 2,
                candidate: 0,
            },
            m2: breaks(2, &[], Some(place(1, None))),
            m3: breaks(3, &[], Some(place(1, Some(1)))),
            m4: GlyphError::default(),
        }
    );
    assert_eq!(comparison.m1.difference(), -2);
}

#[test]
fn labels_and_field_results_that_differ_keep_the_breaks() {
    // A numbering label and a date field that BayanDocs writes differently.
    let reference = pdf_layout(vec![lines(&[
        at("1. First item", 100),
        at("Date: 5 November 2026", 114),
    ])]);
    let candidate = engine_layout(vec![lines(&[
        at("a) First item", 100),
        at("Date: 6 November 2026", 114),
    ])]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m3, breaks(2, &[[0, 2]], None));
    // Every glyph but the label's two and the date's digit.
    assert_eq!(comparison.m4.glyphs, 11 - 2 + 18 - 1);
    assert_eq!(comparison.m4.x, zero());
    assert_eq!(comparison.text.matched, comparison.text.reference - 3);
}

#[test]
fn lines_on_one_baseline_are_joined_on_both_sides() {
    // A table row: one line in Word's PDF, one line per cell in BayanDocs.
    let reference = pdf_layout(vec![vec![
        line(at("Alpha1 Beta1 Gamma1", 100)),
        line(at("After the table", 130)),
    ]]);
    // Each cell where Word draws its text: at characters 0, 7 and 13 of the row.
    let cell = |text, index: i64| crate::support::Spec {
        text,
        baseline: 100_000,
        x: MARGIN + index * ADVANCE,
    };
    let candidate = engine_layout(vec![vec![
        line(cell("Alpha1", 0)),
        line(cell("Beta1", 7)),
        line(cell("Gamma1", 13)),
        line(at("After the table", 130)),
    ]]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m3, breaks(2, &[[0, 2]], None));
    assert_eq!(comparison.m4.lines, 2);
    assert_eq!(comparison.m4.glyphs, 17 + 13);
    assert_eq!(comparison.m4.x, zero());
}

#[test]
fn a_line_read_in_another_order_is_compared_by_display_order() {
    // Displayed alike from left to right: "15 בוחר in". Word's extractor reads it right to left ("in רחוב 15"), BayanDocs' engine left to right ("רחוב 15 in").
    let x = |column: i64| MARGIN + column * ADVANCE;
    let rtl_offsets = [8, 9, 7, 6, 5, 4, 3, 2, 0, 1];
    let ltr_offsets = [5, 6, 4, 3, 2, 1, 0, 7, 8, 9];
    let make = |text: &str, direction, offsets: [usize; 10]| {
        let mut made = line(at(text, 100));
        made.direction = Some(direction);
        made.glyphs = Some(
            offsets
                .iter()
                .enumerate()
                .map(|(column, &offset)| glyph(offset, x(i64::try_from(column).unwrap()), 100_000))
                .collect(),
        );
        made
    };
    let reference = pdf_layout(vec![vec![make(
        "in \u{5E8}\u{5D7}\u{5D5}\u{5D1} 15",
        Direction::Rtl,
        rtl_offsets,
    )]]);
    let candidate = engine_layout(vec![vec![make(
        "\u{5E8}\u{5D7}\u{5D5}\u{5D1} 15 in",
        Direction::Ltr,
        ltr_offsets,
    )]]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m3, breaks(1, &[[0, 1]], None));
    // The texts differ in order, but the same glyphs stand in the same places: all eight that show characters are paired, with no error.
    assert_eq!(
        comparison.m4,
        GlyphError {
            lines: 1,
            glyphs: 8,
            x: zero(),
            y: zero(),
        }
    );
}

#[test]
fn a_ligature_against_separate_letters_is_not_measured() {
    // Word draws "fi" as one glyph; BayanDocs draws two.
    let mut reference_line = line(at("fine", 100));
    let glyphs = reference_line.glyphs.as_mut().unwrap();
    glyphs[0].length = 2;
    glyphs.remove(1);
    glyphs[1].x = points(MARGIN + 2 * ADVANCE);
    let reference = pdf_layout(vec![vec![reference_line]]);
    let candidate = engine_layout(vec![lines(&[at("fine", 100)])]);
    let comparison = compared(&reference, &candidate);
    assert_eq!(comparison.m3, breaks(1, &[[0, 1]], None));
    // "n" and "e" are paired; the ligature and the two letters are not.
    assert_eq!(comparison.m4.glyphs, 2);
}

#[test]
fn comparisons_are_limited() {
    let reference = pdf_layout(vec![lines(&[at("Alpha beta", 100)])]);
    let limits = Limits {
        max_chars: 5,
        ..Limits::DEFAULT
    };
    let error = compare(&reference, &reference.clone(), &limits).unwrap_err();
    assert_eq!(
        error.to_string(),
        "the reference's text has more than 5 characters in the comparable form"
    );
}
