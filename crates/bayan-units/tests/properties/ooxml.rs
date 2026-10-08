//! The OOXML parsers against the big-integer reference and an independent reading of the schema's patterns.

use bayan_units::ooxml::{
    ParseError, parse_angle, parse_eighth_points, parse_emus, parse_fiftieths_of_a_percent,
    parse_half_points, parse_thousandths_of_a_percent, parse_twips, parse_universal_measure,
};
use bayan_units::{Blu, Dpi, LengthUnit, Rounding};
use num_bigint::BigInt;
use proptest::prelude::*;

use crate::reference;

/// A parser of integer lengths.
type LengthParser = fn(&str) -> Result<Blu, ParseError>;

/// The units of a universal measure, with their spelling.
const UNIVERSAL_UNITS: [(&str, LengthUnit); 6] = [
    ("mm", LengthUnit::Millimetre),
    ("cm", LengthUnit::Centimetre),
    ("in", LengthUnit::Inch),
    ("pt", LengthUnit::Point),
    ("pc", LengthUnit::Pica),
    ("pi", LengthUnit::Pica),
];

/// Units to round universal measures into.
fn target_units() -> impl Strategy<Value = LengthUnit> {
    proptest::sample::select(vec![
        LengthUnit::Twip,
        LengthUnit::Emu,
        LengthUnit::HalfPoint,
        LengthUnit::EighthPoint,
        LengthUnit::Point,
        LengthUnit::Inch,
        LengthUnit::Millimetre,
        LengthUnit::Dot(Dpi::DPI_96),
        LengthUnit::Dot(Dpi::new(1_828_800).expect("divides an inch")),
    ])
}

fn roundings() -> impl Strategy<Value = Rounding> {
    proptest::sample::select(Rounding::ALL.to_vec())
}

/// Up to three characters of XML white space.
fn white_space() -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(vec![' ', '\t', '\r', '\n']), 0..=3)
        .prop_map(|characters| characters.into_iter().collect())
}

