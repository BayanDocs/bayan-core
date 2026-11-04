//! Helpers of the tests: a writer of small synthetic PDFs whose glyph positions the tests compute by hand, the test fonts, and lookups into extracted layouts.

pub mod schema_validator;

use bayan_lab_layout::layout::{Glyph, Layout, Line};
use bayan_lab_layout::pdf::{self, Error, Extraction, Limits};

/// Writes a PDF object by object, with a classic cross-reference table or a cross-reference stream.
#[derive(Default)]
pub struct Builder {
    objects: Vec<Option<Vec<u8>>>,
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an object and returns its number.
    pub fn add(&mut self, body: impl AsRef<[u8]>) -> u32 {
        self.objects.push(Some(body.as_ref().to_vec()));
        u32::try_from(self.objects.len()).unwrap()
    }

    /// Reserves an object number, to be set later.
    pub fn reserve(&mut self) -> u32 {
        self.objects.push(None);
        u32::try_from(self.objects.len()).unwrap()
    }

    /// Sets a reserved object.
    pub fn set(&mut self, number: u32, body: impl AsRef<[u8]>) {
        self.objects[usize::try_from(number).unwrap() - 1] = Some(body.as_ref().to_vec());
    }

    /// The header and every object, with the offset of each.
    fn body(&self) -> (Vec<u8>, Vec<usize>) {
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            out.extend_from_slice(object.as_deref().expect("every reserved object is set"));
            out.extend_from_slice(b"\nendobj\n");
        }
        (out, offsets)
    }

    /// The file with a cross-reference table and a trailer naming `root`, plus `trailer` entries.
    pub fn finish_with(&self, root: u32, trailer: &str) -> Vec<u8> {
        let (mut out, offsets) = self.body();
        let xref = out.len();
        out.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
        );
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root {root} 0 R {trailer} >>\nstartxref\n{xref}\n%%EOF\n",
                self.objects.len() + 1
            )
            .as_bytes(),
        );
        out
    }

    /// The file with a cross-reference table.
    pub fn finish(&self, root: u32) -> Vec<u8> {
        self.finish_with(root, "")
    }

    /// The file with a cross-reference stream (compressed, with the PNG Up predictor) instead of a table.
    pub fn finish_with_xref_stream(&self, root: u32) -> Vec<u8> {
        let (mut out, offsets) = self.body();
        let xref = out.len();
        let size = offsets.len() + 2;
        // Object 0 is free; then every object; then the cross-reference stream itself.
        let mut entries: Vec<[u8; 4]> = vec![[0, 0, 0, 0xFF]];
        for offset in offsets.iter().copied().chain([xref]) {
            let offset = u16::try_from(offset).unwrap().to_be_bytes();
            entries.push([1, offset[0], offset[1], 0]);
        }
        // Each row is PNG-filtered with Up: the difference from the row above it (zeros above the first).
        let mut rows = Vec::new();
        let mut previous = [0_u8; 4];
        for entry in entries {
            rows.push(2);
            for (byte, above) in entry.iter().zip(previous) {
                rows.push(byte.wrapping_sub(above));
            }
            previous = entry;
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&rows, 6);
        out.extend_from_slice(
            format!(
                "{} 0 obj\n<< /Type /XRef /Size {size} /W [1 2 1] /Root {root} 0 R /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >> /Length {} >>\nstream\n",
                size - 1,
                compressed.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&compressed);
        out.extend_from_slice(
            format!("\nendstream\nendobj\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    }
}

/// A stream object: `dict` entries plus `Length`, and `data`.
pub fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
    let mut out = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\nendstream");
    out
}

/// A FlateDecode stream object.
pub fn flate_stream(dict: &str, data: &[u8]) -> Vec<u8> {
    stream(
        &format!("{dict} /Filter /FlateDecode"),
        &miniz_oxide::deflate::compress_to_vec_zlib(data, 6),
    )
}

/// The widths of the simple test font `TestSans`, in thousandths of an em, for codes 32 to 126: 500, except the space, `i` and `l` (250), `m` (750) and `W` (900).
pub fn test_sans_width(code: u8) -> f64 {
    match code {
        b' ' | b'i' | b'l' => 250.0,
        b'm' => 750.0,
        b'W' => 900.0,
        _ => 500.0,
    }
}

/// The simple test font: TrueType, WinAnsiEncoding, no `ToUnicode`.
pub fn test_sans() -> String {
    let widths: Vec<String> = (32..=126_u8)
        .map(|code| format!("{}", test_sans_width(code)))
        .collect();
    format!(
        "<< /Type /Font /Subtype /TrueType /BaseFont /ABCDEF+TestSans /FirstChar 32 /LastChar 126 /Widths [{}] /Encoding /WinAnsiEncoding >>",
        widths.join(" ")
    )
}

/// The `ToUnicode` map of the two-byte test font: CIDs 1 to 27 are the Hebrew letters and final forms from א to ת (U+05D0 to U+05EA), 30 to 39 the digits, 40 the space, 41 the full stop, 50 a ligature of f and i, 51 a glyph without characters, and 60 to 63 Arabic: م, the ligature لا (lam and alef), س and the Arabic comma.
pub const TEST_CID_TO_UNICODE: &str = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap
/CMapName /TestCID-UCS def /CMapType 2 def
1 begincodespacerange <0000> <FFFF> endcodespacerange
6 beginbfrange
<0001> <001B> <05D0>
<001E> <0027> <0030>
<0028> <0028> <0020>
<0029> <0029> <002E>
<0032> <0032> <00660069>
<003C> <003F> [<0645> <06440627> <0633> <060C>]
endbfrange
1 beginbfchar
<0033> <>
endbfchar
endcmap CMapName currentdict /CMap defineresource pop end end";

/// The width of CID `cid` of the two-byte test font, in thousandths of an em: 600, except the space (300) and CIDs 60 to 63 (400).
pub fn test_cid_width(cid: u16) -> f64 {
    match cid {
        40 => 300.0,
        60..=63 => 400.0,
        _ => 600.0,
    }
}

/// Adds the two-byte test font (Type 0, `Identity-H`, a CIDFontType2 with `W` and a `ToUnicode` map) and returns the number of its Type 0 dictionary.
pub fn add_test_cid(builder: &mut Builder) -> u32 {
    let to_unicode = builder.add(flate_stream("", TEST_CID_TO_UNICODE.as_bytes()));
    let descriptor = builder.add("<< /Type /FontDescriptor /FontName /GHIJKL+TestCID /FontFamily (Test CID) /Flags 4 /FontBBox [0 -200 1000 800] /ItalicAngle 0 /Ascent 800 /Descent -200 /CapHeight 700 /StemV 80 >>");
    let cid_font = builder.add(format!(
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /GHIJKL+TestCID /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {descriptor} 0 R /DW 600 /W [40 [300] 60 63 400] /CIDToGIDMap /Identity >>"
    ));
    builder.add(format!(
        "<< /Type /Font /Subtype /Type0 /BaseFont /GHIJKL+TestCID-Identity-H /Encoding /Identity-H /DescendantFonts [{cid_font} 0 R] /ToUnicode {to_unicode} 0 R >>"
    ))
}

/// A two-byte string of CIDs, as a hexadecimal string.
pub fn cids(cids: &[u16]) -> String {
    let hex: String = cids.iter().map(|cid| format!("{cid:04X}")).collect();
    format!("<{hex}>")
}

/// A one-page document of 600 by 800 points with the fonts `/F1` (TestSans) and `/F2` (TestCID) and `content`.
pub fn one_page(content: &str) -> Vec<u8> {
    one_page_with(content, "", "")
}

/// [`one_page`] with extra resources and page entries.
pub fn one_page_with(content: &str, resources: &str, page_entries: &str) -> Vec<u8> {
    one_page_mapped(content, resources, page_entries, |body| body.to_owned())
}

/// [`one_page_with`], with every object's body passed through `map` before the cross-reference table is written, so that a test can break one object and keep every offset right.
pub fn one_page_mapped(
    content: &str,
    resources: &str,
    page_entries: &str,
    map: impl Fn(&str) -> String,
) -> Vec<u8> {
    let mut builder = Builder::new();
    let catalog = builder.reserve();
    let pages = builder.reserve();
    let sans = builder.add(map(&test_sans()));
    let cid = add_test_cid(&mut builder);
    let contents = builder.add(flate_stream("", content.as_bytes()));
    let page = builder.add(map(&format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 600 800] /Resources << /Font << /F1 {sans} 0 R /F2 {cid} 0 R >> {resources} >> /Contents {contents} 0 R {page_entries} >>"
    )));
    builder.set(
        pages,
        map(&format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>")),
    );
    builder.set(
        catalog,
        map(&format!("<< /Type /Catalog /Pages {pages} 0 R >>")),
    );
    builder.finish(catalog)
}

/// [`one_page`] with one more font, `/F9`, given as a direct dictionary.
pub fn one_page_with_font(content: &str, font: &str) -> Vec<u8> {
    let font = font.to_owned();
    one_page_mapped(content, "", "", move |body| {
        body.replacen("/Font << ", &format!("/Font << /F9 {font} "), 1)
    })
}

/// The position of the first `needle` in `data`.
pub fn find(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len())
        .position(|window| window == needle)
}

