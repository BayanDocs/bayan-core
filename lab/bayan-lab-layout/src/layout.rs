//! The layout JSON, version 1: what a page looks like, line by line and glyph by glyph, in one format for every producer ([`SCHEMA_ID`]).
//!
//! Three producers write it ([`Source`]): bayan-lab's PDF extractor, from the PDFs that Word exports on the reference machine ([`crate::pdf`]); Word's object model, through the harness of LAB-002, which can give page sizes and the positions where pages and sampled lines start but no glyphs; and BayanDocs' own engine, through `bayan-cli`. The comparisons of LAB-004 read two of them for the same document. `lab/schema/layout-json.md` explains every field and the rules for the text, and `lab/schema/layout.schema.json` is the JSON Schema generated from these types ([`crate::schema`]).
//!
//! Every length is in points, from the top left corner of the page as displayed, x to the right and y down, with a fixed precision of a thousandth of a point ([`Points`]). Where the engine produced a length, it also gives it in BLU (ADR-0005), in a field of the same name with `_blu` appended. [`Layout::check`] enforces the rules that the JSON Schema cannot express.

use std::collections::BTreeSet;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::error::Category;

/// The identifier of version 1 of the layout JSON, the value of every file's `schema` field.
pub const SCHEMA_ID: &str = "urn:bayandocs:lab:layout:v1";

/// BLU per point: 1 BLU is 1/25,400 of a point (ADR-0005).
pub const BLU_PER_POINT: i64 = 25_400;

/// The largest magnitude of a length, in thousandths of a point: a billion points, about 350 kilometres. Every such value is exactly an `f64`, so lengths survive JSON unchanged; anything larger is refused.
pub const MAX_MILLIPOINTS: i64 = 1_000_000_000_000;

/// The longest `producer`, font name or family, in characters.
pub const MAX_LABEL_CHARS: usize = 256;

/// A length or a coordinate, kept as a whole number of thousandths of a point: the layout JSON's fixed precision. In JSON it is a number of points with at most three decimals, such as `72.0` or `841.89`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Points(i64);

impl Points {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// The length of `millis` thousandths of a point, or `None` beyond [`MAX_MILLIPOINTS`].
    #[must_use]
    pub const fn from_millis(millis: i64) -> Option<Self> {
        if millis.unsigned_abs() > MAX_MILLIPOINTS.unsigned_abs() {
            None
        } else {
            Some(Self(millis))
        }
    }

    /// The length in thousandths of a point.
    #[must_use]
    pub const fn millis(self) -> i64 {
        self.0
    }

    /// The length of `blu` BLU, rounded to the nearest thousandth of a point (no value lies exactly halfway, since a thousandth of a point is 25.4 BLU), or `None` beyond [`MAX_MILLIPOINTS`].
    #[must_use]
    pub fn from_blu(blu: i64) -> Option<Self> {
        // A thousandth of a point is 25.4 BLU, so millis = blu × 5 / 127, rounded.
        let numerator = i128::from(blu).checked_mul(5)?;
        let magnitude = (numerator.unsigned_abs() * 2 + 127) / 254;
        let millis = i64::try_from(magnitude).ok()?;
        Self::from_millis(if blu < 0 { -millis } else { millis })
    }

    /// The length of `points` points, rounded to the nearest thousandth (halves away from zero), or `None` if it is not finite or beyond [`MAX_MILLIPOINTS`].
    #[must_use]
    pub fn from_points(points: f64) -> Option<Self> {
        let scaled = (points * 1000.0).round();
        whole_f64_to_i64(scaled).and_then(Self::from_millis)
    }

    /// The length in points, exactly (every value of this type is exactly an `f64` number of thousandths).
    #[must_use]
    pub fn to_points(self) -> f64 {
        millis_to_f64(self.0) / 1000.0
    }
}

/// `value` as an `i64`, if it is a whole number of magnitude at most [`MAX_MILLIPOINTS`].
fn whole_f64_to_i64(value: f64) -> Option<i64> {
    // The limit is far below 2^53, so the comparison and the conversion are exact.
    let limit = 1.0e12;
    if !value.is_finite() || value.trunc() != value || value > limit || value < -limit {
        return None;
    }
    Some(f64_to_i64_exact(value))
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "the only caller passes a whole number of magnitude at most 10^12, which an i64 holds exactly"
)]
const fn f64_to_i64_exact(value: f64) -> i64 {
    value as i64
}