/// Whether `text` matches `-?[0-9]+(\.[0-9]+)?(mm|cm|in|pt|pc|pi)`, the pattern of ST_UniversalMeasure, checked character by character.
fn matches_universal_measure(text: &str) -> bool {
    let Some(number) = UNIVERSAL_UNITS
        .iter()
        .find_map(|(unit, _)| text.strip_suffix(unit))
    else {
        return false;
    };
    let number = number.strip_prefix('-').unwrap_or(number);
    let mut parts = number.split('.');
    let digits = |part: Option<&str>| {
        part.is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    };
    let whole = parts.next();
    let decimals = parts.next();
    digits(whole) && (decimals.is_none() || digits(decimals)) && parts.next().is_none()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1024))]

    #[test]
    fn integer_lengths_are_read_exactly_or_refused_as_out_of_range(value in any::<i64>()) {
        let text = value.to_string();
        let parsers: [(LengthParser, LengthUnit); 4] = [
            (parse_twips, LengthUnit::Twip),
            (parse_emus, LengthUnit::Emu),
            (parse_half_points, LengthUnit::HalfPoint),
            (parse_eighth_points, LengthUnit::EighthPoint),
        ];
        for (parse, unit) in parsers {
            let product = BigInt::from(value) * BigInt::from(unit.blu().0);
            let expected = i64::try_from(&product).map(Blu).map_err(|_| ParseError::OutOfRange);
            prop_assert_eq!(parse(&text), expected, "{} in {:?}", text, unit);
        }
        prop_assert_eq!(parse_angle(&text).map(|angle| angle.0), Ok(value));
        prop_assert_eq!(parse_thousandths_of_a_percent(&text).map(|percentage| percentage.0), Ok(value));
        let fiftieths = i64::try_from(&(BigInt::from(value) * 20)).map_err(|_| ParseError::OutOfRange);
        prop_assert_eq!(parse_fiftieths_of_a_percent(&text).map(|percentage| percentage.0), fiftieths);
    }

    #[test]
    fn integers_may_carry_a_plus_sign_leading_zeros_and_surrounding_white_space(value in 0_i64..1_000_000_000, zeros in 0_usize..30, before in white_space(), after in white_space()) {
        let text = format!("{before}+{}{value}{after}", "0".repeat(zeros));
        prop_assert_eq!(parse_angle(&text).map(|angle| angle.0), Ok(value));
    }

    #[test]
    fn nothing_but_integers_is_read_as_one(text in proptest::collection::vec(proptest::sample::select(vec!['0', '1', '9', '-', '+', ' ', '.', 'e', '\u{a0}', '\u{663}', '\t']), 0..12)) {
        let text: String = text.into_iter().collect();
        let trimmed = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
        let unsigned = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed);
        let is_integer = !unsigned.is_empty() && unsigned.chars().all(|c| c.is_ascii_digit());
        prop_assert_eq!(parse_angle(&text).is_ok(), is_integer, "{:?}", text);
    }

    #[test]
    fn universal_measures_are_read_exactly(negative in any::<bool>(), whole in 0_u64..10_000_000_000, decimals in proptest::collection::vec(0_u8..10, 0..40), unit in 0..UNIVERSAL_UNITS.len(), rounding in roundings(), target in target_units()) {
        let (suffix, unit) = UNIVERSAL_UNITS[unit];
        let decimals: String = decimals.iter().map(|digit| char::from(b'0' + digit)).collect();
        let text = format!("{}{whole}{}{decimals}{suffix}", if negative { "-" } else { "" }, if decimals.is_empty() { "" } else { "." });
        let measure = parse_universal_measure(&text).expect("valid and in range");
        // The exact value in BLU is ±(whole × 10^s + decimals) × size ÷ 10^s.
        let scale = BigInt::from(10).pow(u32::try_from(decimals.len()).expect("short"));
        let digits = if decimals.is_empty() { BigInt::from(0) } else { decimals.parse::<BigInt>().expect("digits") };
        let mut numerator = (BigInt::from(whole) * &scale + digits) * BigInt::from(unit.blu().0);
        if negative {
            numerator = -numerator;
        }
        let blu = reference::round(&numerator, &scale, rounding);
        prop_assert_eq!(BigInt::from(measure.to_blu(rounding).0), blu, "{} {:?}", text, rounding);
        let exact = &numerator % &scale == BigInt::from(0);
        prop_assert_eq!(measure.to_blu_exact().is_some(), exact, "{}", text);
        let in_units = reference::round(&numerator, &(&scale * BigInt::from(target.blu().0)), rounding);
        prop_assert_eq!(BigInt::from(measure.to_units(target, rounding)), in_units, "{} in {:?}, {:?}", text, target, rounding);
        let exact_in_units = &numerator % (&scale * BigInt::from(target.blu().0)) == BigInt::from(0);
        prop_assert_eq!(measure.to_units_exact(target).is_some(), exact_in_units, "{} in {:?}", text, target);
    }

    // Random decimals almost never land exactly halfway, so these are built to: (2j + 1) × 0.0000625 mm is (2j + 1) × 4.5 BLU, and (2j + 1) × 0.025 pt is (2j + 1) × ½ twip.
    #[test]
    fn halfway_cases_round_as_each_mode_says(j in 0_i64..1_000_000_000, negative in any::<bool>(), rounding in roundings()) {
        let odd = 2 * j + 1;
        let sign = if negative { -1 } else { 1 };
        let decimal = |numerator: i64, digits: u32| {
            let scale = 10_i64.pow(digits);
            format!("{}{}.{:0width$}", if negative { "-" } else { "" }, numerator / scale, numerator % scale, width = digits as usize)
        };
        let millimetres = decimal(odd * 625, 7);
        let blu = parse_universal_measure(&format!("{millimetres}mm")).expect("valid").to_blu(rounding);
        prop_assert_eq!(BigInt::from(blu.0), reference::round(&BigInt::from(sign * odd * 9), &BigInt::from(2), rounding), "{}mm {:?}", millimetres, rounding);
        let points = decimal(odd * 25, 3);
        let twips = parse_universal_measure(&format!("{points}pt")).expect("valid").to_units(LengthUnit::Twip, rounding);
        prop_assert_eq!(BigInt::from(twips), reference::round(&BigInt::from(sign * odd), &BigInt::from(2), rounding), "{}pt {:?}", points, rounding);
    }

    #[test]
    fn universal_measures_near_the_limit_are_read_or_refused_as_out_of_range(whole in 5_043_401_157_000_u64..5_043_401_158_000, decimals in proptest::collection::vec(0_u8..10, 1..=3), negative in any::<bool>()) {
        let decimals: String = decimals.iter().map(|digit| char::from(b'0' + digit)).collect();
        let text = format!("{}{whole}.{decimals}in", if negative { "-" } else { "" });
        let scale = BigInt::from(10).pow(u32::try_from(decimals.len()).expect("short"));
        let magnitude = (BigInt::from(whole) * &scale + decimals.parse::<BigInt>().expect("digits")) * BigInt::from(1_828_800);
        // The parser refuses lengths whose whole number of BLU is i64::MAX or more, to leave room for rounding up.
        let fits = magnitude / scale < BigInt::from(i64::MAX);
        let result = parse_universal_measure(&text);
        prop_assert_eq!(result.is_ok(), fits, "{}", text);
        if !fits {
            prop_assert!(matches!(result, Err(ParseError::OutOfRange)));
        }
    }

    #[test]
    fn exactly_the_texts_that_match_the_pattern_are_read(text in proptest::collection::vec(proptest::sample::select(vec!['0', '7', '.', '-', '+', ' ', 'e', 'm', 'c', 'i', 'n', 'p', 't', 'M', 'P', '\u{ff17}']), 0..9)) {
        let text: String = text.into_iter().collect();
        let result = parse_universal_measure(&text);
        prop_assert_eq!(result.is_ok(), matches_universal_measure(&text), "{:?}", text);
        if !matches_universal_measure(&text) {
            prop_assert!(matches!(result, Err(ParseError::Malformed)), "{:?}", text);
        }
    }

    #[test]
    fn any_text_at_all_is_read_or_refused_without_panicking(text in proptest::collection::vec(any::<char>(), 0..40)) {
        let text: String = text.into_iter().collect();
        let _ = parse_universal_measure(&text);
        let _ = parse_twips(&text);
        let _ = parse_angle(&text);
        let _ = parse_fiftieths_of_a_percent(&text);
    }
}
