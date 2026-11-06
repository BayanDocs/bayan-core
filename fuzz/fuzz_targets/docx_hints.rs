//! bayan-lab-compare's reader of `w:lastRenderedPageBreak` hints (LAB-004, M7) on arbitrary bytes, and the alignment that M7 makes of the text it reads. An input that starts with `PK` is a whole `.docx` package (bayan-opc opens it, bayan-xml reads its main document part with Markup Compatibility); any other is the main document part itself, so that mutations reach the reader's walk through WordprocessingML directly rather than through compressed data. For every input it reads: every hint lies within the text and in order; reading it again gives the same hints; and M7 against a fixed two-page layout and against no layout counts every hint once, never pairing more than there are.

#![no_main]

use bayan_lab_compare::align;
use bayan_lab_compare::hints::{Hints, Limits, read, read_document_part};
use bayan_lab_compare::metrics::{self, hint_agreement};
use bayan_lab_layout::layout::{
    Direction, Font, Layout, Line, LineCoverage, Page, Points, SchemaVersion, Source, TextOffsets,
};
use libfuzzer_sys::fuzz_target;

/// The default limits, lowered so that the text of an input of 64 KiB, which bayan-opc lets decompress a hundredfold, cannot keep the alignment busy for seconds: libFuzzer's timeout then catches work that grows faster than its input rather than work the limits allow.
const LIMITS: Limits = Limits {
    max_chars: 1 << 20,
    max_hints: 1 << 16,
    ..Limits::DEFAULT
};

const COMPARE: metrics::Limits = metrics::Limits {
    align: align::Limits {
        max_work: 50_000_000,
    },
    max_chars: 1 << 20,
};

fn read_input(data: &[u8]) -> Option<Hints> {
    if data.starts_with(b"PK") {
        read(data, &LIMITS).ok()
    } else {
        read_document_part(data, &LIMITS).ok()
    }
}

/// A layout of BayanDocs' side with two pages of a few words, which the text of the seeds partly shares.
fn two_pages() -> Layout {
    let line = |text: &str| Line {
        start: None,
        end: None,
        text: Some(text.to_owned()),
        direction: Some(Direction::Ltr),
        baseline: Points::from_millis(100_000),
        baseline_blu: None,
        glyphs: None,
    };
    let page = |lines: Vec<Line>| Page {
        width: Points::from_millis(612_000).unwrap_or(Points::ZERO),
        height: Points::from_millis(792_000).unwrap_or(Points::ZERO),
        width_blu: None,
        height_blu: None,
        start: None,
        lines,
        elements: None,
        skipped_glyphs: None,
    };
    Layout {
        schema: SchemaVersion::V1,
        source: Source::Engine,
        producer: "fuzz".to_owned(),
        text_offsets: TextOffsets::Document,
        line_coverage: LineCoverage::All,
        page_count: 2,
        fonts: vec![Font {
            name: "Fuzz".to_owned(),
            family: None,
        }],
        pages: vec![
            page(vec![line("Text with a page break"), line("Lorem ipsum")]),
            page(vec![line("Second page"), line("Header")]),
        ],
    }
}

fuzz_target!(|data: &[u8]| {
    let Some(hints) = read_input(data) else {
        return;
    };
    let mut previous = 0;
    for &position in &hints.breaks {
        assert!(previous <= position && position <= hints.chars.len());
        previous = position;
    }
    assert_eq!(read_input(data), Some(hints.clone()));
    let empty = Layout {
        page_count: 0,
        pages: Vec::new(),
        ..two_pages()
    };
    for candidate in [two_pages(), empty] {
        let agreement = hint_agreement(&hints, &candidate, &COMPARE).unwrap();
        let breaks = &agreement.hints;
        assert_eq!(breaks.total, u64::try_from(hints.breaks.len()).unwrap());
        assert!(breaks.agreed <= breaks.total.min(u64::from(candidate.page_count)));
        let counted: u64 = breaks.agreed_breaks.iter().map(|range| range[1] - range[0]).sum();
        assert_eq!(counted, breaks.agreed);
        assert!(breaks.agreed_breaks.iter().all(|range| range[0] < range[1] && range[1] <= breaks.total));
    }
});
