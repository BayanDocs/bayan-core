//! Rasterizing a layout: the glyphs' outlines from skrifa, unhinted (ADR-0011 §4), filled in black on white with tiny-skia in its reference mode, and with vello_cpu for comparison.
//!
//! **The device** is a screen of 96 dpi at device scale 2, 192 dpi, where one pixel is 9,525 BLU. A paragraph's image has a margin of 8 pixels on every side.
//!
//! **Coordinates.** A glyph's origin is an integer in BLU; skrifa's outline points are font units in `f32` (exact integers for TrueType, 16.16 fixed-point values for CFF). Each point becomes a device coordinate with IEEE-754 basic operations only, in `f64`: `(origin + point × size ÷ units_per_em) ÷ 9,525`, then rounded to the nearest `f32`. Those operations give the same bits on every platform (ADR-0005 §4).
//!
//! **Blending.** Pixels are 8-bit premultiplied RGBA. Coverage comes from each rasterizer's anti-aliasing, grayscale only (no subpixel rendering), and is composited with source-over. `Blending::Linear` blends the stored sRGB values directly, as most 2D renderers do; `Blending::Gamma2` blends in a linear-light approximation (square, blend, square root), which tiny-skia computes with the exactly rounded square root of IEEE-754. REPORT.md §6 compares them.

use std::collections::BTreeMap;

use bayan_units::Rounding;
use sha2::{Digest, Sha256};
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};

use crate::fonts::{Font, FontBook};
use crate::layout::Layout;

/// BLU per device pixel: 96 dpi at device scale 2 is 192 dpi, and 1,828,800 ÷ 192 = 9,525.
pub const BLU_PER_PIXEL: i64 = 9_525;

/// The margin around a paragraph's image, in device pixels.
pub const MARGIN: u32 = 8;

/// An image of 8-bit premultiplied RGBA pixels, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The pixels: `width × height × 4` bytes, red, green, blue and alpha.
    pub rgba: Vec<u8>,
}

impl Image {
    /// The raster hash: the SHA-256 of a header naming the format and size (`RGBA8 premultiplied <width>x<height>` and a line feed) followed by the pixels.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(format!(
            "RGBA8 premultiplied {}x{}\n",
            self.width, self.height
        ));
        hasher.update(&self.rgba);
        hasher.finalize().into()
    }

    /// The image as a PNG file.
    ///
    /// # Errors
    ///
    /// A message if the image is empty or its size and pixels disagree.
    pub fn png(&self) -> Result<Vec<u8>, String> {
        crate::png::encode_rgba8(self.width, self.height, &self.rgba)
    }
}

/// One command of a glyph outline, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Starts a contour.
    MoveTo(f32, f32),
    /// A straight line.
    LineTo(f32, f32),
    /// A quadratic Bézier curve through a control point.
    QuadTo(f32, f32, f32, f32),
    /// A cubic Bézier curve through two control points.
    CurveTo(f32, f32, f32, f32, f32, f32),
    /// Closes the contour.
    Close,
}

/// What a paragraph's image shows: its size, and the outline of every glyph in device pixels, one path per glyph, in drawing order.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The glyph outlines.
    pub paths: Vec<Vec<Command>>,
}

/// The scene of `layout`: an image big enough for every line (lines that overflow the measure included) plus the margin, and the glyphs placed in it.
///
/// # Errors
///
/// A message if a font of the layout is not in `book`, if skrifa cannot draw a glyph, or if the image would be too big.
pub fn scene(book: &FontBook, layout: &Layout) -> Result<Scene, String> {
    // The lines' horizontal extent: the measure, widened by lines that overflow it on either side.
    let mut left = 0_i64;
    let mut right = layout.measure;
    for line in &layout.lines {
        left = left.min(line.left);
        right = right.max(line.left.saturating_add(line.advance));
    }
    let margin = i64::from(MARGIN) * BLU_PER_PIXEL;
    let width = pixels(right.saturating_sub(left))?
        .checked_add(2 * MARGIN)
        .ok_or("the image would be too wide")?;
    let height = pixels(layout.height)?
        .checked_add(2 * MARGIN)
        .ok_or("the image would be too high")?;
    Ok(Scene {
        width,
        height,
        paths: glyph_paths(book, layout, margin.saturating_sub(left), margin)?,
    })
}

