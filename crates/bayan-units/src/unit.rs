//! The units of length that OOXML, Word and output devices use, each a whole number of BLU.

use crate::Blu;

/// One inch in BLU: 1 BLU = 1/1,828,800 inch = 1/25,400 point (ADR-0005).
pub(crate) const BLU_PER_INCH: i64 = 1_828_800;

/// A resolution, in dots or pixels per inch, at which one dot is a whole number of BLU.
///
/// One dot is 1,828,800 ÷ dpi BLU, so a resolution qualifies when it divides 1,828,800 = 2⁶ · 3² · 5² · 127. All the usual ones do: 72, 96, 120, 144, 192, 300, 600 and 1,200 dpi. Others, such as 168 dpi (175 % of 96), do not, and are refused by [`Dpi::new`]; convert to them with [`Blu::scale`](crate::Blu::scale) and an explicit rounding mode instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Dpi(u32);

impl Dpi {
    /// 72 dpi, where a pixel is a point: 25,400 BLU.
    pub const DPI_72: Dpi = Dpi(72);
    /// 96 dpi, the reference resolution of screens and CSS: 19,050 BLU.
    pub const DPI_96: Dpi = Dpi(96);
    /// 144 dpi: 12,700 BLU.
    pub const DPI_144: Dpi = Dpi(144);
    /// 300 dpi, a common printer resolution: 6,096 BLU.
    pub const DPI_300: Dpi = Dpi(300);
    /// 600 dpi: 3,048 BLU.
    pub const DPI_600: Dpi = Dpi(600);
    /// 1,200 dpi: 1,524 BLU.
    pub const DPI_1200: Dpi = Dpi(1_200);

    /// The resolution `dots_per_inch`, or `None` if one dot at that resolution is not a whole number of BLU (including 0 dpi).
    ///
    /// ```
    /// use bayan_units::Dpi;
    ///
    /// assert_eq!(Dpi::new(96), Some(Dpi::DPI_96));
    /// assert!(Dpi::new(192).is_some()); // 200 % of 96 dpi
    /// assert_eq!(Dpi::new(168), None); // 175 %: one dot would be 10,885.71… BLU
    /// ```
    #[must_use]
    pub const fn new(dots_per_inch: u32) -> Option<Dpi> {
        // `as` widens a u32 losslessly; `i64::from` is not available in a `const fn`.
        if dots_per_inch != 0 && BLU_PER_INCH % (dots_per_inch as i64) == 0 {
            Some(Dpi(dots_per_inch))
        } else {
            None
        }
    }

    /// The number of dots per inch.
    #[must_use]
    pub const fn dots_per_inch(self) -> u32 {
        self.0
    }

    /// The size of one dot.
    #[must_use]
    pub const fn dot(self) -> Blu {
        Blu(BLU_PER_INCH / (self.0 as i64))
    }
}

/// A unit of length, each an exact whole number of BLU (the table of ADR-0005).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LengthUnit {
    /// The inch: 1,828,800 BLU.
    Inch,
    /// The pica, 12 points or 1/6 inch: 304,800 BLU. OOXML universal measures write it `pc` or `pi`.
    Pica,
    /// The point, 1/72 inch: 25,400 BLU.
    Point,
    /// Half a point: 12,700 BLU. WordprocessingML measures font sizes in it (`w:sz`).
    HalfPoint,
    /// An eighth of a point: 3,175 BLU. WordprocessingML measures border widths in it.
    EighthPoint,
    /// The twip ("twentieth of a point"), 1/1,440 inch: 1,270 BLU. Most WordprocessingML lengths are in twips.
    Twip,
    /// The English Metric Unit, 1/914,400 inch or 1/360,000 centimetre: 2 BLU. DrawingML measures in it.
    Emu,
    /// The centimetre: 720,000 BLU.
    Centimetre,
    /// The millimetre: 72,000 BLU.
    Millimetre,
    /// A pixel or printer dot at the given resolution, for example 19,050 BLU at 96 dpi.
    Dot(Dpi),
}

