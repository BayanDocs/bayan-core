//! Running the probe set: each probe's layout JSON and rasters, their hashes, and the results file of one environment, which the comparison job compares across environments.

use serde::Serialize;

use crate::fonts::FontBook;
use crate::hash::{hex, sha256};
use crate::layout::{Layout, lay_out};
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

/// What one probe produced besides its images, which [`run_probe`] hands over as it makes them.
pub struct ProbeOutput {
    /// The hashes and counts.
    pub result: ProbeResult,
    /// The layout JSON.
    pub layout_json: Vec<u8>,
}

/// Receives each image of a scene as soon as it is drawn, with its name (`reference`, `gamma2` and then each [`Vello::name`], in that order), to write it out before the next one is drawn: at the size of the viewport each image is 31.6 MiB. An error stops the run and is returned.
pub type Keep<'a> = dyn FnMut(&str, &Image) -> Result<(), String> + 'a;

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

/// Lays out and rasterizes a viewport of `width` × `height` pixels from `probes` (the results use the measurement's, [`crate::bench::VIEWPORT_WIDTH`] × [`crate::bench::VIEWPORT_HEIGHT`]), hands each of its images to `keep` as [`run_probe`] does, and returns its result.
///
/// # Errors
///
/// A message if a font is missing, if layout or rasterization fails, or what `keep` returns.
pub fn run_viewport(
    book: &FontBook,
    probes: &[&'static Probe],
    width: u32,
    height: u32,
    keep: &mut Keep<'_>,
) -> Result<ViewportResult, String> {
    let (scene, layouts) = crate::bench::viewport(book, probes, width, height)?;
    let mut json = Vec::new();
    for layout in &layouts {
        json.extend(layout.to_json()?);
    }
    let renderings = render_all(&scene, keep)?;
    Ok(ViewportResult {
        width: scene.width,
        height: scene.height,
        paragraphs: layouts.len(),
        glyphs: scene.paths.len(),
        layouts_sha256: hex(&sha256(&json)),
        reference_sha256: renderings.reference_sha256,
        gamma2_sha256: renderings.gamma2_sha256,
        vello: renderings.vello,
    })
}

/// The raster hashes of every rendering of a scene, and vello_cpu's results.
struct Renderings {
    reference_sha256: String,
    gamma2_sha256: String,
    vello: Vec<VelloResult>,
}

/// Renders `scene` as the reference, with gamma-2 blending, and with every vello_cpu configuration, comparing those with the reference, and hands each image to `keep` as soon as it is drawn. Only the reference stays in memory until the comparisons are done, so at most two images exist at a time.
fn render_all(scene: &crate::raster::Scene, keep: &mut Keep<'_>) -> Result<Renderings, String> {
    let reference = render_reference(scene, Blending::Linear)?;
    keep("reference", &reference)?;
    let gamma2 = render_reference(scene, Blending::Gamma2)?;
    keep("gamma2", &gamma2)?;
    let gamma2_sha256 = hex(&gamma2.sha256());
    drop(gamma2);
    let mut vello = Vec::with_capacity(Vello::ALL.len());
    for configuration in Vello::ALL {
        let (image, level) = render_vello(scene, configuration)?;
        keep(configuration.name(), &image)?;
        vello.push(VelloResult {
            configuration: configuration.name().to_owned(),
            level,
            sha256: hex(&image.sha256()),
            difference: compare(&reference, &image)?.into(),
        });
    }
    Ok(Renderings {
        reference_sha256: hex(&reference.sha256()),
        gamma2_sha256,
        vello,
    })
}

/// Lays out `probe` with the fonts of `book`.
///
/// # Errors
///
/// A message if a font of the probe is missing from the book, or if layout fails.
pub fn lay_out_probe(book: &FontBook, probe: &Probe) -> Result<Layout, String> {
    let fonts = probe.font_indices(book)?;
    lay_out(book, &probe.paragraph(&fonts, probe.measure()))
        .map_err(|error| format!("{}: {error}", probe.id))
}

/// Lays out and rasterizes `probe` with the fonts of `book`, and hands each of its images to `keep` as soon as it is drawn ([`Keep`]).
///
/// # Errors
///
/// A message if a font of the probe is missing from the book, if layout or rasterization fails, or what `keep` returns.
pub fn run_probe(
    book: &FontBook,
    probe: &Probe,
    keep: &mut Keep<'_>,
) -> Result<ProbeOutput, String> {
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
    let renderings = render_all(&scene, keep)?;
    Ok(ProbeOutput {
        result: ProbeResult {
            id: probe.id.to_owned(),
            lines: layout.lines.len(),
            glyphs: glyphs().count(),
            missing_glyphs: glyphs().filter(|glyph| glyph.id == 0).count(),
            layout_sha256: hex(&sha256(&layout_json)),
            width: scene.width,
            height: scene.height,
            reference_sha256: renderings.reference_sha256,
            gamma2_sha256: renderings.gamma2_sha256,
            vello: renderings.vello,
        },
        layout_json,
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
            let mut images: Vec<(String, Image)> = Vec::new();
            let output = run_probe(&book, probe, &mut |name, image| {
                images.push((name.to_owned(), image.clone()));
                Ok(())
            })
            .unwrap();
            let result = &output.result;
            assert_eq!(result.id, probe.id);
            assert!(result.lines >= 2, "{}: {} lines", probe.id, result.lines);
            assert!(result.glyphs > 20, "{}", probe.id);
            assert_eq!(result.missing_glyphs, 0, "{}", probe.id);
            assert_eq!(result.layout_sha256, hex(&sha256(&output.layout_json)));
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
            assert_eq!(result.vello.len(), 3);
            for (vello, (_, image)) in result.vello.iter().zip(&images[2..]) {
                assert_eq!(vello.sha256, hex(&image.sha256()));
                assert_eq!(
                    vello.difference.pixels,
                    u64::from(result.width * result.height)
                );
            }
        }
    }

    #[test]
    fn stops_at_the_first_image_it_cannot_keep() {
        let book = committed_book();
        let mut kept = Vec::new();
        let problem = run_probe(&book, committed_probes()[0], &mut |name, _| {
            if kept.len() == 1 {
                return Err(format!("cannot write {name}"));
            }
            kept.push(name.to_owned());
            Ok(())
        })
        .err();
        assert_eq!(problem.as_deref(), Some("cannot write gamma2"));
        assert_eq!(kept, ["reference"]);
    }

    #[test]
    fn summarizes_a_run_in_two_hashes() {
        let book = committed_book();
        let probes: Vec<ProbeResult> = committed_probes()
            .into_iter()
            .map(|probe| run_probe(&book, probe, &mut |_, _| Ok(())).unwrap().result)
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
        let mut images: Vec<(String, Image)> = Vec::new();
        let result = run_viewport(&book, &probes, 640, 900, &mut |name, image| {
            images.push((name.to_owned(), image.clone()));
            Ok(())
        })
        .unwrap();
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
