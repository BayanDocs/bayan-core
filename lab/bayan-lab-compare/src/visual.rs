//! M5, visual difference: how much of a page looks different, for what text metrics cannot capture (shapes, charts, borders; Fidelity Lab specification §6).
//!
//! M5 compares two rasters of the same page at the same resolution: Word's PDF rasterized by the lab's tools, and BayanDocs' reference raster. Neither exists yet: the lab has no PDF rasterizer, and the engine cannot lay out documents. This module is the interface the lab will feed, with a basic measure behind it; until then the reports say that M5 is not measured ([`NOT_MEASURED`]).
//!
//! The basic measure, [`difference`], is the share of pixels whose surroundings differ visibly: each pixel's 3×3 neighbourhood is averaged on both rasters (pixels beyond an edge repeat the edge), and a pixel differs when the two averages are more than [`THRESHOLD`] grey levels apart. Averaging forgives the one-pixel shifts and the different anti-aliasing of two rasterizers; a missing rule, a moved shape or a different border does not pass. A perceptual measure such as SSIM can replace it behind the same interface. It uses integers only, so it gives the same count on every platform.

use core::fmt;

use serde::{Deserialize, Serialize};

/// The difference, in grey levels from 0 to 255, beyond which two neighbourhood averages differ visibly.
pub const THRESHOLD: u32 = 32;

/// What the reports say about M5 until the lab rasterizes both sides.
pub const NOT_MEASURED: &str = "M5 (visual difference) is not measured yet: the lab has no PDF rasterizer for Word's PDFs and the engine produces no reference rasters of documents; bayan_lab_compare::visual::difference is the measure that will compare them";

/// An 8-bit grey raster: one byte a pixel, row after row, 0 black and 255 white.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raster<'a> {
    width: usize,
    height: usize,
    pixels: &'a [u8],
}

/// Why two rasters could not be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The pixels are not `width × height` bytes, or a side is 0.
    Shape,
    /// The two rasters differ in size.
    Size,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Shape => "a raster's pixels do not fill its width and height",
            Self::Size => "the two rasters differ in size",
        })
    }
}

impl std::error::Error for Error {}

impl<'a> Raster<'a> {
    /// A raster of `width` × `height` pixels.
    ///
    /// # Errors
    ///
    /// [`Error::Shape`] if a side is 0 or `pixels` does not hold exactly `width × height` bytes.
    pub fn new(width: usize, height: usize, pixels: &'a [u8]) -> Result<Self, Error> {
        if width == 0 || height == 0 || width.checked_mul(height) != Some(pixels.len()) {
            return Err(Error::Shape);
        }
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    /// The sum of the 3×3 neighbourhood of (`x`, `y`), edges repeated: nine times its average.
    fn neighbourhood(&self, x: usize, y: usize) -> u32 {
        let mut sum = 0;
        for row in [y.saturating_sub(1), y, (y + 1).min(self.height - 1)] {
            for column in [x.saturating_sub(1), x, (x + 1).min(self.width - 1)] {
                sum += u32::from(self.pixels[row * self.width + column]);
            }
        }
        sum
    }
}

/// The visual difference of one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageDifference {
    /// The page's pixels.
    pub pixels: u64,
    /// How many of them differ visibly.
    pub differing: u64,
}

/// The visual difference of two rasters of one page (see the module's documentation).
///
/// # Errors
///
/// [`Error::Size`] if the rasters differ in size.
pub fn difference(reference: &Raster<'_>, candidate: &Raster<'_>) -> Result<PageDifference, Error> {
    if reference.width != candidate.width || reference.height != candidate.height {
        return Err(Error::Size);
    }
    let mut differing = 0_u64;
    for y in 0..reference.height {
        for x in 0..reference.width {
            if reference
                .neighbourhood(x, y)
                .abs_diff(candidate.neighbourhood(x, y))
                > 9 * THRESHOLD
            {
                differing += 1;
            }
        }
    }
    Ok(PageDifference {
        pixels: u64::try_from(reference.pixels.len()).unwrap_or(u64::MAX),
        differing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_pages_and_faint_noise_do_not_differ() {
        let white = vec![255_u8; 100];
        let page = Raster::new(10, 10, &white).unwrap();
        assert_eq!(
            difference(&page, &page).unwrap(),
            PageDifference {
                pixels: 100,
                differing: 0
            }
        );
        // One pixel a third darker: its neighbourhood averages differ by 9.4 levels at most.
        let mut noisy = white.clone();
        noisy[55] = 170;
        let other = Raster::new(10, 10, &noisy).unwrap();
        assert_eq!(difference(&page, &other).unwrap().differing, 0);
        // A single black pixel shifts each average by 28 levels: still within the threshold.
        noisy[55] = 0;
        let other = Raster::new(10, 10, &noisy).unwrap();
        assert_eq!(difference(&page, &other).unwrap().differing, 0);
    }

    #[test]
    fn a_missing_shape_differs() {
        let white = vec![255_u8; 100];
        let mut square = white.clone();
        for y in 3..7 {
            for x in 3..7 {
                square[y * 10 + x] = 0;
            }
        }
        let (page, other) = (
            Raster::new(10, 10, &white).unwrap(),
            Raster::new(10, 10, &square).unwrap(),
        );
        // The 16 pixels of the square and the 16 beside its sides (two black neighbours or more); the four diagonal corners have one.
        assert_eq!(difference(&page, &other).unwrap().differing, 32);
    }

    #[test]
    fn rasters_must_have_their_shape_and_one_size() {
        assert_eq!(Raster::new(0, 1, &[]), Err(Error::Shape));
        assert_eq!(Raster::new(2, 2, &[0; 3]), Err(Error::Shape));
        let small = [0_u8; 4];
        let large = [0_u8; 6];
        let (a, b) = (
            Raster::new(2, 2, &small).unwrap(),
            Raster::new(3, 2, &large).unwrap(),
        );
        assert_eq!(difference(&a, &b), Err(Error::Size));
    }
}