/// `data` with the first `from` replaced by `to`, byte for byte (binary streams stay as they are).
pub fn replace_once(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = find(data, from)
        .unwrap_or_else(|| panic!("{} is not in the file", String::from_utf8_lossy(from)));
    [&data[..at], to, &data[at + from.len()..]].concat()
}

/// The offset that the last `startxref` of a file gives.
pub fn startxref(data: &[u8]) -> usize {
    let keyword = b"startxref\n";
    let at = data
        .windows(keyword.len())
        .rposition(|window| window == keyword)
        .unwrap()
        + keyword.len();
    let digits: Vec<u8> = data[at..]
        .iter()
        .copied()
        .take_while(u8::is_ascii_digit)
        .collect();
    String::from_utf8(digits).unwrap().parse().unwrap()
}

/// Extracts with the default limits, failing the test on an error.
pub fn extract(pdf: &[u8]) -> Extraction {
    match pdf::extract(pdf, &Limits::DEFAULT) {
        Ok(extraction) => extraction,
        Err(error) => panic!("extraction failed: {error}"),
    }
}

/// Extracts with the default limits and returns the error.
pub fn refuse(pdf: &[u8]) -> Error {
    match pdf::extract(pdf, &Limits::DEFAULT) {
        Ok(_) => panic!("the extractor accepted a PDF it should refuse"),
        Err(error) => error,
    }
}

