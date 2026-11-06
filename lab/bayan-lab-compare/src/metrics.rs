//! The metrics of one document (Fidelity Lab specification §6): how Word's layout (the reference) and BayanDocs' layout (the candidate) of the same document agree.
//!
//! | Metric | What [`compare`] and [`hint_agreement`] compute |
//! |---|---|
//! | M1 page count | both page counts ([`PageCount`]); they match when the difference is 0 |
//! | M2 page-break agreement | of the reference's pages that have text, how many start at the same aligned position as a candidate page ([`Breaks`]) |
//! | M3 line-break agreement | of the reference's joined lines that have text, how many start at the same aligned position as a candidate line ([`Breaks`]) |
//! | M4 glyph position error | the median, 95th percentile and maximum horizontal and vertical distance between matched glyphs, on lines whose breaks agree, on pages whose starts agree ([`GlyphError`]) |
//! | M5 visual difference | not here: [`crate::visual`] |
//! | M6 round-trip integrity | not here: a process of the reference machine (`lab/compare.md`) |
//! | M7 hint agreement | of the `w:lastRenderedPageBreak` hints of the `.docx` file, how many fall at the start of a candidate page ([`HintAgreement`]) |
//!
//! Both texts are first made into streams ([`crate::stream`]) and aligned ([`crate::align`]); a break agrees when its image in the candidate's stream holds a candidate break of the same kind, each candidate break serving one reference break.
//!
//! ## Which glyphs M4 measures
//!
//! A reference line's breaks agree when its start agrees with a candidate line's start (M3) and its end falls where that candidate line ends. Its glyphs are measured only if its page's start agrees too (M2), with the candidate line on the page that start agrees with: a line that moved to another page, or sits lower because its page starts elsewhere, would measure pagination, not positioning. Only glyphs that show characters of the comparable form are measured (not spaces). The glyphs of the two lines are paired in one of two ways:
//!
//! 1. **By display order**, when both lines show the same glyphs from left to right: the same number, each showing the same characters. This also pairs the glyphs of a line whose display does not tell its reading order, which the extractor and the engine can read differently (`lab/schema/layout-json.md`, step 3 of "Lines from a PDF").
//! 2. **By the alignment** otherwise: a glyph (or a cluster of glyphs that show the same characters) is paired with the glyph whose characters are aligned with all of its characters, when that holds both ways and both clusters have as many glyphs. A ligature on one side and separate letters on the other are not paired.
//!
//! Distances are whole thousandths of a point, the layout JSON's precision, collected in [`Histogram`]s.

use core::fmt;

use bayan_lab_layout::layout::{Layout, MAX_MILLIPOINTS, Points};
use bayan_lab_layout::text::comparable_chars;
use serde::{Deserialize, Serialize};

use crate::align::{self, Alignment, align, match_breaks};
use crate::hints::Hints;
use crate::histogram::Histogram;
use crate::stream::{Stream, StreamLine, TooLong};

/// The limits of a comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The limits of the alignment.
    pub align: align::Limits,
    /// The most characters, in the comparable form, of either stream.
    pub max_chars: usize,
}

impl Limits {
    /// The default limits: [`align::Limits::DEFAULT`], and 16 million characters a stream, four times what the PDF extractor's default limits let a layout hold.
    pub const DEFAULT: Self = Self {
        align: align::Limits::DEFAULT,
        max_chars: 16 << 20,
    };
}