/// The outline of every glyph of `layout` in device pixels, one path per glyph in drawing order, with the paragraph's top left corner `x` and `y` BLU from the image's top left corner.
///
/// # Errors
///
/// A message if a font of the layout is not in `book` or is another file, or if skrifa cannot draw a glyph.
pub fn glyph_paths(
    book: &FontBook,
    layout: &Layout,
    x: i64,
    y: i64,
) -> Result<Vec<Vec<Command>>, String> {
    let fonts = layout
        .fonts
        .iter()
        .map(|font| {
            let index = book
                .index(&font.file)
                .ok_or_else(|| format!("the font book has no {}", font.file))?;
            let found = &book.fonts()[index];
            if crate::hash::hex(found.sha256()) == font.sha256 {
                Ok(found)
            } else {
                Err(format!(
                    "{} is not the file the layout was made with",
                    font.file
                ))
            }
        })
        .collect::<Result<Vec<&Font>, String>>()?;
    let references = fonts
        .iter()
        .map(|font| font.outlines())
        .collect::<Result<Vec<_>, String>>()?;
    let collections: Vec<_> = references
        .iter()
        .map(MetadataProvider::outline_glyphs)
        .collect();
    // Outlines in font units, drawn once per font and glyph.
    let mut outlines: BTreeMap<(usize, u32), Vec<Command>> = BTreeMap::new();
    let mut paths = Vec::new();
    for line in &layout.lines {
        for run in &line.runs {
            let font = fonts
                .get(run.font)
                .ok_or("a run names a font the layout lacks")?;
            let scale = exact(layout.size)? / f64::from(font.units_per_em());
            for glyph in &run.glyphs {
                let outline = match outlines.entry((run.font, glyph.id)) {
                    std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        let mut pen = Recorder::default();
                        if let Some(drawable) =
                            collections[run.font].get(skrifa::GlyphId::new(glyph.id))
                        {
                            drawable
                                .draw(
                                    DrawSettings::unhinted(
                                        Size::unscaled(),
                                        LocationRef::default(),
                                    ),
                                    &mut pen,
                                )
                                .map_err(|error| {
                                    format!("{}: glyph {}: {error}", font.file(), glyph.id)
                                })?;
                        }
                        entry.insert(pen.commands)
                    }
                };
                let origin_x = exact(x.saturating_add(glyph.x))?;
                let origin_y = exact(y.saturating_add(glyph.y))?;
                paths.push(
                    outline
                        .iter()
                        .map(|&command| place(command, origin_x, origin_y, scale))
                        .collect(),
                );
            }
        }
    }
    Ok(paths)
}

/// A length in BLU as a whole number of device pixels, rounded up.
fn pixels(blu: i64) -> Result<u32, String> {
    bayan_units::scale(blu, 1, BLU_PER_PIXEL, Rounding::Ceiling)
        .and_then(|pixels| u32::try_from(pixels).ok())
        .ok_or_else(|| format!("{blu} BLU is not a usable image size"))
}

/// `value` as an `f64`, exactly: every integer up to 2^53 is one.
#[expect(
    clippy::cast_precision_loss,
    reason = "checked to be at most 2^53 in magnitude, where the conversion is exact"
)]
fn exact(value: i64) -> Result<f64, String> {
    if value.unsigned_abs() > 1 << 53 {
        return Err(format!("{value} BLU is too far from the page to draw"));
    }
    Ok(value as f64)
}

