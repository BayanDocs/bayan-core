//! PDFs of known tools: LibreOffice 24.2 and Chromium 153 (`tests/fixtures/README.md` says how they were made).
//!
//! Two checks for each: every word that poppler's `pdftotext -bbox`, an independent PDF reader, finds starts where the first of our glyphs of the word starts and ends where the advance of our last glyph ends, within 0.01 pt (AC-1 on real files); and the whole extraction is the committed sample in `lab/schema/samples/`, which the schema tests validate (AC-3). The files are compiled in, so these tests also run in WebAssembly and show that extraction gives the same bytes there.

use bayan_lab_layout::layout::{Direction, Layout};

use crate::support::{extract, glyph_texts};

const LIBREOFFICE: &[u8] = include_bytes!("../fixtures/libreoffice.pdf");
const LIBREOFFICE_WORDS: &str = include_str!("../fixtures/libreoffice.words");
const LIBREOFFICE_SAMPLE: &str = include_str!("../../../schema/samples/pdf-libreoffice.json");
const CHROMIUM: &[u8] = include_bytes!("../fixtures/chromium.pdf");
const CHROMIUM_WORDS: &str = include_str!("../fixtures/chromium.words");
const CHROMIUM_SAMPLE: &str = include_str!("../../../schema/samples/pdf-chromium.json");

/// Checks every word poppler found against our glyphs, and returns how many words there were.
fn check_words(layout: &Layout, words: &str) -> usize {
    let mut count = 0;
    for line in words.lines() {
        let mut fields = line.split('\t');
        let (Some(page), Some(x0), Some(x1), Some(word), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            panic!("a malformed line in the words file: {line}");
        };
        let page: usize = page.parse().unwrap();
        let (x0, x1): (f64, f64) = (x0.parse().unwrap(), x1.parse().unwrap());
        let reversed: String = word.chars().rev().collect();
        let mut found = false;
        'lines: for line in &layout.pages[page - 1].lines {
            let glyphs = line.glyphs.as_deref().unwrap();
            let texts = glyph_texts(line);
            for start in 0..glyphs.len() {
                if (glyphs[start].x.to_points() - x0).abs() > 0.01 {
                    continue;
                }
                let mut spelled = String::new();
                for end in start..glyphs.len() {
                    spelled.push_str(&texts[end]);
                    // poppler writes a right-to-left word's characters in display order.
                    if spelled == word || spelled == reversed {
                        let right = glyphs[end].x.to_points() + glyphs[end].advance.to_points();
                        assert!(
                            (right - x1).abs() <= 0.01,
                            "{word}: ends at {right}, poppler says {x1}"
                        );
                        found = true;
                        break 'lines;
                    }
                    if spelled.chars().count() > word.chars().count() {
                        break;
                    }
                }
            }
        }
        assert!(
            found,
            "no glyphs spell the word {word} from x = {x0} on page {page}"
        );
        count += 1;
    }
    count
}

/// The extraction as the committed samples write it: pretty-printed, with a final line feed.
fn sample_text(layout: &Layout) -> String {
    serde_json::to_string_pretty(layout).unwrap() + "\n"
}

/// Compares an extraction with its committed sample, or rewrites the sample when `BAYAN_UPDATE_GENERATED` is set (on the host only: the tests in WebAssembly have no files).
fn check_sample(file: &str, committed: &str, layout: &Layout) {
    let generated = sample_text(layout);
    #[cfg(not(target_arch = "wasm32"))]
    {
        if std::env::var_os("BAYAN_UPDATE_GENERATED").is_some() {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../schema/samples")
                .join(file);
            std::fs::write(path, &generated).unwrap();
            return;
        }
    }
    assert!(
        committed.replace("\r\n", "\n") == generated,
        "lab/schema/samples/{file} is out of date with the extractor. Regenerate it with `BAYAN_UPDATE_GENERATED=1 cargo test -p bayan-lab-layout --test layout` and review the difference."
    );
}