impl LengthUnit {
    /// One of this unit, in BLU.
    ///
    /// ```
    /// use bayan_units::{Blu, Dpi, LengthUnit};
    ///
    /// assert_eq!(LengthUnit::Twip.blu(), Blu(1_270));
    /// assert_eq!(LengthUnit::Dot(Dpi::DPI_96).blu(), Blu(19_050));
    /// ```
    #[must_use]
    pub const fn blu(self) -> Blu {
        match self {
            LengthUnit::Inch => Blu::INCH,
            LengthUnit::Pica => Blu::PICA,
            LengthUnit::Point => Blu::POINT,
            LengthUnit::HalfPoint => Blu::HALF_POINT,
            LengthUnit::EighthPoint => Blu::EIGHTH_POINT,
            LengthUnit::Twip => Blu::TWIP,
            LengthUnit::Emu => Blu::EMU,
            LengthUnit::Centimetre => Blu::CENTIMETRE,
            LengthUnit::Millimetre => Blu::MILLIMETRE,
            LengthUnit::Dot(dpi) => dpi.dot(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unit_has_its_size_from_the_table_of_adr_0005() {
        let table = [
            (LengthUnit::Inch, 1_828_800),
            (LengthUnit::Point, 25_400),
            (LengthUnit::Twip, 1_270),
            (LengthUnit::HalfPoint, 12_700),
            (LengthUnit::Emu, 2),
            (LengthUnit::EighthPoint, 3_175),
            (LengthUnit::Millimetre, 72_000),
            (LengthUnit::Centimetre, 720_000),
            (LengthUnit::Dot(Dpi::DPI_96), 19_050),
            (LengthUnit::Dot(Dpi::DPI_72), 25_400),
            (LengthUnit::Dot(Dpi::DPI_300), 6_096),
            (LengthUnit::Dot(Dpi::DPI_600), 3_048),
            (LengthUnit::Dot(Dpi::DPI_1200), 1_524),
            (LengthUnit::Dot(Dpi::DPI_144), 12_700),
            // Not in the table, but needed for the universal measures `pc` and `pi`: 12 points.
            (LengthUnit::Pica, 304_800),
        ];
        for (unit, blu) in table {
            assert_eq!(unit.blu(), Blu(blu), "{unit:?}");
        }
    }

    #[test]
    fn the_units_relate_to_each_other_as_their_definitions_say() {
        let blu = |unit: LengthUnit| unit.blu().0;
        assert_eq!(blu(LengthUnit::Inch), 72 * blu(LengthUnit::Point));
        assert_eq!(blu(LengthUnit::Inch), 1_440 * blu(LengthUnit::Twip));
        assert_eq!(blu(LengthUnit::Inch), 914_400 * blu(LengthUnit::Emu));
        assert_eq!(blu(LengthUnit::Pica), 12 * blu(LengthUnit::Point));
        assert_eq!(blu(LengthUnit::Point), 20 * blu(LengthUnit::Twip));
        assert_eq!(blu(LengthUnit::Point), 2 * blu(LengthUnit::HalfPoint));
        assert_eq!(blu(LengthUnit::Point), 8 * blu(LengthUnit::EighthPoint));
        assert_eq!(blu(LengthUnit::Point), 12_700 * blu(LengthUnit::Emu));
        assert_eq!(
            blu(LengthUnit::Centimetre),
            10 * blu(LengthUnit::Millimetre)
        );
        assert_eq!(blu(LengthUnit::Centimetre), 360_000 * blu(LengthUnit::Emu));
        // 127 mm = 5 inches, because an inch is exactly 25.4 mm.
        assert_eq!(127 * blu(LengthUnit::Millimetre), 5 * blu(LengthUnit::Inch));
    }

    #[test]
    fn accepts_exactly_the_resolutions_that_divide_an_inch_into_whole_blu() {
        for dpi in [
            1, 72, 96, 120, 144, 150, 192, 288, 300, 600, 1_200, 2_400, 1_828_800,
        ] {
            let resolution = Dpi::new(dpi).expect("a divisor of 1,828,800");
            assert_eq!(resolution.dots_per_inch(), dpi);
            assert_eq!(i64::from(dpi) * resolution.dot().0, 1_828_800, "{dpi} dpi");
        }
        for dpi in [0, 7, 168, 9_600, 1_000_000, 3_657_600, u32::MAX] {
            assert_eq!(Dpi::new(dpi), None, "{dpi} dpi");
        }
    }

    #[test]
    fn the_named_resolutions_are_the_ones_their_names_say() {
        let named = [
            (Dpi::DPI_72, 72),
            (Dpi::DPI_96, 96),
            (Dpi::DPI_144, 144),
            (Dpi::DPI_300, 300),
            (Dpi::DPI_600, 600),
            (Dpi::DPI_1200, 1_200),
        ];
        for (resolution, dpi) in named {
            assert_eq!(Dpi::new(dpi), Some(resolution));
        }
    }
}