impl Default for Limits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a comparison could not be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A layout's text, or the document's for M7, has more characters than [`Limits::max_chars`].
    TooLong {
        /// Which: `reference`, `candidate` or `hints`.
        side: &'static str,
        /// The limit.
        limit: usize,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { side, limit } => write!(
                formatter,
                "the {side}'s text has more than {limit} characters in the comparable form"
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Where a break of the reference is: its page and, for a line, its line on that page, both counted from 1. The line is the layout's line where the joined line begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Place {
    /// The page, from 1.
    pub page: u32,
    /// The line on that page, from 1; absent for a page break.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// How many breaks of the reference agree (M2, M3, M7), and which.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Breaks {
    /// The reference's breaks.
    pub total: u64,
    /// How many of them agree.
    pub agreed: u64,
    /// The breaks that agree, by their index in the reference's order, as ranges `[first, end)`: what a baseline keeps, so that a later run can tell which breaks it lost.
    pub agreed_breaks: Vec<[u64; 2]>,
    /// Where the first break that does not agree is, if one does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_difference: Option<Place>,
}

impl Breaks {
    /// The breaks of `matches` (one entry per reference break: whether and with which candidate break it agrees), with `place` saying where each is.
    fn new(matches: &[Option<usize>], place: impl Fn(usize) -> Place) -> Self {
        let mut breaks = Self {
            total: as_u64(matches.len()),
            ..Self::default()
        };
        for (index, matched) in matches.iter().enumerate() {
            if matched.is_some() {
                breaks.agreed += 1;
                let index = as_u64(index);
                match breaks.agreed_breaks.last_mut() {
                    Some(range) if range[1] == index => range[1] = index + 1,
                    _ => breaks.agreed_breaks.push([index, index + 1]),
                }
            } else if breaks.first_difference.is_none() {
                breaks.first_difference = Some(place(index));
            }
        }
        breaks
    }

    /// Whether the break with index `index` agrees.
    #[must_use]
    pub fn agrees(&self, index: u64) -> bool {
        let at = self
            .agreed_breaks
            .partition_point(|range| range[1] <= index);
        self.agreed_breaks
            .get(at)
            .is_some_and(|range| range[0] <= index)
    }
}

/// The page counts (M1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageCount {
    /// The reference's pages.
    pub reference: u32,
    /// The candidate's pages.
    pub candidate: u32,
}

impl PageCount {
    /// The candidate's pages minus the reference's.
    #[must_use]
    pub fn difference(self) -> i64 {
        i64::from(self.candidate) - i64::from(self.reference)
    }

    /// Whether the counts match.
    #[must_use]
    pub const fn matches(self) -> bool {
        self.reference == self.candidate
    }
}

/// How much of the text the alignment matched.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAgreement {
    /// Characters of the reference's stream.
    pub reference: u64,
    /// Characters of the candidate's stream.
    pub candidate: u64,
    /// Characters of the reference's stream aligned with the same characters of the candidate's.
    pub matched: u64,
    /// Whether the alignment ran to its end within its limits.
    pub complete: bool,
}

/// The median, 95th percentile and maximum of a distribution of distances.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spread {
    /// The median, by nearest rank.
    pub median: Points,
    /// The 95th percentile, by nearest rank.
    pub p95: Points,
    /// The largest.
    pub max: Points,
}

impl Spread {
    /// The spread of `histogram` (of thousandths of a point), if it holds values.
    #[must_use]
    pub fn of(histogram: &Histogram) -> Option<Self> {
        // A distance between two lengths in range is at most twice the range; it is shown at most as the range's end.
        let points = |value: u64| {
            let millis =
                i64::try_from(value).map_or(MAX_MILLIPOINTS, |value| value.min(MAX_MILLIPOINTS));
            Points::from_millis(millis).unwrap_or(Points::ZERO)
        };
        Some(Self {
            median: points(histogram.percentile(500)?),
            p95: points(histogram.percentile(950)?),
            max: points(histogram.max()?),
        })
    }
}

/// The glyph position error (M4).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlyphError {
    /// The reference lines whose glyphs were measured: lines whose breaks agree, on pages whose starts agree.
    pub lines: u64,
    /// The glyphs measured.
    pub glyphs: u64,
    /// The horizontal distances, if any glyph was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Spread>,
    /// The vertical distances, if any glyph was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Spread>,
}

/// The metrics M1 to M4 of a document, with the agreement of its text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Comparison {
    /// How much of the text the alignment matched.
    pub text: TextAgreement,
    /// M1, the page counts.
    pub m1: PageCount,
    /// M2, page-break agreement.
    pub m2: Breaks,
    /// M3, line-break agreement.
    pub m3: Breaks,
    /// M4, the glyph position error.
    pub m4: GlyphError,
}

/// A comparison with the histograms of its glyph errors, which groups of documents merge.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Measured {
    /// The metrics.
    pub comparison: Comparison,
    /// The horizontal distances of the glyphs measured, in thousandths of a point.
    pub x: Histogram,
    /// The vertical distances.
    pub y: Histogram,
}