/// Moves a command from font units at the glyph origin (`x`, `y`, in BLU from the image's top left corner) to device pixels: x to the right, y down (font units point up).
#[expect(
    clippy::cast_possible_truncation,
    reason = "rounds an f64 to the nearest f32, which IEEE-754 defines, the same on every platform"
)]
fn place(command: Command, x: f64, y: f64, scale: f64) -> Command {
    let pixels = f64::from(i32::try_from(BLU_PER_PIXEL).unwrap_or(i32::MAX));
    let px = |u: f32| ((x + f64::from(u) * scale) / pixels) as f32;
    let py = |v: f32| ((y - f64::from(v) * scale) / pixels) as f32;
    match command {
        Command::MoveTo(a, b) => Command::MoveTo(px(a), py(b)),
        Command::LineTo(a, b) => Command::LineTo(px(a), py(b)),
        Command::QuadTo(a, b, c, d) => Command::QuadTo(px(a), py(b), px(c), py(d)),
        Command::CurveTo(a, b, c, d, e, f) => {
            Command::CurveTo(px(a), py(b), px(c), py(d), px(e), py(f))
        }
        Command::Close => Command::Close,
    }
}

/// Records an outline as skrifa draws it, in font units.
#[derive(Default)]
struct Recorder {
    commands: Vec<Command>,
}

impl OutlinePen for Recorder {
    fn move_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::MoveTo(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.commands.push(Command::LineTo(x, y));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.commands.push(Command::QuadTo(cx0, cy0, x, y));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        self.commands
            .push(Command::CurveTo(cx0, cy0, cx1, cy1, x, y));
    }

    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}

/// How coverage is blended in the reference rasterizer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blending {
    /// Source-over on the stored sRGB values (tiny-skia's `ColorSpace::Linear`).
    Linear,
    /// Source-over in an approximate linear light, with gamma 2 (tiny-skia's `ColorSpace::Gamma2`).
    Gamma2,
}

/// Renders `scene` with tiny-skia in its reference mode (no SIMD, libm's math; see Cargo.toml): a white page, black glyphs, non-zero winding, anti-aliased.
///
/// # Errors
///
/// A message if the image cannot be allocated.
pub fn render_reference(scene: &Scene, blending: Blending) -> Result<Image, String> {
    let mut pixmap = tiny_skia::Pixmap::new(scene.width, scene.height).ok_or_else(|| {
        format!(
            "tiny-skia cannot allocate {} × {} pixels",
            scene.width, scene.height
        )
    })?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    paint.blend_mode = tiny_skia::BlendMode::SourceOver;
    paint.colorspace = match blending {
        Blending::Linear => tiny_skia::ColorSpace::Linear,
        Blending::Gamma2 => tiny_skia::ColorSpace::Gamma2,
    };
    for commands in &scene.paths {
        let mut builder = tiny_skia::PathBuilder::new();
        for &command in commands {
            match command {
                Command::MoveTo(x, y) => builder.move_to(x, y),
                Command::LineTo(x, y) => builder.line_to(x, y),
                Command::QuadTo(x1, y1, x, y) => builder.quad_to(x1, y1, x, y),
                Command::CurveTo(x1, y1, x2, y2, x, y) => builder.cubic_to(x1, y1, x2, y2, x, y),
                Command::Close => builder.close(),
            }
        }
        // An empty outline (a space) gives no path.
        if let Some(path) = builder.finish() {
            pixmap.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    Ok(Image {
        width: scene.width,
        height: scene.height,
        rgba: pixmap.take(),
    })
}

/// The configurations of vello_cpu that the spike compares with the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vello {
    /// The u8 pipeline (`RenderMode::OptimizeSpeed`) at the best SIMD level this machine has, as an application would run it.
    U8Detected,
    /// The u8 pipeline at the scalar fallback level, the same code on every machine.
    U8Fallback,
    /// The f32 pipeline (`RenderMode::OptimizeQuality`) at the scalar fallback level.
    F32Fallback,
}

impl Vello {
    /// Every configuration, in the order results list them.
    pub const ALL: [Vello; 3] = [Vello::U8Detected, Vello::U8Fallback, Vello::F32Fallback];

    /// A short name for results and reports.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Vello::U8Detected => "vello_u8_detected",
            Vello::U8Fallback => "vello_u8_fallback",
            Vello::F32Fallback => "vello_f32_fallback",
        }
    }
}

