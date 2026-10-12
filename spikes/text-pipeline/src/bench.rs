//! The work that the measurements of AC-3 time, the same natively and in WebAssembly. The hosts time it with their own clocks (the native runner with `std::time::Instant`, the JavaScript runners with `performance.now()`), so this module reads no clock (ADR-0005 §5).
//!
//! - **Shaping**: harfrust shaping every run of every probe, itemized ahead of time, so that only shaping is timed.
//! - **Layout**: the whole pipeline for every probe: bidi, itemization, segmentation, shaping, measurement, line breaking and reordering.
//! - **Rasterization**: a viewport of 1,920 × 1,080 CSS pixels at device scale 2, 3,840 × 2,160 device pixels, filled with the probe paragraphs laid out across its width, one below the other; the last one is cut off at the bottom edge.

use icu_properties::CodePointMapData;
use icu_properties::props::BidiClass;
use unicode_bidi::{BidiInfo, Level};

use crate::fonts::FontBook;
use crate::itemize::{Run, itemize};
use crate::layout::{Layout, Paragraph, lay_out};
use crate::probes::Probe;
use crate::raster::{BLU_PER_PIXEL, MARGIN, Scene, glyph_paths};

/// The width of the viewport in device pixels: 1,920 CSS pixels at device scale 2.
pub const VIEWPORT_WIDTH: u32 = 3_840;
/// The height of the viewport in device pixels: 1,080 CSS pixels at device scale 2.
pub const VIEWPORT_HEIGHT: u32 = 2_160;

/// A probe itemized ahead of time.
struct Prepared {
    probe: &'static Probe,
    fonts: Vec<usize>,
    runs: Vec<Run>,
}

/// The probes of a measurement, with their fonts found and their runs itemized.
pub struct Workload {
    prepared: Vec<Prepared>,
}