/// M7: the agreement of the candidate's page starts with the `w:lastRenderedPageBreak` hints of the document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HintAgreement {
    /// The hints: how many fall at the start of a candidate page.
    pub hints: Breaks,
    /// The pages that the hints imply (one more than the hints) and the candidate's pages: an approximate M1.
    pub pages: PageCount,
}

/// Compares `reference` (Word's layout) with `candidate` (BayanDocs') within `limits`: M1 to M4.
///
/// # Errors
///
/// [`Error::TooLong`] if either text exceeds [`Limits::max_chars`].
pub fn compare(reference: &Layout, candidate: &Layout, limits: &Limits) -> Result<Measured, Error> {
    let streams = (
        Stream::new(reference, limits.max_chars).map_err(too_long("reference"))?,
        Stream::new(candidate, limits.max_chars).map_err(too_long("candidate"))?,
    );
    let (ours, theirs) = (&streams.0, &streams.1);
    let mut alignment = align(ours.chars(), theirs.chars(), &limits.align);
    // Every page start is a line start, so placing deletions by the line starts serves both.
    let (reference_lines, line_starts) = ours.line_starts();
    alignment.slide_deletions(ours.chars(), &line_starts);

    // M2: page starts.
    let (reference_pages, page_starts) = ours.page_starts();
    let (candidate_pages, candidate_page_starts) = theirs.page_starts();
    let page_matches = match_breaks(&alignment.images(&page_starts), &candidate_page_starts);
    let m2 = Breaks::new(&page_matches, |index| Place {
        page: count(reference_pages[index] + 1),
        line: None,
    });
    // The candidate page whose start agrees with each reference page's, if one does.
    let mut corresponding = vec![None; ours.pages().len()];
    for (index, matched) in page_matches.iter().enumerate() {
        if let Some(matched) = matched {
            corresponding[reference_pages[index]] = Some(candidate_pages[*matched]);
        }
    }

    // M3: line starts.
    let (candidate_lines, candidate_line_starts) = theirs.line_starts();
    let line_matches = match_breaks(&alignment.images(&line_starts), &candidate_line_starts);
    let m3 = Breaks::new(&line_matches, |index| {
        let line = &ours.lines()[reference_lines[index]];
        Place {
            page: count(line.page + 1),
            line: Some(count(line.lines.start + 1)),
        }
    });

    // M4: glyphs on lines whose breaks agree, on pages whose starts agree.
    let ends: Vec<usize> = reference_lines
        .iter()
        .map(|&line| ours.lines()[line].span.end)
        .collect();
    let end_images = alignment.images(&ends);
    let mut measured = Measured::default();
    for (index, matched) in line_matches.iter().enumerate() {
        let Some(matched) = matched else {
            continue;
        };
        let line = &ours.lines()[reference_lines[index]];
        let other = &theirs.lines()[candidate_lines[*matched]];
        if !end_images[index].contains(other.span.end)
            || corresponding[line.page] != Some(other.page)
        {
            continue;
        }
        measured.comparison.m4.lines += 1;
        measured.comparison.m4.glyphs += measure_line(
            (reference, ours, line),
            (candidate, theirs, other),
            &alignment,
            &mut measured.x,
            &mut measured.y,
        );
    }
    measured.comparison.m4.x = Spread::of(&measured.x);
    measured.comparison.m4.y = Spread::of(&measured.y);
    measured.comparison.text = TextAgreement {
        reference: as_u64(ours.chars().len()),
        candidate: as_u64(theirs.chars().len()),
        matched: as_u64(alignment.matched()),
        complete: alignment.complete(),
    };
    measured.comparison.m1 = PageCount {
        reference: reference.page_count,
        candidate: candidate.page_count,
    };
    measured.comparison.m2 = m2;
    measured.comparison.m3 = m3;
    Ok(measured)
}

