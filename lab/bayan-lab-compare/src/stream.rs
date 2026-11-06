//! A layout's text as one stream of characters in the comparable form, with every page and line start in it: what [`crate::align`] aligns and where breaks are compared.
//!
//! The stream is the comparable form ([`bayan_lab_layout::text::comparable`]) of every line's text, page after page and line after line, without separators: the comparable form removes white space anyway, and a break is a position in the stream, not a character. Each character remembers where it comes from (page, line and character of the line's text), so that a glyph can be found for it.
//!
//! ## Joined lines
//!
//! A PDF has no lines, so the extractor makes one of all the glyphs it draws on one baseline in one go: a table row whose cells each hold one line, or text on both sides of a tab, is one line in Word's layout, where the engine has a line for each cell (`lab/schema/layout-json.md`, "Lines from a PDF"). Before breaks are compared, both sides' lines are therefore joined the same way: consecutive lines of a page join when their baselines lie within half an em of each other, the em of the larger of the line's largest glyph and the largest glyph of the joined line so far, whose baseline is the joined line's reference. That is the extractor's rule for glyphs, applied to lines. Lines without a baseline or without glyphs join nothing.
//!
//! A joined line whose text has no character in the comparable form (only spaces, say) has no position in the stream and takes no part in the comparison of line breaks; a page without such a line takes no part in the comparison of page breaks.

use core::ops::Range;

use bayan_lab_layout::layout::{Layout, Line, Points};
use bayan_lab_layout::text::comparable_chars;

/// Where a character of the stream comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    /// The page, counted from 0.
    pub page: u32,
    /// The line of the layout on that page, counted from 0.
    pub line: u32,
    /// The character of that line's text, counted from 0 in characters, as glyph offsets are.
    pub index: u32,
}

/// A joined line: consecutive lines of a page on one baseline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamLine {
    /// Its page, counted from 0.
    pub page: usize,
    /// The lines of the layout it joins, on its page.
    pub lines: Range<usize>,
    /// Its characters in the stream.
    pub span: Range<usize>,
}

/// A page of the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamPage {
    /// Its joined lines, as indices into [`Stream::lines`].
    pub lines: Range<usize>,
    /// Its characters in the stream.
    pub span: Range<usize>,
}

/// The text of a layout as one stream (see the module's documentation).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream {
    chars: Vec<char>,
    origins: Vec<Origin>,
    lines: Vec<StreamLine>,
    pages: Vec<StreamPage>,
}

/// A stream with more characters than the comparison allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLong {
    /// The limit, in characters.
    pub limit: usize,
}

impl Stream {
    /// The stream of `layout`, with at most `max_chars` characters.
    ///
    /// # Errors
    ///
    /// [`TooLong`] if the layout's text has more characters in the comparable form than `max_chars`.
    pub fn new(layout: &Layout, max_chars: usize) -> Result<Self, TooLong> {
        let mut stream = Self {
            chars: Vec::new(),
            origins: Vec::new(),
            lines: Vec::new(),
            pages: Vec::new(),
        };
        for (page_index, page) in layout.pages.iter().enumerate() {
            let first_line = stream.lines.len();
            let page_start = stream.chars.len();
            for lines in joined(&page.lines) {
                let start = stream.chars.len();
                for line_index in lines.clone() {
                    let Some(text) = &page.lines[line_index].text else {
                        continue;
                    };
                    for (index, character) in comparable_chars(text) {
                        if stream.chars.len() >= max_chars {
                            return Err(TooLong { limit: max_chars });
                        }
                        stream.chars.push(character);
                        stream.origins.push(Origin {
                            page: count(page_index),
                            line: count(line_index),
                            index: count(index),
                        });
                    }
                }
                stream.lines.push(StreamLine {
                    page: page_index,
                    lines,
                    span: start..stream.chars.len(),
                });
            }
            stream.pages.push(StreamPage {
                lines: first_line..stream.lines.len(),
                span: page_start..stream.chars.len(),
            });
        }
        Ok(stream)
    }

    /// The characters, in the comparable form.
    #[must_use]
    pub fn chars(&self) -> &[char] {
        &self.chars
    }

    /// Where the character at `position` comes from.
    #[must_use]
    pub fn origin(&self, position: usize) -> Option<Origin> {
        self.origins.get(position).copied()
    }

    /// The joined lines, page after page.
    #[must_use]
    pub fn lines(&self) -> &[StreamLine] {
        &self.lines
    }

    /// The pages.
    #[must_use]
    pub fn pages(&self) -> &[StreamPage] {
        &self.pages
    }

    /// The joined lines that have characters, as indices into [`Stream::lines`], with the positions where they start: strictly ascending.
    #[must_use]
    pub fn line_starts(&self) -> (Vec<usize>, Vec<usize>) {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.span.is_empty())
            .map(|(index, line)| (index, line.span.start))
            .unzip()
    }

    /// The pages that have characters, as indices into [`Stream::pages`], with the positions where they start: strictly ascending.
    #[must_use]
    pub fn page_starts(&self) -> (Vec<usize>, Vec<usize>) {
        self.pages
            .iter()
            .enumerate()
            .filter(|(_, page)| !page.span.is_empty())
            .map(|(index, page)| (index, page.span.start))
            .unzip()
    }
}

/// `value` as a `u32`, which every page, line and character index of a layout within the comparison's limits is.
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The size of a line's largest glyph, if it has glyphs.
fn largest_size(line: &Line) -> Option<Points> {
    line.glyphs.as_ref()?.iter().map(|glyph| glyph.size).max()
}

/// The lines of a page grouped into joined lines (see the module's documentation), as ranges of line indices.
#[must_use]
pub fn joined(lines: &[Line]) -> Vec<Range<usize>> {
    let mut groups: Vec<Range<usize>> = Vec::new();
    // The baseline and size of the current group's line with the largest glyph (the first of equal ones).
    let mut reference: Option<(Points, Points)> = None;
    for (index, line) in lines.iter().enumerate() {
        let own = line.baseline.zip(largest_size(line));
        let joins = match (own, reference) {
            (Some((baseline, size)), Some((group_baseline, group_size))) => {
                let em = if size > group_size { size } else { group_size };
                2 * (baseline.millis() - group_baseline.millis()).abs() <= em.millis()
            }
            _ => false,
        };
        match groups.last_mut() {
            Some(group) if joins => {
                group.end = index + 1;
                if let (Some((baseline, size)), Some((_, group_size))) = (own, reference)
                    && size > group_size
                {
                    reference = Some((baseline, size));
                }
            }
            _ => {
                groups.push(index..index + 1);
                reference = own;
            }
        }
    }
    groups
}
