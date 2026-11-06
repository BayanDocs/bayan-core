//! Synthetic layouts for the tests: every line's glyphs one per character, 6 pt apart, 12 pt in size, from a given x on a given baseline, so that every position a test expects can be worked out by hand.

use bayan_lab_layout::layout::{
    Direction, Font, Glyph, Layout, Line, LineCoverage, Page, Points, SchemaVersion, Source,
    TextOffsets,
};

/// The advance of every glyph, in thousandths of a point.
pub const ADVANCE: i64 = 6_000;

/// The size of every glyph.
pub const SIZE: i64 = 12_000;

/// The left margin where lines start unless a test says otherwise.
pub const MARGIN: i64 = 72_000;

/// A line of a synthetic page: its text, its baseline and where its first glyph starts, in thousandths of a point.
#[derive(Clone, Copy, Debug)]
pub struct Spec<'a> {
    pub text: &'a str,
    pub baseline: i64,
    pub x: i64,
}

/// A line at the margin on `baseline` points.
pub const fn at(text: &str, baseline: i64) -> Spec<'_> {
    Spec {
        text,
        baseline: baseline * 1000,
        x: MARGIN,
    }
}

pub fn points(millis: i64) -> Points {
    Points::from_millis(millis).unwrap()
}

/// A line whose glyphs are its characters in order, from left to right.
pub fn line(spec: Spec<'_>) -> Line {
    let glyphs = spec
        .text
        .chars()
        .enumerate()
        .map(|(index, _)| {
            glyph(
                index,
                spec.x + ADVANCE * i64::try_from(index).unwrap(),
                spec.baseline,
            )
        })
        .collect();
    Line {
        start: None,
        end: None,
        text: Some(spec.text.to_owned()),
        direction: Some(Direction::Ltr),
        baseline: Some(points(spec.baseline)),
        baseline_blu: None,
        glyphs: Some(glyphs),
    }
}

/// A glyph showing character `offset` of its line, at (`x`, `y`).
pub fn glyph(offset: usize, x: i64, y: i64) -> Glyph {
    Glyph {
        offset: u32::try_from(offset).unwrap(),
        length: 1,
        x: points(x),
        y: points(y),
        advance: points(ADVANCE),
        font: 0,
        size: points(SIZE),
        gid: None,
        x_blu: None,
        y_blu: None,
        advance_blu: None,
        size_blu: None,
    }
}

fn page(lines: Vec<Line>) -> Page {
    Page {
        width: points(612_000),
        height: points(792_000),
        width_blu: None,
        height_blu: None,
        start: None,
        lines,
        elements: None,
        skipped_glyphs: None,
    }
}

/// A layout as the PDF extractor writes it (Word's side): offsets through its own text, elements and skipped glyphs on every page. Checked against the layout JSON's rules.
pub fn pdf_layout(pages: Vec<Vec<Line>>) -> Layout {
    let mut layout = Layout {
        schema: SchemaVersion::V1,
        source: Source::Pdf,
        producer: "test".to_owned(),
        text_offsets: TextOffsets::Lines,
        line_coverage: LineCoverage::All,
        page_count: u32::try_from(pages.len()).unwrap(),
        fonts: vec![Font {
            name: "Test".to_owned(),
            family: None,
        }],
        pages: pages.into_iter().map(page).collect(),
    };
    let mut offset = 0_u64;
    for page in &mut layout.pages {
        page.start = Some(offset);
        page.elements = Some(Vec::new());
        page.skipped_glyphs = Some(0);
        for line in &mut page.lines {
            let characters = u64::try_from(line.text.as_ref().unwrap().chars().count()).unwrap();
            line.start = Some(offset);
            line.end = Some(offset + characters);
            offset += characters + 1;
        }
    }
    layout.check().unwrap();
    layout
}

/// A layout as the engine will write it (BayanDocs' side): no offsets. Checked against the rules.
pub fn engine_layout(pages: Vec<Vec<Line>>) -> Layout {
    let layout = Layout {
        schema: SchemaVersion::V1,
        source: Source::Engine,
        producer: "test".to_owned(),
        text_offsets: TextOffsets::Document,
        line_coverage: LineCoverage::All,
        page_count: u32::try_from(pages.len()).unwrap(),
        fonts: vec![Font {
            name: "Test".to_owned(),
            family: None,
        }],
        pages: pages.into_iter().map(page).collect(),
    };
    layout.check().unwrap();
    layout
}

/// The lines of `specs`.
pub fn lines(specs: &[Spec<'_>]) -> Vec<Line> {
    specs.iter().copied().map(line).collect()
}

/// A layout with no pages: what a document without BayanDocs' layout is compared with.
pub fn empty_layout() -> Layout {
    engine_layout(Vec::new())
}