/// M7: aligns the document's text, as [`crate::hints`] reads it, with the candidate's, and checks for each `w:lastRenderedPageBreak` hint whether a candidate page starts at its image.
///
/// # Errors
///
/// [`Error::TooLong`] if either text exceeds [`Limits::max_chars`].
pub fn hint_agreement(
    hints: &Hints,
    candidate: &Layout,
    limits: &Limits,
) -> Result<HintAgreement, Error> {
    if hints.chars.len() > limits.max_chars {
        return Err(Error::TooLong {
            side: "hints",
            limit: limits.max_chars,
        });
    }
    let theirs = Stream::new(candidate, limits.max_chars).map_err(too_long("candidate"))?;
    let mut alignment = align(&hints.chars, theirs.chars(), &limits.align);
    alignment.slide_deletions(&hints.chars, &hints.breaks);
    let (_, candidate_page_starts) = theirs.page_starts();
    let matches = match_breaks(&alignment.images(&hints.breaks), &candidate_page_starts);
    // Hint i begins page i + 2 of Word's last layout.
    let breaks = Breaks::new(&matches, |index| Place {
        page: count(index + 2),
        line: None,
    });
    Ok(HintAgreement {
        hints: breaks,
        pages: PageCount {
            reference: count(hints.breaks.len() + 1),
            candidate: candidate.page_count,
        },
    })
}

fn too_long(side: &'static str) -> impl Fn(TooLong) -> Error {
    move |error| Error::TooLong {
        side,
        limit: error.limit,
    }
}