#[expect(
    clippy::cast_precision_loss,
    reason = "lengths are at most 10^12 thousandths of a point in magnitude, which an f64 holds exactly"
)]
const fn millis_to_f64(millis: i64) -> f64 {
    millis as f64
}

impl fmt::Display for Points {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let magnitude = self.0.unsigned_abs();
        write!(
            formatter,
            "{sign}{}.{:03} pt",
            magnitude / 1000,
            magnitude % 1000
        )
    }
}

impl Serialize for Points {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64(self.to_points())
    }
}

impl<'de> Deserialize<'de> for Points {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PointsVisitor)
    }
}

/// Reads a JSON number of points.
struct PointsVisitor;

/// The message for a length beyond [`MAX_MILLIPOINTS`] thousandths of a point.
const OUT_OF_RANGE: &str = "the length is out of range";

/// The message for a length with more than three decimals.
const TOO_PRECISE: &str = "a length has more than three decimals";

impl Visitor<'_> for PointsVisitor {
    type Value = Points;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a number of points with at most three decimals")
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Points, E> {
        value
            .checked_mul(1000)
            .and_then(Points::from_millis)
            .ok_or_else(|| E::custom(OUT_OF_RANGE))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Points, E> {
        i64::try_from(value)
            .map_err(|_| E::custom(OUT_OF_RANGE))
            .and_then(|value| self.visit_i64(value))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Points, E> {
        let scaled = value * 1000.0;
        let rounded = scaled.round();
        let millis = whole_f64_to_i64(rounded)
            .and_then(Points::from_millis)
            .ok_or_else(|| E::custom(OUT_OF_RANGE))?;
        // A number written with at most three decimals is within a rounding error of a whole number of thousandths; the tolerance is far larger than that error for every length in range, and far smaller than a ten-thousandth.
        if (scaled - rounded).abs() > 0.001 {
            return Err(E::custom(TOO_PRECISE));
        }
        Ok(millis)
    }
}

impl JsonSchema for Points {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Points".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A length or coordinate in points, with at most three decimals (a fixed precision of a thousandth of a point).",
            "type": "number",
            "minimum": -1_000_000_000,
            "maximum": 1_000_000_000
        })
    }
}

/// The version of the layout JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum SchemaVersion {
    /// Version 1, this one.
    #[serde(rename = "urn:bayandocs:lab:layout:v1")]
    V1,
}

/// Who produced a layout JSON, which tells what its fields can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// bayan-lab's PDF extractor, from a PDF (in the lab, the PDF that Word exported): glyphs, lines, images and rules, with offsets into its own text (`text_offsets` is `lines`).
    Pdf,
    /// Word's object model, through the ground-truth harness of LAB-002: page sizes and the document positions where pages and sampled lines start; no glyphs.
    Word,
    /// BayanDocs' engine, through `bayan-cli`: everything, with lengths also in BLU.
    Engine,
}

/// What the `start` and `end` offsets of pages and lines count.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextOffsets {
    /// Characters (Unicode scalar values) of the file's own text stream: the `text` of every line, page after page and line after line, each followed by one line feed (U+000A). Every line then has `start`, `end` and `text`, and `end - start` is the number of characters of its `text`.
    Lines,
    /// Positions in the source document's text, as its producer counts them (Word's character positions, or the engine's). They increase through the document but need not match the lines' `text`, which holds what is displayed.
    Document,
}

/// Whether the pages list every line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LineCoverage {
    /// Every line of every page.
    All,
    /// Only some lines (Word's object model samples paragraphs).
    Sampled,
}

/// The base direction of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Left to right.
    Ltr,
    /// Right to left.
    Rtl,
}

/// A kind of positioned element other than text.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    /// An image, by the box it fills.
    Image,
    /// A straight line or a thin filled rectangle, horizontal or vertical: borders, underlines, separators.
    Rule,
    /// The box of a table cell.
    Cell,
}

/// A layout JSON file: the layout of one document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// The format's version: always `urn:bayandocs:lab:layout:v1`.
    pub schema: SchemaVersion,
    /// Who produced the file.
    pub source: Source,
    /// The program and its version, such as `bayan-lab pdf-extract 1`: printable text of at most 256 characters.
    pub producer: String,
    /// What the offsets of pages and lines count.
    pub text_offsets: TextOffsets,
    /// Whether the pages list every line.
    pub line_coverage: LineCoverage,
    /// The number of pages; equal to the length of `pages`.
    pub page_count: u32,
    /// The fonts that glyphs refer to by their index in this list.
    pub fonts: Vec<Font>,
    /// The pages, in order.
    pub pages: Vec<Page>,
}