/// Renders `scene` with vello_cpu on one thread, and returns the image with the name of the SIMD level it used.
///
/// # Errors
///
/// A message if the image is larger than vello_cpu's 65,535 pixels a side.
pub fn render_vello(scene: &Scene, configuration: Vello) -> Result<(Image, String), String> {
    use vello_cpu::kurbo::{BezPath, Rect};
    let width = u16::try_from(scene.width).map_err(|_| "too wide for vello_cpu")?;
    let height = u16::try_from(scene.height).map_err(|_| "too high for vello_cpu")?;
    let (level, render_mode) = match configuration {
        Vello::U8Detected => (
            vello_cpu::Level::new(),
            vello_cpu::RenderMode::OptimizeSpeed,
        ),
        Vello::U8Fallback => (
            vello_cpu::Level::fallback(),
            vello_cpu::RenderMode::OptimizeSpeed,
        ),
        Vello::F32Fallback => (
            vello_cpu::Level::fallback(),
            vello_cpu::RenderMode::OptimizeQuality,
        ),
    };
    let mut context = vello_cpu::RenderContext::new_with(
        width,
        height,
        vello_cpu::RenderSettings {
            level,
            num_threads: 0,
        },
    );
    context.set_paint(vello_cpu::color::palette::css::WHITE);
    context.fill_rect(&Rect::new(0.0, 0.0, f64::from(width), f64::from(height)));
    context.set_paint(vello_cpu::color::palette::css::BLACK);
    context.set_fill_rule(vello_cpu::peniko::Fill::NonZero);
    for commands in &scene.paths {
        let mut path = BezPath::new();
        for &command in commands {
            let p = |x: f32, y: f32| (f64::from(x), f64::from(y));
            match command {
                Command::MoveTo(x, y) => path.move_to(p(x, y)),
                Command::LineTo(x, y) => path.line_to(p(x, y)),
                Command::QuadTo(x1, y1, x, y) => path.quad_to(p(x1, y1), p(x, y)),
                Command::CurveTo(x1, y1, x2, y2, x, y) => {
                    path.curve_to(p(x1, y1), p(x2, y2), p(x, y))
                }
                Command::Close => path.close_path(),
            }
        }
        if !path.is_empty() {
            context.fill_path(&path);
        }
    }
    context.flush();
    let mut pixmap = vello_cpu::Pixmap::new(width, height);
    context.render_with(
        &mut pixmap,
        &mut vello_cpu::Resources::new(),
        vello_cpu::RasterizerSettings {
            render_mode,
            ..vello_cpu::RasterizerSettings::default()
        },
    );
    // fearless_simd's Debug output starts with the level's name, such as `Avx2(…)` or `Fallback(…)`.
    let name = format!("{level:?}");
    let name = name
        .split(['(', ' ', '{'])
        .next()
        .unwrap_or("unknown")
        .to_owned();
    Ok((
        Image {
            width: scene.width,
            height: scene.height,
            rgba: pixmap.data_as_u8_slice().to_vec(),
        },
        name,
    ))
}

/// The renderings by name, as the measurements and the image files name them: the reference, tiny-skia with gamma-2 blending, and each vello_cpu configuration ([`Vello::name`]).
pub const RASTERIZERS: [&str; 5] = [
    "reference",
    "gamma2",
    "vello_u8_detected",
    "vello_u8_fallback",
    "vello_f32_fallback",
];

/// Renders `scene` with the rasterizer `name`, one of [`RASTERIZERS`].
///
/// # Errors
///
/// A message if there is no rasterizer of that name, or rendering fails.
pub fn render_named(scene: &Scene, name: &str) -> Result<Image, String> {
    match name {
        "reference" => render_reference(scene, Blending::Linear),
        "gamma2" => render_reference(scene, Blending::Gamma2),
        name => {
            let configuration = Vello::ALL
                .into_iter()
                .find(|configuration| configuration.name() == name)
                .ok_or_else(|| format!("no rasterizer {name}"))?;
            Ok(render_vello(scene, configuration)?.0)
        }
    }
}