/// The lines of a page as (text, direction).
fn page_lines(layout: &Layout, page: usize) -> Vec<(String, Direction)> {
    layout.pages[page]
        .lines
        .iter()
        .map(|line| (line.text.clone().unwrap(), line.direction.unwrap()))
        .collect()
}

#[test]
fn libreoffice_glyphs_are_where_poppler_finds_them() {
    let extraction = extract(LIBREOFFICE);
    assert_eq!(
        (extraction.unmapped_glyphs, extraction.skipped_glyphs),
        (0, 0)
    );
    assert_eq!(check_words(&extraction.layout, LIBREOFFICE_WORDS), 79);
    let layout = &extraction.layout;
    assert_eq!(layout.page_count, 2);
    assert_eq!(
        layout
            .fonts
            .iter()
            .map(|font| font.name.as_str())
            .collect::<Vec<_>>(),
        ["LiberationSerif"]
    );
    let ltr = |text: &str| (text.to_owned(), Direction::Ltr);
    let rtl = |text: &str| (text.to_owned(), Direction::Rtl);
    assert_eq!(
        page_lines(layout, 0),
        [
            ltr("The quick brown fox jumps over the lazy dog. Kerning: AVAWAY, To Te."),
            ltr("Ligatures office fluffy affirm, larger text, and spaced letters."),
            ltr("Energy: E = mc2 with a superscript."),
            rtl("שלום עולם, זהו טקסט בעברית."),
            rtl("בשנת 2026 נכתב PDF חדש עם 3 עמודים."),
            ltr("English with a Hebrew word: the word שלום means peace."),
            ltr("A paragraph with a border below it."),
            // A table row whose cells share a baseline is one line (`lab/schema/layout-json.md`, "Lines from a PDF").
            ltr("Alpha1"),
            ltr("Beta22"),
        ]
    );
    assert_eq!(
        page_lines(layout, 1),
        [ltr("The second page starts here, with a final line.")]
    );
    // The paragraph border and the table's six borders.
    assert_eq!(layout.pages[0].elements.as_ref().map(Vec::len), Some(7));
    check_sample("pdf-libreoffice.json", LIBREOFFICE_SAMPLE, layout);
}

#[test]
fn chromium_glyphs_are_where_poppler_finds_them() {
    let extraction = extract(CHROMIUM);
    assert_eq!(
        (extraction.unmapped_glyphs, extraction.skipped_glyphs),
        (0, 0)
    );
    assert_eq!(check_words(&extraction.layout, CHROMIUM_WORDS), 69);
    let layout = &extraction.layout;
    assert_eq!(layout.page_count, 1);
    let ltr = |text: &str| (text.to_owned(), Direction::Ltr);
    let rtl = |text: &str| (text.to_owned(), Direction::Rtl);
    assert_eq!(
        page_lines(layout, 0),
        [
            ltr("Printed by Chromium from HTML, in Liberation Sans at 11 points."),
            ltr("This paragraph is justified, so the spaces between"),
            ltr("its words stretch to fill each line except the last one"),
            ltr("of the paragraph."),
            ltr("Letter spacing"),
            ltr("Word spacing makes gaps wider"),
            ltr("H2O and x2 keep their lines."),
            rtl("שלום עולם, זהו טקסט בעברית."),
            rtl("בשנת 2026 נכתב PDF חדש עם 3 עמודים."),
            ltr("An image:  inline."),
            ltr("Alpha1"),
            ltr("Beta22"),
        ]
    );
    let images = layout.pages[0]
        .elements
        .as_deref()
        .unwrap()
        .iter()
        .filter(|element| element.kind == bayan_lab_layout::layout::ElementKind::Image)
        .count();
    assert_eq!(images, 1);
    check_sample("pdf-chromium.json", CHROMIUM_SAMPLE, layout);
}