/// A font that glyphs use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Font {
    /// The font's name as the producer knows it: in a PDF the `BaseFont` without the six-letter subset tag (`Calibri-Bold`, `TimesNewRomanPSMT`), which is the font's PostScript name for TrueType and OpenType fonts; for the engine, the PostScript name of the font file.
    pub name: String,
    /// The font family, where the producer knows it: the PDF's `FontFamily`, or the engine's family name. Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
}

/// A page.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Page {
    /// The width of the page as displayed (in a PDF, the crop box turned by the page's rotation).
    pub width: Points,
    /// The height of the page as displayed.
    pub height: Points,
    /// The width in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_blu: Option<i64>,
    /// The height in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height_blu: Option<i64>,
    /// The offset of the page's first character, where the producer knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    /// The lines of the page in reading order (see `lab/schema/layout-json.md`).
    pub lines: Vec<Line>,
    /// The page's images, rules and table cells, where the producer reports them, in drawing order; absent when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elements: Option<Vec<Element>>,
    /// The PDF extractor's count of glyphs that it drew on no line: text that is not upright, and invisible text (render modes 3 and 7). Absent for other producers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_glyphs: Option<u64>,
}

/// A line: text on one baseline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Line {
    /// The offset of the line's first character, where the producer knows it (see `text_offsets`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    /// The offset after its last character, where the producer knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    /// The line's characters in logical (reading) order, by the text rules of `lab/schema/layout-json.md`, where the producer knows them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// The line's base direction, where the producer knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// The y of the line's baseline, where the producer knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<Points>,
    /// The baseline in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_blu: Option<i64>,
    /// The line's glyphs in display order, from left to right, where the producer knows them. Requires `text`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyphs: Option<Vec<Glyph>>,
}

/// A glyph placed on a line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Glyph {
    /// The index, in the line's `text`, of the first character that the glyph shows, counted in characters (Unicode scalar values).
    pub offset: u32,
    /// How many characters of the line's `text` the glyph shows: more than one for a ligature, zero for a glyph that shows no character of the text. Glyphs of one cluster share the same characters; other glyphs never share any.
    pub length: u32,
    /// The x of the glyph's origin.
    pub x: Points,
    /// The y of the glyph's origin, on its baseline (above the line's baseline for a superscript).
    pub y: Points,
    /// How far the glyph moves the pen along its baseline: its advance width at its size, with the producer's character and word spacing, without kerning or other positioning that moves the next glyph only.
    pub advance: Points,
    /// The index of its font in `fonts`.
    pub font: u32,
    /// The font size: the height of the em square, perpendicular to the baseline.
    pub size: Points,
    /// The glyph identifier in the font, where the producer knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
    /// `x` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x_blu: Option<i64>,
    /// `y` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y_blu: Option<i64>,
    /// `advance` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advance_blu: Option<i64>,
    /// `size` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_blu: Option<i64>,
}

/// A positioned element other than text, by its bounding box.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Element {
    /// What it is.
    pub kind: ElementKind,
    /// The left edge.
    pub x0: Points,
    /// The top edge.
    pub y0: Points,
    /// The right edge, at least `x0`.
    pub x1: Points,
    /// The bottom edge, at least `y0`.
    pub y1: Points,
    /// `x0` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x0_blu: Option<i64>,
    /// `y0` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y0_blu: Option<i64>,
    /// `x1` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x1_blu: Option<i64>,
    /// `y1` in BLU, where the engine produced it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y1_blu: Option<i64>,
}

