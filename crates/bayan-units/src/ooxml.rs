//! Reading the measurements of OOXML attribute values.
//!
//! Each function reads one attribute value and returns it, exactly, as one of this crate's types, or a [`ParseError`] if the text is not a measurement of that kind or its value does not fit. They follow the lexical rules of the OOXML schemas (ECMA-376 / ISO/IEC 29500) and refuse everything else; whether Word itself accepts other spellings is a question for the Fidelity Lab.
//!
//! - **Integer measurements** (twips, EMUs, half-points, eighths of a point, fiftieths and thousandths of a percent, sixty-thousandths of a degree) are XML Schema integers: an optional `+` or `-` sign and one or more ASCII digits, leading zeros allowed. As XML Schema prescribes for integers, spaces, tabs and line breaks around the number are ignored.
//! - **Universal measures** of Strict OOXML (`ST_UniversalMeasure`) are a number with an optional `-` sign, optional decimals and a unit: `12pt`, `1.5in`, `2cm`, `10mm`, `3pc` or `4pi` (both pica). They match the schema's pattern exactly, without surrounding spaces.
//!
//! The functions accept any value that fits in the result type. The narrower ranges of individual schema types (for example `ST_PositiveCoordinate` or `ST_PositiveFixedAngle`), and whether a negative value is allowed, are for the caller to check.
//!
//! Errors never repeat the text they were given, because attribute values are document content, which must not reach logs or error reports (AGENTS.md §6).

use core::fmt;

use crate::{Angle, Blu, LengthUnit, Percentage, Rounding};

/// Why a measurement could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParseError {
    /// The text is not a measurement of the expected kind.
    Malformed,
    /// The text is a well-formed measurement, but its value does not fit in the result type.
    OutOfRange,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParseError::Malformed => "malformed measurement",
            ParseError::OutOfRange => "measurement out of range",
        })
    }
}

impl core::error::Error for ParseError {}

/// Reads a length in twips (WordprocessingML's `ST_TwipsMeasure`, `ST_SignedTwipsMeasure` and others in integer form).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if the length does not fit in a [`Blu`].
pub fn parse_twips(text: &str) -> Result<Blu, ParseError> {
    parse_length(text, LengthUnit::Twip)
}

/// Reads a length in English Metric Units (DrawingML's `ST_Coordinate` and others in integer form).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if the length does not fit in a [`Blu`].
pub fn parse_emus(text: &str) -> Result<Blu, ParseError> {
    parse_length(text, LengthUnit::Emu)
}

/// Reads a length in half-points (WordprocessingML's `ST_HpsMeasure` and `ST_SignedHpsMeasure` in integer form, such as font sizes).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if the length does not fit in a [`Blu`].
pub fn parse_half_points(text: &str) -> Result<Blu, ParseError> {
    parse_length(text, LengthUnit::HalfPoint)
}

/// Reads a length in eighths of a point (WordprocessingML's `ST_EighthPointMeasure`, such as border widths).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if the length does not fit in a [`Blu`].
pub fn parse_eighth_points(text: &str) -> Result<Blu, ParseError> {
    parse_length(text, LengthUnit::EighthPoint)
}

/// Reads a percentage in fiftieths of a percent (WordprocessingML widths of type `pct`, where 5,000 is 100 %).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if the percentage does not fit in a [`Percentage`].
pub fn parse_fiftieths_of_a_percent(text: &str) -> Result<Percentage, ParseError> {
    let fiftieths = parse_integer(text)?;
    fiftieths
        .checked_mul(Percentage::from_fiftieths_of_a_percent(1).0)
        .map(Percentage)
        .ok_or(ParseError::OutOfRange)
}

/// Reads a percentage in thousandths of a percent (DrawingML's `ST_Percentage` and its relatives in integer form, where 100,000 is 100 %).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if it does not fit in an `i64`.
pub fn parse_thousandths_of_a_percent(text: &str) -> Result<Percentage, ParseError> {
    parse_integer(text).map(Percentage)
}