impl Workload {
    /// Prepares `probes`, whose fonts must be in `book`.
    ///
    /// # Errors
    ///
    /// A message if a font of a probe is missing from the book.
    pub fn new(book: &FontBook, probes: &[&'static Probe]) -> Result<Workload, String> {
        let data = CodePointMapData::<BidiClass>::new();
        let mut prepared = Vec::with_capacity(probes.len());
        for &probe in probes {
            let fonts = font_indices(book, probe)?;
            let base = if probe.rtl {
                Level::rtl()
            } else {
                Level::ltr()
            };
            let levels = BidiInfo::new_with_data_source(&data, probe.text, Some(base)).levels;
            let runs = itemize(probe.text, &levels, fonts.len(), &|font, c| {
                book.fonts()[fonts[font]].covers(c)
            });
            prepared.push(Prepared { probe, fonts, runs });
        }
        Ok(Workload { prepared })
    }

    /// The number of characters (Unicode scalar values) of the probes' texts: what one round of [`Workload::shape`] or [`Workload::lay_out`] processes.
    #[must_use]
    pub fn characters(&self) -> usize {
        self.prepared
            .iter()
            .map(|prepared| prepared.probe.text.chars().count())
            .sum()
    }

    /// Shapes every run of every probe once, and returns the number of glyphs.
    ///
    /// # Errors
    ///
    /// A message if shaping fails.
    pub fn shape(&self, book: &FontBook) -> Result<usize, String> {
        let mut glyphs = 0;
        for prepared in &self.prepared {
            let probe = prepared.probe;
            for run in &prepared.runs {
                glyphs += crate::shape::shape(
                    &book.fonts()[prepared.fonts[run.font]],
                    probe.text,
                    run.start..run.end,
                    run.level % 2 == 1,
                    run.script,
                    probe.language,
                    probe.features,
                )?
                .len();
            }
        }
        Ok(glyphs)
    }

    /// Lays out every probe once, and returns the number of lines.
    ///
    /// # Errors
    ///
    /// A message if a layout fails.
    pub fn lay_out(&self, book: &FontBook) -> Result<usize, String> {
        let mut lines = 0;
        for prepared in &self.prepared {
            lines += lay_out(
                book,
                &paragraph(prepared.probe, &prepared.fonts, prepared.probe.measure()),
            )?
            .lines
            .len();
        }
        Ok(lines)
    }
}

/// The viewport scene of `width` × `height` pixels (the measurement uses [`VIEWPORT_WIDTH`] × [`VIEWPORT_HEIGHT`]; tests use smaller ones), and the layouts it was drawn from, in order.
///
/// # Errors
///
/// A message if the viewport is no wider than its margins, if a probe's font is missing from `book`, or if a layout or its outlines fail.
pub fn viewport(
    book: &FontBook,
    probes: &[&'static Probe],
    width: u32,
    height: u32,
) -> Result<(Scene, Vec<Layout>), String> {
    let margin = i64::from(MARGIN) * BLU_PER_PIXEL;
    let inner = width
        .checked_sub(2 * MARGIN)
        .filter(|&inner| inner > 0)
        .ok_or("a viewport must be wider than its margins")?;
    let measure = bayan_units::Blu(i64::from(inner) * BLU_PER_PIXEL);
    let bottom = i64::from(height) * BLU_PER_PIXEL;
    let mut layouts = Vec::new();
    let mut paths = Vec::new();
    let mut top = margin;
    // Round after round of the probes, until a paragraph reaches past the bottom edge.
    'fill: loop {
        if probes.is_empty() {
            return Err("a viewport needs at least one probe".to_owned());
        }
        for &probe in probes {
            let fonts = font_indices(book, probe)?;
            let layout = lay_out(book, &paragraph(probe, &fonts, measure))?;
            paths.extend(glyph_paths(book, &layout, margin, top)?);
            top = top
                .checked_add(layout.height)
                .ok_or("the viewport overflows")?;
            layouts.push(layout);
            if top >= bottom {
                break 'fill;
            }
        }
    }
    Ok((
        Scene {
            width,
            height,
            paths,
        },
        layouts,
    ))
}

/// The indices in `book` of a probe's fonts.
fn font_indices(book: &FontBook, probe: &Probe) -> Result<Vec<usize>, String> {
    probe
        .fonts
        .iter()
        .map(|&file| {
            book.index(file)
                .ok_or_else(|| format!("{}: the font {file} was not loaded", probe.id))
        })
        .collect()
}

/// The paragraph of `probe` at the given measure.
fn paragraph<'a>(probe: &'a Probe, fonts: &'a [usize], measure: bayan_units::Blu) -> Paragraph<'a> {
    Paragraph {
        text: probe.text,
        rtl: probe.rtl,
        fonts,
        size: probe.size(),
        measure,
        language: probe.language,
        features: probe.features,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::tests::committed_book;
    use crate::probes::PROBES;

    fn committed() -> Vec<&'static Probe> {
        PROBES.iter().filter(|probe| probe.committed()).collect()
    }

    #[test]
    fn counts_the_characters_and_shapes_and_lays_out_every_probe() {
        let book = committed_book();
        let probes = committed();
        let workload = Workload::new(&book, &probes).unwrap();
        let characters: usize = probes.iter().map(|probe| probe.text.chars().count()).sum();
        assert_eq!(workload.characters(), characters);
        let glyphs = workload.shape(&book).unwrap();
        // Thai and Hebrew marks make more glyphs than letters, never fewer than half the characters.
        assert!(
            glyphs > characters / 2,
            "{glyphs} glyphs for {characters} characters"
        );
        let lines: usize = probes
            .iter()
            .map(|probe| {
                crate::run::run_probe(&book, probe, &mut |_, _| Ok(()))
                    .unwrap()
                    .result
                    .lines
            })
            .sum();
        assert_eq!(workload.lay_out(&book).unwrap(), lines);
    }

    #[test]
    fn fills_the_viewport_and_cuts_the_last_paragraph_off() {
        let book = committed_book();
        // A small viewport keeps the debug-build test fast; the measurement's size only changes how many paragraphs fit.
        let (width, height) = (640, 900);
        let (scene, layouts) = viewport(&book, &committed(), width, height).unwrap();
        assert_eq!((scene.width, scene.height), (width, height));
        assert!(layouts.len() > committed().len(), "{}", layouts.len());
        // Paragraphs follow each other from the top margin until one reaches past the bottom edge.
        let margin = i64::from(MARGIN) * BLU_PER_PIXEL;
        let bottom = i64::from(height) * BLU_PER_PIXEL;
        let total: i64 = layouts.iter().map(|layout| layout.height).sum();
        assert!(margin + total - layouts.last().unwrap().height < bottom);
        assert!(margin + total >= bottom);
        // Every paragraph is as wide as the viewport less its margins.
        let measure = i64::from(width - 2 * MARGIN) * BLU_PER_PIXEL;
        assert!(layouts.iter().all(|layout| layout.measure == measure));
        assert!(viewport(&book, &committed(), 2 * MARGIN, height).is_err());
        let glyphs: usize = layouts
            .iter()
            .flat_map(|layout| &layout.lines)
            .flat_map(|line| &line.runs)
            .map(|run| run.glyphs.len())
            .sum();
        assert_eq!(scene.paths.len(), glyphs);
    }
}