impl Layout {
    /// Reads a layout JSON file and checks its rules ([`Layout::check`]).
    ///
    /// # Errors
    ///
    /// A message if the text is not a layout JSON of version 1 or breaks one of its rules.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let layout: Self = serde_json::from_str(text).map_err(|error| json_error(&error))?;
        layout.check()?;
        Ok(layout)
    }

    /// The layout JSON text: compact, with the fields in the order of the types above, and ending with a line feed.
    ///
    /// # Errors
    ///
    /// A message if serialization fails, which it cannot for these types.
    pub fn to_json(&self) -> Result<String, String> {
        let mut text = Vec::new();
        self.write_json(&mut text)?;
        String::from_utf8(text).map_err(|error| error.to_string())
    }

    /// Writes the text of [`Layout::to_json`] to `writer` as it is formed, so that a large layout's text is never in memory as a whole.
    ///
    /// # Errors
    ///
    /// A message if writing fails.
    pub fn write_json(&self, mut writer: impl std::io::Write) -> Result<(), String> {
        serde_json::to_writer(&mut writer, self).map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())
    }

    /// Checks the rules that the types and the JSON Schema cannot express. `lab/schema/layout-json.md` lists them; the first one broken is reported, with where.
    ///
    /// # Errors
    ///
    /// A message naming the field that breaks a rule, as a path such as `pages[0].lines[3].glyphs[2]`.
    pub fn check(&self) -> Result<(), String> {
        check_label("producer", &self.producer)?;
        if usize::try_from(self.page_count).ok() != Some(self.pages.len()) {
            return Err(format!(
                "page_count is {} but there are {} pages",
                self.page_count,
                self.pages.len()
            ));
        }
        for (index, font) in self.fonts.iter().enumerate() {
            check_label(&format!("fonts[{index}].name"), &font.name)?;
            if let Some(family) = &font.family {
                check_label(&format!("fonts[{index}].family"), family)?;
            }
        }
        if self.source == Source::Pdf {
            self.check_pdf_fields()?;
        }
        if self.text_offsets == TextOffsets::Lines && self.line_coverage != LineCoverage::All {
            return Err("text_offsets `lines` needs line_coverage `all`".to_owned());
        }
        let mut stream = 0_u64;
        for (page_index, page) in self.pages.iter().enumerate() {
            let at = format!("pages[{page_index}]");
            self.check_page(&at, page, &mut stream)?;
        }
        Ok(())
    }

    /// The fields that every file of the PDF extractor has.
    fn check_pdf_fields(&self) -> Result<(), String> {
        if self.text_offsets != TextOffsets::Lines {
            return Err(
                "a layout from a PDF counts offsets in its lines (text_offsets `lines`)".to_owned(),
            );
        }
        for (page_index, page) in self.pages.iter().enumerate() {
            if page.elements.is_none() || page.skipped_glyphs.is_none() {
                return Err(format!(
                    "pages[{page_index}]: a layout from a PDF gives every page's elements and skipped_glyphs"
                ));
            }
            for (line_index, line) in page.lines.iter().enumerate() {
                if line.direction.is_none() || line.baseline.is_none() || line.glyphs.is_none() {
                    return Err(format!(
                        "pages[{page_index}].lines[{line_index}]: a layout from a PDF gives every line's direction, baseline and glyphs"
                    ));
                }
            }
        }
        Ok(())
    }

    /// One page's rules; `stream` is the offset where the page starts in a `lines` text stream, and moves past it.
    fn check_page(&self, at: &str, page: &Page, stream: &mut u64) -> Result<(), String> {
        if page.width <= Points::ZERO || page.height <= Points::ZERO {
            return Err(format!(
                "{at}: the page's width and height must be positive"
            ));
        }
        check_blu(&format!("{at}.width"), page.width, page.width_blu)?;
        check_blu(&format!("{at}.height"), page.height, page.height_blu)?;
        if self.text_offsets == TextOffsets::Lines
            && let Some(start) = page.start
            && start != *stream
        {
            return Err(format!(
                "{at}.start is {start}, but the page starts at offset {stream} of the text"
            ));
        }
        let mut previous_start = page.start;
        for (line_index, line) in page.lines.iter().enumerate() {
            let at = format!("{at}.lines[{line_index}]");
            self.check_line(&at, line, stream)?;
            if self.text_offsets == TextOffsets::Document
                && let Some(start) = line.start
            {
                if previous_start.is_some_and(|previous| start < previous) {
                    return Err(format!(
                        "{at}.start is {start}, before the start of the line or page before it"
                    ));
                }
                previous_start = Some(start);
            }
        }
        if let Some(elements) = &page.elements {
            for (index, element) in elements.iter().enumerate() {
                check_element(&format!("{at}.elements[{index}]"), element)?;
            }
        }
        Ok(())
    }

    fn check_line(&self, at: &str, line: &Line, stream: &mut u64) -> Result<(), String> {
        let characters = match &line.text {
            Some(text) => Some(check_text(at, text)?),
            None => None,
        };
        match self.text_offsets {
            TextOffsets::Lines => {
                let (Some(start), Some(end), Some(characters)) = (line.start, line.end, characters)
                else {
                    return Err(format!(
                        "{at}: with text_offsets `lines`, every line has start, end and text"
                    ));
                };
                if start != *stream {
                    return Err(format!(
                        "{at}.start is {start}, but the line starts at offset {stream} of the text"
                    ));
                }
                let expected_end = start
                    .checked_add(characters)
                    .ok_or_else(|| format!("{at}: the offsets are out of range"))?;
                if end != expected_end {
                    return Err(format!(
                        "{at}.end is {end}, but its text ends at offset {expected_end}"
                    ));
                }
                *stream = end
                    .checked_add(1)
                    .ok_or_else(|| format!("{at}: the offsets are out of range"))?;
            }
            TextOffsets::Document => {
                if let (Some(start), Some(end)) = (line.start, line.end)
                    && end < start
                {
                    return Err(format!("{at}: end {end} is before start {start}"));
                }
            }
        }
        if let Some(baseline) = line.baseline {
            check_blu(&format!("{at}.baseline"), baseline, line.baseline_blu)?;
        } else if line.baseline_blu.is_some() {
            return Err(format!("{at}: baseline_blu without baseline"));
        }
        if let Some(glyphs) = &line.glyphs {
            let Some(characters) = characters else {
                return Err(format!("{at}: a line with glyphs needs its text"));
            };
            self.check_glyphs(at, glyphs, characters)?;
        }
        Ok(())
    }

    fn check_glyphs(&self, at: &str, glyphs: &[Glyph], characters: u64) -> Result<(), String> {
        let mut ranges = BTreeSet::new();
        for (index, glyph) in glyphs.iter().enumerate() {
            let at = format!("{at}.glyphs[{index}]");
            let end = u64::from(glyph.offset) + u64::from(glyph.length);
            if end > characters {
                return Err(format!(
                    "{at}: characters {}..{end} are beyond the line's {characters} characters",
                    glyph.offset
                ));
            }
            let known_font = usize::try_from(glyph.font).is_ok_and(|font| font < self.fonts.len());
            if !known_font {
                return Err(format!(
                    "{at}: font {} is not in fonts, which has {}",
                    glyph.font,
                    self.fonts.len()
                ));
            }
            if glyph.size <= Points::ZERO {
                return Err(format!("{at}: the size must be positive"));
            }
            check_blu(&format!("{at}.x"), glyph.x, glyph.x_blu)?;
            check_blu(&format!("{at}.y"), glyph.y, glyph.y_blu)?;
            check_blu(&format!("{at}.advance"), glyph.advance, glyph.advance_blu)?;
            check_blu(&format!("{at}.size"), glyph.size, glyph.size_blu)?;
            if glyph.length > 0 {
                ranges.insert((u64::from(glyph.offset), end));
            }
        }
        // Distinct ranges must not overlap: glyphs of one cluster share theirs exactly.
        let mut previous_end = 0;
        for (start, end) in ranges {
            if start < previous_end {
                return Err(format!(
                    "{at}: two glyphs show overlapping but different characters (one of them starts at {start})"
                ));
            }
            previous_end = end;
        }
        Ok(())
    }
}

