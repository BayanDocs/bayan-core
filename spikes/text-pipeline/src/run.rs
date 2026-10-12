//! Running the probe set: each probe's layout JSON and rasters, their hashes, and the results file of one environment, which the comparison job compares across environments.

use serde::Serialize;

use crate::fonts::FontBook;
use crate::hash::{hex, sha256};
use crate::layout::{Layout, Paragraph, lay_out};
use crate::probes::Probe;
use crate::raster::{
    Blending, Difference, Image, Vello, compare, render_reference, render_vello, scene,
};

/// The format of the results file; change it whenever a field changes meaning.
pub const FORMAT: &str = "bayan-core CORE-003 text-pipeline results v1";

/// What one probe produced on this platform.
#[derive(Clone, Debug, Serialize)]
pub struct ProbeResult {
    /// The probe's name.
    pub id: String,
    /// The number of lines.
    pub lines: usize,
    /// The number of glyphs.
    pub glyphs: usize,
    /// The glyphs that are glyph 0 (`.notdef`): characters none of the probe's fonts has.
    pub missing_glyphs: usize,
    /// The SHA-256 of the layout JSON: identical on every platform, or the spike has failed (AC-1).
    pub layout_sha256: String,
    /// The width of the rasters in pixels.
    pub width: u32,
    /// The height of the rasters in pixels.
    pub height: u32,
    /// The raster hash of the reference rendering, tiny-skia with linear blending: identical on every platform (AC-2).
    pub reference_sha256: String,
    /// The raster hash of tiny-skia with gamma-2 blending, the alternative REPORT.md §6 weighs.
    pub gamma2_sha256: String,
    /// vello_cpu's renderings, compared with the reference (AC-3).
    pub vello: Vec<VelloResult>,
}

/// One vello_cpu rendering of a probe.
#[derive(Clone, Debug, Serialize)]
pub struct VelloResult {
    /// The configuration ([`Vello::name`]).
    pub configuration: String,
    /// The SIMD level it ran at on this machine.
    pub level: String,
    /// Its raster hash.
    pub sha256: String,
    /// How it differs from the reference.
    pub difference: DifferenceResult,
}

/// [`Difference`], as the results file writes it.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct DifferenceResult {
    /// The number of pixels.
    pub pixels: u64,
    /// The pixels where any channel differs.
    pub differing: u64,
    /// The pixels where a channel differs by more than 2.
    pub beyond_2: u64,
    /// The pixels where a channel differs by more than 64.
    pub beyond_64: u64,
    /// The largest difference of any channel.
    pub max: u8,
}

impl From<Difference> for DifferenceResult {
    fn from(difference: Difference) -> DifferenceResult {
        DifferenceResult {
            pixels: difference.pixels,
            differing: difference.differing,
            beyond_2: difference.beyond_2,
            beyond_64: difference.beyond_64,
            max: difference.max,
        }
    }
}

/// Everything one probe produced: its result, its layout JSON and its images, each with a name for its file.
pub struct ProbeOutput {
    /// The hashes and counts.
    pub result: ProbeResult,
    /// The layout JSON.
    pub layout_json: Vec<u8>,
    /// The images: `reference`, `gamma2` and one per vello_cpu configuration.
    pub images: Vec<(String, Image)>,
}

/// What the viewport of the rasterization measurement produced on this platform: a large scene whose last paragraph is cut off at the image's bottom edge, which takes tiny-skia's edge clipping through its curve intersections.
#[derive(Clone, Debug, Serialize)]
pub struct ViewportResult {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The number of paragraphs drawn.
    pub paragraphs: usize,
    /// The number of glyphs drawn.
    pub glyphs: usize,
    /// The SHA-256 of the paragraphs' layout JSON, one after another.
    pub layouts_sha256: String,
    /// The raster hash of the reference rendering.
    pub reference_sha256: String,
    /// The raster hash with gamma-2 blending.
    pub gamma2_sha256: String,
    /// vello_cpu's renderings, compared with the reference.
    pub vello: Vec<VelloResult>,
}

