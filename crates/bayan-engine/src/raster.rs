//! The mock document's rasterizer: rectangles of solid colour, drawn with exact area coverage in integer arithmetic only.
//!
//! Until the reference rasterizer of ADR-0011 arrives (CORE-003), the mock document is drawn from axis-aligned rectangles. Each pixel of a tile covers a known part of the page (spec §7); a rectangle's coverage of a pixel is the exact fraction of the pixel's area it overlaps, computed with integers and rounded once, so every platform produces the same bytes (ADR-0004 tier A, ADR-0005). Pixels are premultiplied RGBA8, rows from top to bottom, and start transparent, so anything outside the page stays transparent.

use crate::protocol::Rect;

/// An opaque colour. For opaque colours the premultiplied and the straight form are the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb {
    pub(crate) r: u8,
    pub(crate) g: u8,
    pub(crate) b: u8,
}

/// A rectangle of solid colour on a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    /// The rectangle, in page coordinates (BLU).
    pub(crate) rect: Rect,
    /// Its colour.
    pub(crate) color: Rgb,
}

/// The part of a page a tile shows, and its size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileGeometry {
    /// The part of the page, in page coordinates; its width and height are positive.
    pub(crate) rect: Rect,
    /// The width in pixels; positive.
    pub(crate) width: u32,
    /// The height in pixels; positive.
    pub(crate) height: u32,
}

/// The pixels along one axis that a shape overlaps, and how much of each.
struct Span {
    /// The first pixel overlapped.
    first: usize,
    /// How much of each pixel, from `first` on, is overlapped, as a length in units where a whole pixel is `extent` (the tile's extent along the axis in BLU).
    overlaps: Vec<i128>,
}

/// The pixels along one axis that the interval from `start` to `end` (BLU) overlaps, for a tile that covers `extent` BLU from `origin` with `pixels` pixels.
///
/// Multiplying every coordinate by `pixels` makes pixel `k` cover exactly the interval from `origin × pixels + k × extent` to `origin × pixels + (k + 1) × extent`, so every boundary is an integer. The callers keep coordinates within 2⁴¹ BLU and `pixels` within 2¹², so the products stay far inside `i128`.
fn span(origin: i64, extent: i64, pixels: u32, start: i64, end: i64) -> Option<Span> {
    let (origin, extent, count) = (i128::from(origin), i128::from(extent), i128::from(pixels));
    let (start, end) = (i128::from(start) * count, i128::from(end) * count);
    let base = origin * count;
    if end <= start || extent <= 0 || count <= 0 {
        return None;
    }
    // The first and last pixels the interval can reach, clamped to the tile.
    let first = ((start - base).div_euclid(extent)).max(0);
    let last = ((end - base - 1).div_euclid(extent)).min(count - 1);
    if first > last {
        return None;
    }
    let mut overlaps = Vec::new();
    for pixel in first..=last {
        let pixel_start = base + pixel * extent;
        let pixel_end = pixel_start + extent;
        let overlap = end.min(pixel_end) - start.max(pixel_start);
        overlaps.push(overlap.max(0));
    }
    Some(Span {
        first: usize::try_from(first).ok()?,
        overlaps,
    })
}

