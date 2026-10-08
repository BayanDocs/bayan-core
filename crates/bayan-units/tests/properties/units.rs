//! Conversions between units and BLU: exact round trips, rounding against the big-integer reference, and enough headroom for every realistic document.

use bayan_units::{Blu, Dpi, LengthUnit, Rounding};
use num_bigint::BigInt;
use proptest::prelude::*;

use crate::reference;

/// Every unit, with a selection of resolutions for dots.
fn units() -> impl Strategy<Value = LengthUnit> {
    let dpi = |dots_per_inch| LengthUnit::Dot(Dpi::new(dots_per_inch).expect("divides an inch"));
    proptest::sample::select(vec![
        LengthUnit::Inch,
        LengthUnit::Pica,
        LengthUnit::Point,
        LengthUnit::HalfPoint,
        LengthUnit::EighthPoint,
        LengthUnit::Twip,
        LengthUnit::Emu,
        LengthUnit::Centimetre,
        LengthUnit::Millimetre,
        dpi(1),
        dpi(72),
        dpi(96),
        dpi(120),
        dpi(144),
        dpi(192),
        dpi(300),
        dpi(600),
        dpi(1_200),
        dpi(1_828_800),
    ])
}

fn roundings() -> impl Strategy<Value = Rounding> {
    proptest::sample::select(Rounding::ALL.to_vec())
}

/// The largest magnitude of a DrawingML coordinate (ST_Coordinate runs from −27,273,042,329,600 to 27,273,042,316,900 EMU, about 757 km), in BLU: a generous bound for realistic document lengths. The schema sets no upper limit for WordprocessingML's measurements in twips, and the parsers accept any length that fits; bringing such values into a realistic range is the document model's job.
const LARGEST_COORDINATE: i64 = 27_273_042_329_600 * 2;

/// Pairs of units where the first is a whole number of the second, with that number.
const MULTIPLES: [(LengthUnit, LengthUnit, i64); 12] = [
    (LengthUnit::Inch, LengthUnit::Point, 72),
    (LengthUnit::Inch, LengthUnit::Twip, 1_440),
    (LengthUnit::Inch, LengthUnit::Emu, 914_400),
    (LengthUnit::Inch, LengthUnit::Dot(Dpi::DPI_96), 96),
    (LengthUnit::Pica, LengthUnit::Point, 12),
    (LengthUnit::Point, LengthUnit::HalfPoint, 2),
    (LengthUnit::Point, LengthUnit::EighthPoint, 8),
    (LengthUnit::Point, LengthUnit::Twip, 20),
    (LengthUnit::Point, LengthUnit::Emu, 12_700),
    (LengthUnit::Twip, LengthUnit::Emu, 635),
    (LengthUnit::Centimetre, LengthUnit::Millimetre, 10),
    (LengthUnit::Millimetre, LengthUnit::Emu, 36_000),
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn every_whole_count_of_every_unit_round_trips_exactly(count in any::<i32>(), unit in units(), rounding in roundings()) {
        let length = Blu::from_units(count, unit);
        prop_assert_eq!(length.to_units_exact(unit), Some(i64::from(count)));
        prop_assert_eq!(length.to_units(unit, rounding), i64::from(count));
        prop_assert_eq!(Blu::checked_from_units(i64::from(count), unit), Some(length));
    }

    #[test]
    fn a_whole_count_of_a_larger_unit_is_a_whole_count_of_a_smaller_one(count in any::<i32>(), pair in 0..MULTIPLES.len()) {
        let (larger, smaller, ratio) = MULTIPLES[pair];
        prop_assert_eq!(
            Blu::from_units(count, larger).to_units_exact(smaller),
            Some(i64::from(count) * ratio)
        );
    }

    #[test]
    fn rounding_to_a_unit_matches_the_reference(blu in any::<i64>(), unit in units(), rounding in roundings()) {
        let expected = reference::round(&BigInt::from(blu), &BigInt::from(unit.blu().0), rounding);
        prop_assert_eq!(BigInt::from(Blu(blu).to_units(unit, rounding)), expected);
        let exact = Blu(blu).to_units_exact(unit);
        prop_assert_eq!(exact.is_some(), blu % unit.blu().0 == 0);
    }

    #[test]
    fn large_counts_convert_exactly_or_not_at_all(count in any::<i64>(), unit in units()) {
        let product = BigInt::from(count) * BigInt::from(unit.blu().0);
        prop_assert_eq!(Blu::checked_from_units(count, unit).map(|length| BigInt::from(length.0)), i64::try_from(&product).ok().map(BigInt::from));
    }

    // "Arithmetic does not overflow for any realistic document dimension" (CORE-002): the sum of 64 groups of 1,563 lengths (100,032 in all), each as large as OOXML can write, fits, and so does any of them repeated 10,000 times.
    #[test]
    fn arithmetic_on_document_lengths_never_overflows(lengths in proptest::collection::vec(-LARGEST_COORDINATE..=LARGEST_COORDINATE, 64), factor in -10_000_i64..=10_000) {
        let mut total = Blu::ZERO;
        for &length in &lengths {
            let group = Blu(length).checked_mul(1_563);
            prop_assert!(group.is_some());
            let sum = total.checked_add(group.unwrap());
            prop_assert!(sum.is_some());
            total = sum.unwrap();
            prop_assert!(Blu(length).checked_mul(factor).is_some());
        }
    }

    // The font-unit conversion of CORE-003 and the Word measurement model: any advance (a 16-bit font value) at any font size Word allows (up to 1,638 pt) in any em size (16 to 16,384 units) converts.
    #[test]
    fn font_unit_conversions_never_overflow(advance in i64::from(i16::MIN)..=i64::from(u16::MAX), size in 1_i64..=1_638 * 25_400, units_per_em in 16_i64..=16_384, rounding in roundings()) {
        prop_assert!(Blu(size).scale(advance, units_per_em, rounding).is_some());
    }
}