/// How two images of the same size differ, channel by channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Difference {
    /// The number of pixels.
    pub pixels: u64,
    /// The pixels where any channel differs.
    pub differing: u64,
    /// The pixels where a channel differs by more than 2, the tolerance ADR-0011 §3 allows the interactive rasterizer at anti-aliased edges.
    pub beyond_2: u64,
    /// The pixels where a channel differs by more than 64: beyond what anti-aliasing at an edge explains, a sign of different geometry.
    pub beyond_64: u64,
    /// The largest difference of any channel.
    pub max: u8,
}

/// Compares two images of the same size.
///
/// # Errors
///
/// A message if their sizes differ.
pub fn compare(a: &Image, b: &Image) -> Result<Difference, String> {
    if (a.width, a.height) != (b.width, b.height) || a.rgba.len() != b.rgba.len() {
        return Err(format!(
            "cannot compare an image of {} × {} pixels with one of {} × {}",
            a.width, a.height, b.width, b.height
        ));
    }
    let mut difference = Difference::default();
    for (p, q) in a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
    {
        let largest = p
            .iter()
            .zip(q)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        difference.pixels += 1;
        difference.differing += u64::from(largest > 0);
        difference.beyond_2 += u64::from(largest > 2);
        difference.beyond_64 += u64::from(largest > 64);
        difference.max = difference.max.max(largest);
    }
    Ok(difference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::tests::committed_book;
    use crate::layout::{Paragraph, lay_out};
    use crate::shape::Features;
    use bayan_units::Blu;

    fn hebrew_scene() -> Scene {
        let book = committed_book();
        let layout = lay_out(
            &book,
            &Paragraph {
                text: "שלום עולם, זהו מבחן 123",
                rtl: true,
                fonts: &[0],
                size: Blu::from_points(14),
                measure: Blu::INCH * 2,
                language: "he",
                features: Features {
                    kerning: true,
                    ligatures: true,
                },
            },
        )
        .unwrap();
        scene(&book, &layout).unwrap()
    }

    /// The share of pixels, in thousandths, that are neither pure white nor pure black.
    fn grey_permille(image: &Image) -> u64 {
        let grey = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] != 0 && pixel[0] != 255)
            .count();
        u64::try_from(grey * 1000 / (image.rgba.len() / 4)).unwrap()
    }

    #[test]
    fn renders_with_the_rasterizer_it_is_named() {
        let scene = hebrew_scene();
        assert_eq!(
            RASTERIZERS[..2],
            ["reference", "gamma2"],
            "the reference first, as the measurements and results list them"
        );
        assert_eq!(
            render_named(&scene, "reference").unwrap(),
            render_reference(&scene, Blending::Linear).unwrap()
        );
        assert_eq!(
            render_named(&scene, "gamma2").unwrap(),
            render_reference(&scene, Blending::Gamma2).unwrap()
        );
        assert_eq!(RASTERIZERS.len(), 2 + Vello::ALL.len());
        for (name, configuration) in RASTERIZERS[2..].iter().zip(Vello::ALL) {
            assert_eq!(*name, configuration.name());
            assert_eq!(
                render_named(&scene, name).unwrap(),
                render_vello(&scene, configuration).unwrap().0
            );
        }
        assert_eq!(
            render_named(&scene, "skia").unwrap_err(),
            "no rasterizer skia"
        );
    }

    #[test]
    fn sizes_the_image_for_the_measure_and_the_margin() {
        let scene = hebrew_scene();
        // Two inches at 192 dpi are 384 pixels, plus 8 on each side.
        assert_eq!(scene.width, 384 + 16);
        assert!(scene.height > 16);
        // One path per glyph, and the glyphs of letters are not empty.
        assert!(scene.paths.iter().filter(|path| !path.is_empty()).count() >= 15);
        // Every point lies inside the image.
        for command in scene.paths.iter().flatten() {
            if let Command::MoveTo(x, y) | Command::LineTo(x, y) = *command {
                assert!(x > 0.0 && x < 400.0 && y > 0.0 && y < 400.0, "{x} {y}");
            }
        }
    }

    #[test]
    fn renders_black_glyphs_on_a_white_page() {
        let image = render_reference(&hebrew_scene(), Blending::Linear).unwrap();
        assert_eq!(
            image.rgba.len(),
            usize::try_from(image.width * image.height * 4).unwrap()
        );
        // Opaque everywhere, grey levels only (R = G = B), with ink and anti-aliased edges.
        assert!(
            image
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2])
        );
        assert!(image.rgba.as_chunks::<4>().0.iter().any(|p| p[0] == 0));
        assert!(grey_permille(&image) > 0);
        // The margin is white.
        assert!(
            image.rgba[..usize::try_from(4 * image.width * MARGIN).unwrap()]
                .iter()
                .all(|&b| b == 255)
        );
    }

    #[test]
    fn renders_the_same_pixels_every_time() {
        let scene = hebrew_scene();
        let first = render_reference(&scene, Blending::Linear).unwrap();
        let second = render_reference(&scene, Blending::Linear).unwrap();
        assert_eq!(first.sha256(), second.sha256());
        assert_eq!(compare(&first, &second).unwrap().differing, 0);
    }

    #[test]
    fn gamma_2_blending_changes_only_the_edges() {
        let scene = hebrew_scene();
        let linear = render_reference(&scene, Blending::Linear).unwrap();
        let gamma = render_reference(&scene, Blending::Gamma2).unwrap();
        let difference = compare(&linear, &gamma).unwrap();
        assert!(difference.differing > 0);
        // Fully covered pixels (black) and uncovered ones (white) are the same in both: gamma only changes partial coverage.
        for (a, b) in linear.rgba.iter().zip(&gamma.rgba) {
            if *a == 0 || *a == 255 {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn vello_draws_the_same_glyphs_within_anti_aliasing() {
        let scene = hebrew_scene();
        let reference = render_reference(&scene, Blending::Linear).unwrap();
        for configuration in Vello::ALL {
            let (image, level) = render_vello(&scene, configuration).unwrap();
            assert!(!level.is_empty());
            let difference = compare(&reference, &image).unwrap();
            // Different anti-aliasing, the same geometry: no pixel is ink in one and blank in the other.
            assert!(difference.differing > 0, "{configuration:?}");
            assert!(difference.max < 255, "{configuration:?}: {difference:?}");
        }
    }

    #[test]
    fn the_raster_hash_covers_the_size_and_every_pixel() {
        let image = Image {
            width: 2,
            height: 1,
            rgba: vec![255; 8],
        };
        let mut wider = image.clone();
        wider.width = 1;
        wider.height = 2;
        assert_ne!(image.sha256(), wider.sha256());
        let mut darker = image.clone();
        darker.rgba[5] = 254;
        assert_ne!(image.sha256(), darker.sha256());
        let mut expected = b"RGBA8 premultiplied 2x1\n".to_vec();
        expected.extend_from_slice(&[255; 8]);
        assert_eq!(image.sha256(), crate::hash::sha256(&expected));
    }

    #[test]
    fn compares_only_images_of_one_size() {
        let a = Image {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 255],
        };
        let b = Image {
            width: 1,
            height: 1,
            rgba: vec![3, 0, 70, 255],
        };
        assert_eq!(
            compare(&a, &b).unwrap(),
            Difference {
                pixels: 1,
                differing: 1,
                beyond_2: 1,
                beyond_64: 1,
                max: 70
            }
        );
        let c = Image {
            width: 2,
            height: 1,
            rgba: vec![0; 8],
        };
        assert!(compare(&a, &c).is_err());
    }
}