/// The lines of the first page.
pub fn lines(layout: &Layout) -> &[Line] {
    &layout.pages[0].lines
}

/// The characters each glyph of a line shows, in display order.
pub fn glyph_texts(line: &Line) -> Vec<String> {
    let characters: Vec<char> = line.text.as_deref().unwrap().chars().collect();
    line.glyphs
        .as_deref()
        .unwrap()
        .iter()
        .map(|glyph| {
            let start = usize::try_from(glyph.offset).unwrap();
            let end = start + usize::try_from(glyph.length).unwrap();
            characters[start..end].iter().collect()
        })
        .collect()
}

/// Asserts that a length is within the 0.01 pt of AC-1 of the expected number of points.
#[track_caller]
pub fn assert_close(what: &str, actual: bayan_lab_layout::layout::Points, expected: f64) {
    let difference = (actual.to_points() - expected).abs();
    assert!(
        difference <= 0.01,
        "{what}: extracted {actual}, expected {expected} pt (off by {difference} pt)"
    );
}

/// Asserts a glyph's origin, advance and size.
#[track_caller]
pub fn assert_glyph(glyph: &Glyph, x: f64, y: f64, advance: f64, size: f64) {
    assert_close("x", glyph.x, x);
    assert_close("y", glyph.y, y);
    assert_close("advance", glyph.advance, advance);
    assert_close("size", glyph.size, size);
}