/// The message for a JSON error: the kind of error and where it is, never a value of the file. serde_json's own messages quote the value they did not expect (`invalid type: string "…"`, an unknown variant or field), and a layout JSON's values can be a document's text, which must never reach a log. The messages of this module's own checks of lengths quote nothing and are kept.
fn json_error(error: &serde_json::Error) -> String {
    let message = error.to_string();
    let own = [OUT_OF_RANGE, TOO_PRECISE]
        .into_iter()
        .find(|own| message.starts_with(own));
    let kind = match error.classify() {
        Category::Io => "the text could not be read",
        Category::Syntax => "the text is not valid JSON",
        Category::Eof => "the text ends in the middle of a value",
        Category::Data => own.unwrap_or(
            "a value does not fit the layout JSON (a value of the wrong type or out of range, an unknown or missing field, or an unknown name)",
        ),
    };
    format!("line {}, column {}: {kind}", error.line(), error.column())
}

/// The number of characters of a line's text, which must not contain control characters other than the tab, nor U+FFFE, U+FFFF or other noncharacters.
fn check_text(at: &str, text: &str) -> Result<u64, String> {
    let mut count = 0_u64;
    for character in text.chars() {
        if (character.is_control() && character != '\t') || is_noncharacter(character) {
            return Err(format!(
                "{at}.text contains the control or noncharacter U+{:04X}",
                u32::from(character)
            ));
        }
        count += 1;
    }
    Ok(count)
}