/// Reads an angle in sixty-thousandths of a degree (DrawingML's `ST_Angle` and its relatives).
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` is not an integer, [`ParseError::OutOfRange`] if it does not fit in an `i64`.
pub fn parse_angle(text: &str) -> Result<Angle, ParseError> {
    parse_integer(text).map(Angle)
}

/// Reads a universal measure of Strict OOXML (`ST_UniversalMeasure`), such as `1.5in`.
///
/// The result is exact, however many decimals the text has: `0.0001mm` is 7.2 BLU, not a whole number, and the result remembers that, so it can be rounded exactly once, into BLU or straight into the unit Word stores the attribute in (twips, for instance), with the rounding mode the Fidelity Lab finds Word uses.
///
/// # Errors
///
/// [`ParseError::Malformed`] if `text` does not match the schema's pattern, [`ParseError::OutOfRange`] if the length does not fit in a [`Blu`].
pub fn parse_universal_measure(text: &str) -> Result<UniversalMeasure, ParseError> {
    let bytes = text.as_bytes();
    let unit_starts = bytes.len().checked_sub(2).ok_or(ParseError::Malformed)?;
    let (number, unit) = bytes.split_at(unit_starts);
    let unit = match unit {
        b"mm" => LengthUnit::Millimetre,
        b"cm" => LengthUnit::Centimetre,
        b"in" => LengthUnit::Inch,
        b"pt" => LengthUnit::Point,
        b"pc" | b"pi" => LengthUnit::Pica,
        _ => return Err(ParseError::Malformed),
    };
    let (negative, number) = match number {
        [b'-', rest @ ..] => (true, rest),
        all => (false, all),
    };
    let (whole, decimals) = match number.iter().position(|&byte| byte == b'.') {
        // `point` is an index into `number`, so both slices are in bounds.
        Some(point) => (&number[..point], Some(&number[point + 1..])),
        None => (number, None),
    };
    if !is_digits(whole) || decimals.is_some_and(|decimals| !is_digits(decimals)) {
        return Err(ParseError::Malformed);
    }
    let size = unit.blu().0;
    let mut count: i64 = 0;
    for &byte in whole {
        count = count
            .checked_mul(10)
            .and_then(|count| count.checked_add(digit(byte)))
            .ok_or(ParseError::OutOfRange)?;
    }
    let whole_blu = count.checked_mul(size).ok_or(ParseError::OutOfRange)?;
    // The decimals times the unit's size, multiplied out digit by digit from the last, as on paper: `carry` ends as the whole BLU they make, and the digits written down along the way are the remaining fraction of a BLU, of which rounding needs only the first digit and whether any later one is not zero. The carry stays below `size`, so nothing overflows however many decimals the text has.
    let mut carry: i64 = 0;
    let mut first_digit = 0;
    let mut later_digits_nonzero = false;
    for (position, &byte) in decimals.unwrap_or_default().iter().enumerate().rev() {
        let product = digit(byte) * size + carry;
        let written = product % 10;
        carry = product / 10;
        if position == 0 {
            first_digit = written;
        } else {
            later_digits_nonzero |= written != 0;
        }
    }
    let fraction = match (first_digit, later_digits_nonzero) {
        (0, false) => Fraction::Zero,
        (0..=4, _) => Fraction::BelowHalf,
        (5, false) => Fraction::Half,
        _ => Fraction::AboveHalf,
    };
    // Keeping the magnitude below i64::MAX leaves room to round it up, whatever its sign.
    let magnitude = whole_blu
        .checked_add(carry)
        .filter(|&magnitude| magnitude < i64::MAX)
        .ok_or(ParseError::OutOfRange)?;
    Ok(match (negative, fraction) {
        (false, _) => UniversalMeasure {
            floor: magnitude,
            fraction,
        },
        (true, Fraction::Zero) => UniversalMeasure {
            floor: -magnitude,
            fraction,
        },
        // −(n + f) = (−n − 1) + (1 − f)
        (true, _) => UniversalMeasure {
            floor: -magnitude - 1,
            fraction: fraction.complement(),
        },
    })
}

/// A length read from a universal measure, kept exactly until it is rounded (see [`parse_universal_measure`]).
///
/// It stores the length as a whole number of BLU and the remaining fraction of a BLU, in as much detail as rounding needs: whether the fraction is zero, below a half, exactly a half or above a half. That suffices to round it, exactly and once, to a whole number of any [`LengthUnit`], all of which are whole numbers of BLU. It does not keep the exact value beyond that, so two different measures can look the same, and it deliberately implements no equality.
#[derive(Clone, Copy, Debug)]
pub struct UniversalMeasure {
    /// The largest whole number of BLU not above the length.
    floor: i64,
    /// What lies between `floor` and the length, less than one BLU.
    fraction: Fraction,
}

/// Where a value lies between two neighbouring integers, as far as rounding needs to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fraction {
    Zero,
    BelowHalf,
    Half,
    AboveHalf,
}

impl UniversalMeasure {
    /// The length rounded to a whole number of BLU as `rounding` says.
    #[must_use]
    pub fn to_blu(self, rounding: Rounding) -> Blu {
        Blu(round(self.floor, self.fraction, rounding))
    }

    /// The length if it is a whole number of BLU, or `None` otherwise.
    #[must_use]
    pub fn to_blu_exact(self) -> Option<Blu> {
        (self.fraction == Fraction::Zero).then_some(Blu(self.floor))
    }

    /// The length as a whole number of `unit`, rounded once as `rounding` says. Rounding straight into the unit avoids rounding twice (first to BLU, then to the unit), which could differ.
    #[must_use]
    pub fn to_units(self, unit: LengthUnit, rounding: Rounding) -> i64 {
        let (quotient, fraction) = self.divided_by(unit);
        round(quotient, fraction, rounding)
    }

    /// The length as a whole number of `unit`, or `None` if it is not one.
    #[must_use]
    pub fn to_units_exact(self, unit: LengthUnit) -> Option<i64> {
        let (quotient, fraction) = self.divided_by(unit);
        (fraction == Fraction::Zero).then_some(quotient)
    }

    /// The length divided by the size of `unit`: a whole number of units, and the fraction of a unit left over.
    fn divided_by(self, unit: LengthUnit) -> (i64, Fraction) {
        let size = unit.blu().0;
        let quotient = self.floor.div_euclid(size);
        // 0 ≤ remainder < size
        let remainder = self.floor.rem_euclid(size);
        if remainder == 0 && self.fraction == Fraction::Zero {
            return (quotient, Fraction::Zero);
        }
        // The fraction of a unit left over is (remainder + f) ÷ size, where f < 1 is this measure's fraction of a BLU. It is below a half exactly when 2 × remainder + 2f < size, which depends on f only when size − 2 × remainder is 0 or 1.
        let fraction = match size - 2 * remainder {
            2.. => Fraction::BelowHalf,
            1 => match self.fraction {
                Fraction::Zero | Fraction::BelowHalf => Fraction::BelowHalf,
                Fraction::Half => Fraction::Half,
                Fraction::AboveHalf => Fraction::AboveHalf,
            },
            0 => {
                if self.fraction == Fraction::Zero {
                    Fraction::Half
                } else {
                    Fraction::AboveHalf
                }
            }
            _ => Fraction::AboveHalf,
        };
        (quotient, fraction)
    }
}

impl Fraction {
    /// The fraction 1 − f, as far as rounding needs to know.
    const fn complement(self) -> Fraction {
        match self {
            Fraction::Zero => Fraction::Zero,
            Fraction::BelowHalf => Fraction::AboveHalf,
            Fraction::Half => Fraction::Half,
            Fraction::AboveHalf => Fraction::BelowHalf,
        }
    }
}

/// Rounds `floor + f`, where `f` is a fraction from 0 up to 1 that `fraction` describes, as `rounding` says. The caller guarantees that `floor + 1` fits.
fn round(floor: i64, fraction: Fraction, rounding: Rounding) -> i64 {
    if fraction == Fraction::Zero {
        return floor;
    }
    let ceiling = floor + 1;
    // floor + f is negative exactly when floor is.
    let negative = floor < 0;
    match rounding {
        Rounding::Floor => floor,
        Rounding::Ceiling => ceiling,
        Rounding::TowardZero => {
            if negative {
                ceiling
            } else {
                floor
            }
        }
        Rounding::HalfUp | Rounding::HalfEven | Rounding::HalfAwayFromZero => match fraction {
            Fraction::Zero | Fraction::BelowHalf => floor,
            Fraction::AboveHalf => ceiling,
            Fraction::Half => match rounding {
                Rounding::HalfEven if floor % 2 == 0 => floor,
                Rounding::HalfAwayFromZero if negative => floor,
                _ => ceiling,
            },
        },
    }
}

/// Reads an XML Schema integer: an optional sign and one or more ASCII digits, with XML white space around it ignored (the schema's `whiteSpace="collapse"`).
fn parse_integer(text: &str) -> Result<i64, ParseError> {
    let text = text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n' | '\r'));
    let (negative, digits) = match text.as_bytes() {
        [b'-', rest @ ..] => (true, rest),
        [b'+', rest @ ..] => (false, rest),
        all => (false, all),
    };
    if !is_digits(digits) {
        return Err(ParseError::Malformed);
    }
    // The value is built up as a negative number, which can reach i64::MIN, and negated at the end if it is positive.
    let mut value: i64 = 0;
    for &byte in digits {
        value = value
            .checked_mul(10)
            .and_then(|value| value.checked_sub(digit(byte)))
            .ok_or(ParseError::OutOfRange)?;
    }
    if negative {
        Ok(value)
    } else {
        value.checked_neg().ok_or(ParseError::OutOfRange)
    }
}

/// Reads an XML Schema integer as a count of `unit`.
fn parse_length(text: &str, unit: LengthUnit) -> Result<Blu, ParseError> {
    Blu::checked_from_units(parse_integer(text)?, unit).ok_or(ParseError::OutOfRange)
}

/// Whether `bytes` is one or more ASCII digits.
fn is_digits(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)
}

/// The value of an ASCII digit.
fn digit(byte: u8) -> i64 {
    i64::from(byte - b'0')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dpi;

    /// Text that no integer parser accepts.
    const NOT_INTEGERS: [&str; 22] = [
        "",
        " ",
        "\t\n",
        "7 20",
        "720pt",
        "7.2",
        "1e3",
        "0x10",
        "--1",
        "+-1",
        "-+1",
        "+",
        "-",
        "\u{663}\u{662}",   // Arabic-Indic digits ٣٢
        "\u{ff11}\u{ff12}", // fullwidth digits １２
        "720\u{a0}",        // a no-break space is not XML white space
        "\u{2003}720",      // nor is an em space
        "7\u{0}",
        "seven",
        "1_000",
        "1,000",
        "+ 1",
    ];

    #[test]
    fn reads_integer_lengths_exactly() {
        assert_eq!(parse_twips("720"), Ok(Blu(914_400)));
        assert_eq!(parse_twips("-720"), Ok(Blu(-914_400)));
        assert_eq!(parse_emus("914400"), Ok(Blu::INCH));
        assert_eq!(parse_emus("27273042329600"), Ok(Blu(54_546_084_659_200))); // the largest ST_Coordinate
        assert_eq!(parse_half_points("22"), Ok(Blu(279_400))); // 11 pt
        assert_eq!(parse_eighth_points("4"), Ok(Blu(12_700))); // ½ pt
        assert_eq!(parse_twips("0"), Ok(Blu::ZERO));
    }

    #[test]
    fn accepts_every_spelling_that_xml_schema_allows_for_integers() {
        for text in [
            "720",
            "+720",
            "000720",
            "+000720",
            " 720",
            "720 ",
            "\t720\n",
            "\r\n 720 \t",
        ] {
            assert_eq!(parse_twips(text), Ok(Blu(914_400)), "{text:?}");
        }
        assert_eq!(parse_twips("-0"), Ok(Blu::ZERO));
        assert_eq!(parse_twips("+0"), Ok(Blu::ZERO));
    }

    #[test]
    fn refuses_text_that_is_not_an_integer() {
        for text in NOT_INTEGERS {
            assert_eq!(parse_twips(text), Err(ParseError::Malformed), "{text:?}");
            assert_eq!(parse_emus(text), Err(ParseError::Malformed), "{text:?}");
            assert_eq!(
                parse_half_points(text),
                Err(ParseError::Malformed),
                "{text:?}"
            );
            assert_eq!(
                parse_eighth_points(text),
                Err(ParseError::Malformed),
                "{text:?}"
            );
            assert_eq!(
                parse_fiftieths_of_a_percent(text),
                Err(ParseError::Malformed),
                "{text:?}"
            );
            assert_eq!(
                parse_thousandths_of_a_percent(text),
                Err(ParseError::Malformed),
                "{text:?}"
            );
            assert_eq!(parse_angle(text), Err(ParseError::Malformed), "{text:?}");
        }
    }

    #[test]
    fn refuses_integers_that_do_not_fit() {
        // i64::MAX ÷ 1,270 = 7,262,497,666,814,784.1
        assert_eq!(
            parse_twips("7262497666814784"),
            Ok(Blu(9_223_372_036_854_775_680))
        );
        assert_eq!(parse_twips("7262497666814785"), Err(ParseError::OutOfRange));
        assert_eq!(
            parse_twips("-7262497666814785"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(
            parse_emus("4611686018427387904"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(parse_emus("-4611686018427387904"), Ok(Blu::MIN));
        assert_eq!(
            parse_half_points("726249766681479"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(
            parse_eighth_points("2904999066725914"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(
            parse_twips("99999999999999999999999999"),
            Err(ParseError::OutOfRange)
        );
        // Leading zeros do not count against the range.
        assert_eq!(
            parse_twips("0000000000000000000000000000000000000000001"),
            Ok(Blu(1_270))
        );
    }

    #[test]
    fn reads_percentages_in_both_units() {
        assert_eq!(
            parse_fiftieths_of_a_percent("5000"),
            Ok(Percentage::HUNDRED)
        );
        assert_eq!(
            parse_fiftieths_of_a_percent("-2500"),
            Ok(Percentage(-50_000))
        );
        assert_eq!(
            parse_thousandths_of_a_percent("100000"),
            Ok(Percentage::HUNDRED)
        );
        assert_eq!(parse_thousandths_of_a_percent("-1"), Ok(Percentage(-1)));
        // i64::MAX ÷ 20 = 461,168,601,842,738,790.35
        assert_eq!(
            parse_fiftieths_of_a_percent("461168601842738790"),
            Ok(Percentage(9_223_372_036_854_775_800))
        );
        assert_eq!(
            parse_fiftieths_of_a_percent("461168601842738791"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(
            parse_thousandths_of_a_percent("9223372036854775808"),
            Err(ParseError::OutOfRange)
        );
    }

    #[test]
    fn reads_angles() {
        assert_eq!(parse_angle("5400000"), Ok(Angle::RIGHT_ANGLE));
        assert_eq!(parse_angle("-60000"), Ok(-Angle::DEGREE));
        assert_eq!(parse_angle("-9223372036854775808"), Ok(Angle(i64::MIN)));
        assert_eq!(parse_angle("9223372036854775807"), Ok(Angle(i64::MAX)));
        assert_eq!(
            parse_angle("9223372036854775808"),
            Err(ParseError::OutOfRange)
        );
        assert_eq!(
            parse_angle("-9223372036854775809"),
            Err(ParseError::OutOfRange)
        );
    }

    /// The universal measure `text`, which must be valid.
    fn measure(text: &str) -> UniversalMeasure {
        parse_universal_measure(text).expect("a valid universal measure")
    }

    #[test]
    fn reads_the_universal_measures_of_the_brief_exactly() {
        let cases = [
            ("12pt", 304_800),
            ("1.5in", 2_743_200),
            ("2cm", 1_440_000),
            ("10mm", 720_000),
            ("3pc", 914_400),
            ("4pi", 1_219_200),
            ("-0.5pt", -12_700),
            ("0.001mm", 72),
            ("0.25in", 457_200),
            ("007.50pt", 190_500),
            ("0pt", 0),
            ("-0pt", 0),
            ("-0.000mm", 0),
            (
                "1.000000000000000000000000000000000000000000000000in",
                1_828_800,
            ),
        ];
        for (text, blu) in cases {
            assert_eq!(measure(text).to_blu_exact(), Some(Blu(blu)), "{text}");
            for rounding in Rounding::ALL {
                assert_eq!(
                    measure(text).to_blu(rounding),
                    Blu(blu),
                    "{text} {rounding:?}"
                );
            }
        }
    }

    /// The expected results in the order of [`Rounding::ALL`]: floor, ceiling, toward zero, half up, half even, half away from zero. Each was computed with Python's exact fractions.
    fn check_blu(text: &str, expected: [i64; 6]) {
        assert_eq!(measure(text).to_blu_exact(), None, "{text}");
        for (rounding, want) in Rounding::ALL.into_iter().zip(expected) {
            assert_eq!(
                measure(text).to_blu(rounding),
                Blu(want),
                "{text} {rounding:?}"
            );
        }
    }

    fn check_units(text: &str, unit: LengthUnit, expected: [i64; 6]) {
        for (rounding, want) in Rounding::ALL.into_iter().zip(expected) {
            assert_eq!(
                measure(text).to_units(unit, rounding),
                want,
                "{text} in {unit:?}, {rounding:?}"
            );
        }
    }

    #[test]
    fn rounds_measures_that_are_not_whole_blu_once_and_as_asked() {
        check_blu("0.0001mm", [7, 8, 7, 7, 7, 7]); // 7.2 BLU
        check_blu("-0.0001mm", [-8, -7, -7, -7, -7, -7]);
        check_blu(
            "1.001in",
            [
                1_830_628, 1_830_629, 1_830_628, 1_830_629, 1_830_629, 1_830_629,
            ],
        ); // 1,830,628.8 BLU
        check_blu(
            "-1.001in",
            [
                -1_830_629, -1_830_628, -1_830_628, -1_830_629, -1_830_629, -1_830_629,
            ],
        );
        check_blu("0.00000625cm", [4, 5, 4, 5, 4, 5]); // exactly 4.5 BLU
        check_blu("-0.00000625cm", [-5, -4, -4, -4, -4, -5]);
    }

    #[test]
    fn rounds_straight_into_other_units() {
        check_units(
            "1.001in",
            LengthUnit::Twip,
            [1_441, 1_442, 1_441, 1_441, 1_441, 1_441],
        ); // 1,441.44
        check_units(
            "1.001in",
            LengthUnit::Emu,
            [915_314, 915_315, 915_314, 915_314, 915_314, 915_314],
        ); // 915,314.4
        check_units("0.025pt", LengthUnit::Twip, [0, 1, 0, 1, 0, 1]); // exactly half a twip
        check_units("-0.025pt", LengthUnit::Twip, [-1, 0, 0, 0, 0, -1]);
        // A hair above and below half a twip, far beyond the precision of a 64-bit float.
        check_units(
            "0.0250000000000000000001pt",
            LengthUnit::Twip,
            [0, 1, 0, 1, 1, 1],
        );
        check_units(
            "0.0249999999999999999999pt",
            LengthUnit::Twip,
            [0, 1, 0, 0, 0, 0],
        );
        check_units("1.5in", LengthUnit::Dot(Dpi::DPI_96), [144; 6]);
        assert_eq!(measure("0.025pt").to_units_exact(LengthUnit::Twip), None);
        assert_eq!(
            measure("1.5in").to_units_exact(LengthUnit::Twip),
            Some(2_160)
        );
        assert_eq!(measure("0.0001mm").to_units_exact(LengthUnit::Emu), None);
    }

    #[test]
    fn refuses_text_that_does_not_match_the_pattern() {
        let malformed = [
            "",
            "pt",
            "12",
            "12 pt",
            "12PT",
            "12Pt",
            " 12pt",
            "12pt ",
            ".5pt",
            "5.pt",
            "+5pt",
            "1e2pt",
            "1,5pt",
            "5px",
            "5em",
            "5%",
            "-pt",
            "-.5pt",
            "--5pt",
            "5.5.5pt",
            "\u{ff15}pt",
            "5pt\u{0}",
            "5ptt",
            "5 mm",
            "0x5pt",
            "- 5pt",
            "5\u{a0}pt",
            "5p",
            "t",
            "-",
            "5.-5pt",
            "\u{663}pt",
        ];
        for text in malformed {
            assert!(
                matches!(parse_universal_measure(text), Err(ParseError::Malformed)),
                "{text:?}"
            );
        }
    }

    #[test]
    fn refuses_measures_that_do_not_fit() {
        // i64::MAX ÷ 1,828,800 = 5,043,401,157,510.27
        assert_eq!(
            measure("5043401157510in").to_blu_exact(),
            Some(Blu(9_223_372_036_854_288_000))
        );
        assert_eq!(
            measure("5043401157510.2in").to_blu(Rounding::Ceiling),
            Blu(9_223_372_036_854_653_760)
        );
        assert!(matches!(
            parse_universal_measure("5043401157510.3in"),
            Err(ParseError::OutOfRange)
        ));
        assert!(matches!(
            parse_universal_measure("-5043401157510.3in"),
            Err(ParseError::OutOfRange)
        ));
        assert!(matches!(
            parse_universal_measure("5043401157511in"),
            Err(ParseError::OutOfRange)
        ));
        assert!(matches!(
            parse_universal_measure("99999999999999999999999pt"),
            Err(ParseError::OutOfRange)
        ));
    }

    #[test]
    fn errors_say_what_went_wrong_without_repeating_the_text() {
        assert_eq!(ParseError::Malformed.to_string(), "malformed measurement");
        assert_eq!(
            ParseError::OutOfRange.to_string(),
            "measurement out of range"
        );
    }
}