fn as_u64(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// A glyph that M4 can measure: one that shows characters of the comparable form.
struct Mark {
    x: Points,
    y: Points,
    text: Vec<char>,
}

/// The measurable glyphs of a joined line, sorted by the x of their origins (glyphs at the same x in their order).
fn marks(layout: &Layout, line: &StreamLine) -> Vec<Mark> {
    let mut marks = Vec::new();
    for layout_line in &layout.pages[line.page].lines[line.lines.clone()] {
        let (Some(text), Some(glyphs)) = (&layout_line.text, &layout_line.glyphs) else {
            continue;
        };
        let characters: Vec<char> = text.chars().collect();
        for glyph in glyphs {
            let shown = shown(&characters, glyph.offset, glyph.length);
            let text: Vec<char> = comparable_chars(&shown).map(|(_, c)| c).collect();
            if !text.is_empty() {
                marks.push(Mark {
                    x: glyph.x,
                    y: glyph.y,
                    text,
                });
            }
        }
    }
    marks.sort_by_key(|mark| mark.x);
    marks
}

/// The characters a glyph shows.
fn shown(characters: &[char], offset: u32, length: u32) -> String {
    let start = usize::try_from(offset).unwrap_or(usize::MAX);
    let end = start.saturating_add(usize::try_from(length).unwrap_or(usize::MAX));
    characters
        .get(start..end.min(characters.len()))
        .unwrap_or_default()
        .iter()
        .collect()
}

/// The glyphs of a joined line grouped into clusters (glyphs that show the same characters), with a lookup from each character to its cluster.
struct Clusters {
    /// For each layout line of the joined line, in order: the cluster of each character of its text, if a glyph shows it.
    lookup: Vec<Vec<Option<usize>>>,
    /// Each cluster's glyph origins, in display order.
    glyphs: Vec<Vec<(Points, Points)>>,
    /// Each cluster's characters in the comparable form, and how many of them the alignment paired with characters of one cluster of the other line.
    characters: Vec<(u64, u64)>,
    /// The cluster of the other line that each cluster's characters are paired with: `None` before any, `Some(None)` when they go to more than one or to text without a glyph.
    partner: Vec<Option<Option<usize>>>,
}

impl Clusters {
    fn new(layout: &Layout, line: &StreamLine) -> Self {
        let mut clusters = Self {
            lookup: Vec::new(),
            glyphs: Vec::new(),
            characters: Vec::new(),
            partner: Vec::new(),
        };
        for layout_line in &layout.pages[line.page].lines[line.lines.clone()] {
            let length = layout_line
                .text
                .as_ref()
                .map_or(0, |text| text.chars().count());
            let mut lookup = vec![None; length];
            let mut by_range = std::collections::BTreeMap::new();
            for glyph in layout_line.glyphs.iter().flatten() {
                if glyph.length > 0 {
                    by_range
                        .entry((glyph.offset, glyph.length))
                        .or_insert_with(Vec::new)
                        .push((glyph.x, glyph.y));
                }
            }
            for ((offset, glyph_length), origins) in by_range {
                let cluster = clusters.glyphs.len();
                clusters.glyphs.push(origins);
                clusters.characters.push((0, 0));
                clusters.partner.push(None);
                let start = usize::try_from(offset).unwrap_or(usize::MAX);
                let end = start.saturating_add(usize::try_from(glyph_length).unwrap_or(0));
                for entry in lookup.iter_mut().take(end).skip(start) {
                    *entry = Some(cluster);
                }
            }
            clusters.lookup.push(lookup);
        }
        clusters
    }

    /// The cluster of the character at `position` of `stream`, on `line`.
    fn at(&self, stream: &Stream, line: &StreamLine, position: usize) -> Option<usize> {
        let origin = stream.origin(position)?;
        let relative = usize::try_from(origin.line)
            .ok()?
            .checked_sub(line.lines.start)?;
        let index = usize::try_from(origin.index).ok()?;
        *self.lookup.get(relative)?.get(index)?
    }

    /// Records that a character of `cluster` (or of text without a glyph) is paired with one of `other`.
    fn pair(&mut self, cluster: Option<usize>, other: Option<usize>) {
        let Some(cluster) = cluster else {
            return;
        };
        let partner = &mut self.partner[cluster];
        *partner = match (*partner, other) {
            (None, other) => Some(other),
            (Some(Some(known)), Some(other)) if known == other => Some(Some(known)),
            _ => Some(None),
        };
        self.characters[cluster].1 += 1;
    }

    /// Whether every character of `cluster` is paired with a character of `other`, and only of it.
    fn complete_with(&self, cluster: usize, other: usize) -> bool {
        let (total, paired) = self.characters[cluster];
        total > 0 && total == paired && self.partner[cluster] == Some(Some(other))
    }
}

/// Measures the glyphs of a reference line and a candidate line whose breaks agree, adding their distances to `x` and `y`; returns how many glyphs it measured.
fn measure_line(
    (reference, ours, line): (&Layout, &Stream, &StreamLine),
    (candidate, theirs, other): (&Layout, &Stream, &StreamLine),
    alignment: &Alignment,
    x: &mut Histogram,
    y: &mut Histogram,
) -> u64 {
    let mut measured = 0;
    let mut add = |(x0, y0): (Points, Points), (x1, y1): (Points, Points)| {
        x.add((x0.millis() - x1.millis()).unsigned_abs());
        y.add((y0.millis() - y1.millis()).unsigned_abs());
        measured += 1;
    };
    // Rule 1: the same glyphs from left to right.
    let (left, right) = (marks(reference, line), marks(candidate, other));
    if left.len() == right.len() && left.iter().zip(&right).all(|(a, b)| a.text == b.text) {
        for (a, b) in left.iter().zip(&right) {
            add((a.x, a.y), (b.x, b.y));
        }
        return measured;
    }
    // Rule 2: clusters paired through the alignment.
    let mut ours_clusters = Clusters::new(reference, line);
    let mut theirs_clusters = Clusters::new(candidate, other);
    for position in line.span.clone() {
        if let Some(cluster) = ours_clusters.at(ours, line, position) {
            ours_clusters.characters[cluster].0 += 1;
        }
    }
    for position in other.span.clone() {
        if let Some(cluster) = theirs_clusters.at(theirs, other, position) {
            theirs_clusters.characters[cluster].0 += 1;
        }
    }
    for (position, partner) in alignment.partners(line.span.clone()) {
        let mine = ours_clusters.at(ours, line, position);
        let yours = if other.span.contains(&partner) {
            theirs_clusters.at(theirs, other, partner)
        } else {
            None
        };
        ours_clusters.pair(mine, yours);
        if other.span.contains(&partner) {
            theirs_clusters.pair(yours, mine);
        }
    }
    for cluster in 0..ours_clusters.glyphs.len() {
        let Some(Some(partner)) = ours_clusters.partner[cluster] else {
            continue;
        };
        if ours_clusters.complete_with(cluster, partner)
            && theirs_clusters.complete_with(partner, cluster)
            && ours_clusters.glyphs[cluster].len() == theirs_clusters.glyphs[partner].len()
        {
            for (&a, &b) in ours_clusters.glyphs[cluster]
                .iter()
                .zip(&theirs_clusters.glyphs[partner])
            {
                add(a, b);
            }
        }
    }
    measured
}