/// Lays out and rasterizes a viewport of `width` × `height` pixels from `probes` (the results use the measurement's, [`crate::bench::VIEWPORT_WIDTH`] × [`crate::bench::VIEWPORT_HEIGHT`]), and returns its result and its images, named as [`run_probe`] names them.
///
/// # Errors
///
/// A message if a font is missing, or if layout or rasterization fails.
pub fn run_viewport(
    book: &FontBook,
    probes: &[&'static Probe],
    width: u32,
    height: u32,
) -> Result<(ViewportResult, Vec<(String, Image)>), String> {
    let (scene, layouts) = crate::bench::viewport(book, probes, width, height)?;
    let mut json = Vec::new();
    for layout in &layouts {
        json.extend(layout.to_json()?);
    }
    let renderings = render_all(&scene)?;
    Ok((
        ViewportResult {
            width: scene.width,
            height: scene.height,
            paragraphs: layouts.len(),
            glyphs: scene.paths.len(),
            layouts_sha256: hex(&sha256(&json)),
            reference_sha256: hex(&renderings.images[0].1.sha256()),
            gamma2_sha256: hex(&renderings.images[1].1.sha256()),
            vello: renderings.vello,
        },
        renderings.images,
    ))
}

/// Every rendering of a scene: the images, and vello_cpu's results.
struct Renderings {
    images: Vec<(String, Image)>,
    vello: Vec<VelloResult>,
}

/// Renders `scene` as the reference, with gamma-2 blending, and with every vello_cpu configuration, comparing those with the reference.
fn render_all(scene: &crate::raster::Scene) -> Result<Renderings, String> {
    let reference = render_reference(scene, Blending::Linear)?;
    let gamma2 = render_reference(scene, Blending::Gamma2)?;
    let mut vello = Vec::with_capacity(Vello::ALL.len());
    let mut images = vec![
        ("reference".to_owned(), reference),
        ("gamma2".to_owned(), gamma2),
    ];
    for configuration in Vello::ALL {
        let (image, level) = render_vello(scene, configuration)?;
        vello.push(VelloResult {
            configuration: configuration.name().to_owned(),
            level,
            sha256: hex(&image.sha256()),
            difference: compare(&images[0].1, &image)?.into(),
        });
        images.push((configuration.name().to_owned(), image));
    }
    Ok(Renderings { images, vello })
}

/// Lays out `probe` with the fonts of `book`.
///
/// # Errors
///
/// A message if a font of the probe is missing from the book, or if layout fails.
pub fn lay_out_probe(book: &FontBook, probe: &Probe) -> Result<Layout, String> {
    let fonts = probe
        .fonts
        .iter()
        .map(|&file| {
            book.index(file)
                .ok_or_else(|| format!("{}: the font {file} was not loaded", probe.id))
        })
        .collect::<Result<Vec<usize>, String>>()?;
    lay_out(
        book,
        &Paragraph {
            text: probe.text,
            rtl: probe.rtl,
            fonts: &fonts,
            size: probe.size(),
            measure: probe.measure(),
            language: probe.language,
            features: probe.features,
        },
    )
    .map_err(|error| format!("{}: {error}", probe.id))
}

/// Lays out and rasterizes `probe` with the fonts of `book`.
///
/// # Errors
///
/// A message if a font of the probe is missing from the book, or if layout or rasterization fails.
pub fn run_probe(book: &FontBook, probe: &Probe) -> Result<ProbeOutput, String> {
    let layout = lay_out_probe(book, probe)?;
    let layout_json = layout.to_json()?;
    let glyphs = || {
        layout
            .lines
            .iter()
            .flat_map(|line| &line.runs)
            .flat_map(|run| &run.glyphs)
    };
    let scene = scene(book, &layout).map_err(|error| format!("{}: {error}", probe.id))?;
    let Renderings { images, vello } = render_all(&scene)?;
    Ok(ProbeOutput {
        result: ProbeResult {
            id: probe.id.to_owned(),
            lines: layout.lines.len(),
            glyphs: glyphs().count(),
            missing_glyphs: glyphs().filter(|glyph| glyph.id == 0).count(),
            layout_sha256: hex(&sha256(&layout_json)),
            width: scene.width,
            height: scene.height,
            reference_sha256: hex(&images[0].1.sha256()),
            gamma2_sha256: hex(&images[1].1.sha256()),
            vello,
        },
        layout_json,
        images,
    })
}

/// The results of a whole run in one environment.
#[derive(Clone, Debug, Serialize)]
pub struct Results {
    /// [`FORMAT`].
    pub format: String,
    /// Where the run happened, as the runner names it (such as `linux-x86_64` or `wasm32-chromium`).
    pub environment: String,
    /// The fonts, with the SHA-256 of each file.
    pub fonts: Vec<FontResult>,
    /// The SHA-256 of every probe's name and layout hash, one line each: one value to compare across environments for AC-1.
    pub layouts_sha256: String,
    /// The same for the reference raster hashes (AC-2).
    pub references_sha256: String,
    /// The probes, in the order of the probe set.
    pub probes: Vec<ProbeResult>,
    /// The viewport of the rasterization measurement, when every font was loaded.
    pub viewport: Option<ViewportResult>,
}

/// A font of a run.
#[derive(Clone, Debug, Serialize)]
pub struct FontResult {
    /// The file name.
    pub file: String,
    /// The SHA-256 of the file.
    pub sha256: String,
}

impl Results {
    /// The results of `probes` from `book` in `environment`.
    #[must_use]
    pub fn new(
        environment: &str,
        book: &FontBook,
        probes: Vec<ProbeResult>,
        viewport: Option<ViewportResult>,
    ) -> Results {
        let summary = |hash: fn(&ProbeResult) -> &str| {
            let mut lines = String::new();
            for probe in &probes {
                lines.push_str(&probe.id);
                lines.push(' ');
                lines.push_str(hash(probe));
                lines.push('\n');
            }
            hex(&sha256(lines.as_bytes()))
        };
        Results {
            format: FORMAT.to_owned(),
            environment: environment.to_owned(),
            fonts: book
                .fonts()
                .iter()
                .map(|font| FontResult {
                    file: font.file().to_owned(),
                    sha256: hex(font.sha256()),
                })
                .collect(),
            layouts_sha256: summary(|probe| &probe.layout_sha256),
            references_sha256: summary(|probe| &probe.reference_sha256),
            probes,
            viewport,
        }
    }

    /// The results file: pretty JSON, ending with a line feed.
    ///
    /// # Errors
    ///
    /// A message if serialization fails, which it cannot for these types.
    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        let mut json = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        json.push(b'\n');
        Ok(json)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::tests::committed_book;
    use crate::probes::PROBES;

    fn committed_probes() -> Vec<&'static Probe> {
        PROBES.iter().filter(|probe| probe.committed()).collect()
    }

    #[test]
    fn runs_a_probe_into_hashes_and_images() {
        let book = committed_book();
        for probe in committed_probes() {
            let output = run_probe(&book, probe).unwrap();
            let result = &output.result;
            assert_eq!(result.id, probe.id);
            assert!(result.lines >= 2, "{}: {} lines", probe.id, result.lines);
            assert!(result.glyphs > 20, "{}", probe.id);
            assert_eq!(result.missing_glyphs, 0, "{}", probe.id);
            assert_eq!(result.layout_sha256, hex(&sha256(&output.layout_json)));
            let names: Vec<&str> = output
                .images
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            assert_eq!(
                names,
                [
                    "reference",
                    "gamma2",
                    "vello_u8_detected",
                    "vello_u8_fallback",
                    "vello_f32_fallback"
                ]
            );
            assert_eq!(result.reference_sha256, hex(&output.images[0].1.sha256()));
            assert_eq!(result.gamma2_sha256, hex(&output.images[1].1.sha256()));
            assert_eq!(result.vello.len(), 3);
            for (vello, (_, image)) in result.vello.iter().zip(&output.images[2..]) {
                assert_eq!(vello.sha256, hex(&image.sha256()));
                assert_eq!(
                    vello.difference.pixels,
                    u64::from(result.width * result.height)
                );
            }
        }
    }

    #[test]
    fn summarizes_a_run_in_two_hashes() {
        let book = committed_book();
        let probes: Vec<ProbeResult> = committed_probes()
            .into_iter()
            .map(|probe| run_probe(&book, probe).unwrap().result)
            .collect();
        let results = Results::new("test", &book, probes.clone(), None);
        assert_eq!(results.format, FORMAT);
        assert_eq!(results.environment, "test");
        assert_eq!(results.fonts.len(), 2);
        assert_eq!(results.fonts[0].sha256, hex(book.fonts()[0].sha256()));
        let mut lines = String::new();
        for probe in &probes {
            lines.push_str(&format!("{} {}\n", probe.id, probe.layout_sha256));
        }
        assert_eq!(results.layouts_sha256, hex(&sha256(lines.as_bytes())));
        assert_ne!(results.references_sha256, results.layouts_sha256);
        // The same run gives the same summary.
        assert_eq!(
            results.references_sha256,
            Results::new("other", &book, probes, None).references_sha256
        );
        assert!(results.to_json().unwrap().ends_with(b"}\n"));
    }
}