/// Whether `character` is one of Unicode's 66 noncharacters.
fn is_noncharacter(character: char) -> bool {
    let value = u32::from(character);
    (0xFDD0..=0xFDEF).contains(&value) || (value & 0xFFFE) == 0xFFFE
}

/// A name or label: 1 to 256 characters, none of them control characters.
fn check_label(at: &str, label: &str) -> Result<(), String> {
    let characters = label.chars().count();
    if characters == 0 || characters > MAX_LABEL_CHARS {
        return Err(format!(
            "{at} must have 1 to {MAX_LABEL_CHARS} characters, not {characters}"
        ));
    }
    if label.chars().any(char::is_control) {
        return Err(format!("{at} contains a control character"));
    }
    Ok(())
}

/// A length given in BLU too must be the BLU value rounded to a thousandth of a point.
fn check_blu(at: &str, points: Points, blu: Option<i64>) -> Result<(), String> {
    let Some(blu) = blu else {
        return Ok(());
    };
    match Points::from_blu(blu) {
        Some(expected) if expected == points => Ok(()),
        Some(expected) => Err(format!(
            "{at} is {points}, but its BLU value {blu} is {expected}"
        )),
        None => Err(format!("{at}: the BLU value {blu} is out of range")),
    }
}

fn check_element(at: &str, element: &Element) -> Result<(), String> {
    if element.x1 < element.x0 || element.y1 < element.y0 {
        return Err(format!("{at}: x1 and y1 must not be less than x0 and y0"));
    }
    check_blu(&format!("{at}.x0"), element.x0, element.x0_blu)?;
    check_blu(&format!("{at}.y0"), element.y0, element.y0_blu)?;
    check_blu(&format!("{at}.x1"), element.x1, element.x1_blu)?;
    check_blu(&format!("{at}.y1"), element.y1, element.y1_blu)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_keep_a_thousandth_of_a_point() {
        assert_eq!(Points::from_points(72.0), Points::from_millis(72_000));
        assert_eq!(Points::from_points(841.889_7), Points::from_millis(841_890));
        assert_eq!(Points::from_points(-0.000_4), Points::from_millis(0));
        assert_eq!(Points::from_points(-0.000_5), Points::from_millis(-1));
        assert_eq!(Points::from_points(f64::NAN), None);
        assert_eq!(Points::from_points(f64::INFINITY), None);
        assert_eq!(
            Points::from_points(1.0e9),
            Points::from_millis(MAX_MILLIPOINTS)
        );
        assert_eq!(Points::from_points(1.000_000_001e9), None);
        assert_eq!(Points::from_millis(MAX_MILLIPOINTS + 1), None);
        assert_eq!(Points::from_millis(-MAX_MILLIPOINTS - 1), None);
        assert_eq!(Points::from_millis(1_500).unwrap().to_string(), "1.500 pt");
        assert_eq!(Points::from_millis(-25).unwrap().to_string(), "-0.025 pt");
    }

    #[test]
    fn blu_rounds_to_the_nearest_thousandth_of_a_point() {
        assert_eq!(Points::from_blu(25_400), Points::from_millis(1_000));
        assert_eq!(Points::from_blu(12_700), Points::from_millis(500));
        // 25.4 BLU is a thousandth: 12 BLU rounds down to 0, 13 BLU up to 1.
        assert_eq!(Points::from_blu(12), Points::from_millis(0));
        assert_eq!(Points::from_blu(13), Points::from_millis(1));
        assert_eq!(Points::from_blu(-13), Points::from_millis(-1));
        assert_eq!(Points::from_blu(i64::MAX), None);
        assert_eq!(Points::from_blu(i64::MIN), None);
    }

    #[test]
    fn points_are_written_as_numbers_and_read_back() {
        let length = Points::from_millis(841_890).unwrap();
        assert_eq!(serde_json::to_string(&length).unwrap(), "841.89");
        assert_eq!(serde_json::to_string(&Points::ZERO).unwrap(), "0.0");
        for text in ["841.89", "841.890", "8.4189e2"] {
            assert_eq!(serde_json::from_str::<Points>(text).unwrap(), length);
        }
        assert_eq!(
            serde_json::from_str::<Points>("72").unwrap(),
            Points::from_millis(72_000).unwrap()
        );
        assert!(serde_json::from_str::<Points>("841.8901").is_err());
        assert!(serde_json::from_str::<Points>("1e10").is_err());
        assert!(serde_json::from_str::<Points>("10000000000").is_err());
        assert!(serde_json::from_str::<Points>("\"72\"").is_err());
    }
}