/// Blends a channel of an opaque colour over a premultiplied channel, with coverage `alpha` from 0 to 255, rounding to the nearest value.
fn blend(source: u8, destination: u8, alpha: u32) -> u8 {
    let value = (u32::from(source) * alpha + u32::from(destination) * (255 - alpha) + 127) / 255;
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// Draws the shapes, in order, into a new tile of `geometry.width × geometry.height` premultiplied RGBA8 pixels without row padding. Returns `None` if the tile's size does not fit in memory.
pub(crate) fn rasterize<'a>(
    shapes: impl IntoIterator<Item = &'a Shape>,
    geometry: &TileGeometry,
) -> Option<Vec<u8>> {
    let width = usize::try_from(geometry.width).ok()?;
    let height = usize::try_from(geometry.height).ok()?;
    let row_bytes = width.checked_mul(4)?;
    let size = row_bytes.checked_mul(height)?;
    // A tile can need 64 MiB. If that much memory cannot be had, the tile fails (`internal`) instead of the allocation aborting the process.
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(size).ok()?;
    pixels.resize(size, 0);
    let area = i128::from(geometry.rect.width) * i128::from(geometry.rect.height);
    for shape in shapes {
        let Some(columns) = span(
            geometry.rect.x,
            geometry.rect.width,
            geometry.width,
            shape.rect.x,
            shape.rect.x.saturating_add(shape.rect.width),
        ) else {
            continue;
        };
        let Some(rows) = span(
            geometry.rect.y,
            geometry.rect.height,
            geometry.height,
            shape.rect.y,
            shape.rect.y.saturating_add(shape.rect.height),
        ) else {
            continue;
        };
        let color = [shape.color.r, shape.color.g, shape.color.b, u8::MAX];
        for (row_index, &row_overlap) in rows.overlaps.iter().enumerate() {
            let row_start = (rows.first + row_index) * row_bytes;
            for (column_index, &column_overlap) in columns.overlaps.iter().enumerate() {
                let alpha = if row_overlap == i128::from(geometry.rect.height)
                    && column_overlap == i128::from(geometry.rect.width)
                {
                    255
                } else {
                    // The exact covered fraction of the pixel's area, times 255, rounded to the nearest integer.
                    let covered = 255 * row_overlap * column_overlap;
                    u32::try_from((covered + area / 2) / area).unwrap_or(255)
                };
                if alpha == 0 {
                    continue;
                }
                let start = row_start + (columns.first + column_index) * 4;
                let Some(pixel) = pixels.get_mut(start..start + 4) else {
                    continue;
                };
                for (channel, &source) in pixel.iter_mut().zip(&color) {
                    *channel = blend(source, *channel, alpha);
                }
            }
        }
    }
    Some(pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = Rgb { r: 255, g: 0, b: 0 };
    const WHITE: Rgb = Rgb {
        r: 255,
        g: 255,
        b: 255,
    };

    fn shape(x: i64, y: i64, width: i64, height: i64, color: Rgb) -> Shape {
        Shape {
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            color,
        }
    }

    fn tile(x: i64, y: i64, size: i64, pixels: u32) -> TileGeometry {
        TileGeometry {
            rect: Rect {
                x,
                y,
                width: size,
                height: size,
            },
            width: pixels,
            height: pixels,
        }
    }

    fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
        let start = (y * width + x) * 4;
        [
            pixels[start],
            pixels[start + 1],
            pixels[start + 2],
            pixels[start + 3],
        ]
    }

    #[test]
    fn starts_transparent_and_fills_covered_pixels_exactly() {
        // A 4 × 4 tile of a 400 × 400 BLU square: each pixel covers 100 × 100 BLU.
        let shapes = [shape(100, 100, 200, 100, RED)];
        let pixels = rasterize(&shapes, &tile(0, 0, 400, 4)).unwrap();
        assert_eq!(pixels.len(), 4 * 4 * 4);
        assert_eq!(pixel(&pixels, 4, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&pixels, 4, 1, 1), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixels, 4, 2, 1), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixels, 4, 3, 1), [0, 0, 0, 0]);
        assert_eq!(pixel(&pixels, 4, 1, 2), [0, 0, 0, 0]);
    }

    #[test]
    fn partial_coverage_is_the_exact_area_fraction() {
        // A quarter of one pixel (50 × 50 of 100 × 100 BLU): alpha 255 / 4 = 63.75, rounded to 64, premultiplied.
        let shapes = [shape(50, 50, 50, 50, RED)];
        let pixels = rasterize(&shapes, &tile(0, 0, 400, 4)).unwrap();
        assert_eq!(pixel(&pixels, 4, 0, 0), [64, 0, 0, 64]);
        // Over opaque white, the same quarter blends into it.
        let shapes = [shape(0, 0, 400, 400, WHITE), shape(50, 50, 50, 50, RED)];
        let pixels = rasterize(&shapes, &tile(0, 0, 400, 4)).unwrap();
        assert_eq!(pixel(&pixels, 4, 0, 0), [255, 191, 191, 255]);
    }

    #[test]
    fn tiles_of_one_page_agree_where_they_meet() {
        // Rendering a square in one tile or in four quarters gives the same pixels.
        let shapes = [
            shape(0, 0, 1000, 1000, WHITE),
            shape(130, 270, 555, 333, RED),
        ];
        let whole = rasterize(&shapes, &tile(0, 0, 1000, 8)).unwrap();
        for (quarter_x, quarter_y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let part = rasterize(&shapes, &tile(quarter_x * 500, quarter_y * 500, 500, 4)).unwrap();
            for y in 0..4 {
                for x in 0..4 {
                    let x_whole = usize::try_from(quarter_x).unwrap() * 4 + x;
                    let y_whole = usize::try_from(quarter_y).unwrap() * 4 + y;
                    assert_eq!(pixel(&part, 4, x, y), pixel(&whole, 8, x_whole, y_whole));
                }
            }
        }
    }

    #[test]
    fn handles_tiles_outside_and_partly_outside_the_shapes() {
        let shapes = [shape(0, 0, 100, 100, RED)];
        let pixels = rasterize(&shapes, &tile(-1_000_000, 5_000, 10, 2)).unwrap();
        assert!(pixels.iter().all(|&byte| byte == 0));
        let pixels = rasterize(&shapes, &tile(-50, -50, 100, 2)).unwrap();
        assert_eq!(pixel(&pixels, 2, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&pixels, 2, 1, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn copes_with_the_largest_coordinates() {
        let huge = crate::limits::MAX_TILE_COORDINATE;
        let shapes = [shape(-huge, -huge, 2 * huge, 2 * huge, RED)];
        let geometry = TileGeometry {
            rect: Rect {
                x: -huge,
                y: -huge,
                width: huge,
                height: huge,
            },
            width: 4096,
            height: 1,
        };
        let pixels = rasterize(&shapes, &geometry).unwrap();
        assert_eq!(pixel(&pixels, 4096, 0, 0), [255, 0, 0, 255]);
        assert_eq!(pixel(&pixels, 4096, 4095, 0), [255, 0, 0, 255]);
        // A shape that ends 7 BLU into the first pixel of a tile whose pixels are 2²⁸ BLU wide covers almost nothing of it.
        let shapes = [shape(-huge, -huge, 7, 2 * huge, RED)];
        let pixels = rasterize(&shapes, &geometry).unwrap();
        assert_eq!(pixel(&pixels, 4096, 0, 0), [0, 0, 0, 0]);
    }
}