#[cfg(test)]
mod viewport_tests {
    use super::*;
    use crate::fonts::tests::committed_book;
    use crate::probes::PROBES;

    #[test]
    fn hashes_the_viewport_and_its_renderings() {
        let book = committed_book();
        let probes: Vec<&'static Probe> = PROBES.iter().filter(|probe| probe.committed()).collect();
        let (result, images) = run_viewport(&book, &probes, 640, 900).unwrap();
        assert_eq!((result.width, result.height), (640, 900));
        assert!(result.paragraphs > probes.len(), "{}", result.paragraphs);
        let (_, layouts) = crate::bench::viewport(&book, &probes, 640, 900).unwrap();
        let mut json = Vec::new();
        for layout in &layouts {
            json.extend(layout.to_json().unwrap());
        }
        assert_eq!(result.layouts_sha256, hex(&sha256(&json)));
        assert_eq!(result.paragraphs, layouts.len());
        let names: Vec<&str> = images.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "reference",
                "gamma2",
                "vello_u8_detected",
                "vello_u8_fallback",
                "vello_f32_fallback"
            ]
        );
        assert_eq!(result.reference_sha256, hex(&images[0].1.sha256()));
        assert_eq!(result.gamma2_sha256, hex(&images[1].1.sha256()));
        assert_eq!(result.vello[2].sha256, hex(&images[4].1.sha256()));
    }
}
