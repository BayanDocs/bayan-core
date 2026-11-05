//! bayan-lab-layout's PDF extractor (LAB-003) on arbitrary bytes: the reader of a file's structure (cross-reference tables and streams, object streams, incremental updates), the stream filters, the fonts and their `ToUnicode` maps, the content-stream interpreter and the grouping of glyphs into lines. For every PDF it accepts: the layout keeps the format's rules, its JSON reads back as the same layout, every line's comparable text can be formed, and extracting the same bytes again gives the same layout.

#![no_main]

use bayan_lab_layout::layout::Layout;
use bayan_lab_layout::pdf::{Limits, extract};
use bayan_lab_layout::text::comparable;
use libfuzzer_sys::fuzz_target;

/// The default limits, lowered where an input of 64 KiB could otherwise keep the target busy for seconds (a form XObject that draws itself twice at each of 16 levels runs its operators 65,536 times), so that libFuzzer's timeout catches work that grows faster than its input rather than work the limits allow.
const LIMITS: Limits = Limits {
    max_objects: 65_536,
    max_object_values: 262_144,
    max_stream_bytes: 4 << 20,
    max_read_bytes: 16 << 20,
    max_operators: 500_000,
    max_glyphs: 100_000,
    max_font_entries: 500_000,
    max_path_segments: 100_000,
    max_elements: 100_000,
    ..Limits::DEFAULT
};

fuzz_target!(|data: &[u8]| {
    let Ok(extraction) = extract(data, &LIMITS) else {
        return;
    };
    let layout = &extraction.layout;
    layout.check().unwrap();
    let json = layout.to_json().unwrap();
    assert_eq!(&Layout::from_json(&json).unwrap(), layout);
    for line in layout.pages.iter().flat_map(|page| &page.lines) {
        let _ = comparable(line.text.as_deref().unwrap_or_default());
    }
    assert_eq!(extract(data, &LIMITS).unwrap(), extraction);
});
