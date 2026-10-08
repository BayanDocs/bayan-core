//! # bayan-units
//!
//! Integer layout units, geometry, unit conversions, rounding and deterministic math: the foundation every other crate of the engine measures with ([ADR-0005]).
//!
//! ## The Bayan Layout Unit
//!
//! All layout geometry in BayanDocs is measured in **Bayan Layout Units** (BLU), stored as integers ([`Blu`]): 1 BLU = 1/1,828,800 inch = 1/25,400 point. Every unit OOXML and Word use is a whole number of BLU, so a length read from a document is stored exactly, and layout arithmetic is exact integer arithmetic that gives the same result on every platform (ADR-0004, ADR-0005):
//!
//! | Unit ([`LengthUnit`]) | BLU | Unit | BLU |
//! |---|---|---|---|
//! | inch | 1,828,800 | point | 25,400 |
//! | twip (1/20 point) | 1,270 | half-point | 12,700 |
//! | EMU (1/914,400 inch) | 2 | eighth of a point | 3,175 |
//! | millimetre | 72,000 | centimetre | 720,000 |
//! | pixel at 96 dpi | 19,050 | pixel at 72 dpi | 25,400 |
//! | dot at 300 / 600 / 1,200 dpi | 6,096 / 3,048 / 1,524 | dot at 144 dpi | 12,700 |
//! | pica (12 points) | 304,800 | | |
//!
//! An `i64` of BLU reaches about ±128 million kilometres. DrawingML limits its coordinates to about ±757 km, but the schema sets no upper limit for WordprocessingML's measurements in twips or for universal measures, so a hostile document can hold lengths near the limits of the type (see [Overflow](#overflow)).
//!
//! ## Rounding is always explicit
//!
//! Wherever a result is not a whole number, the caller says how to round it with a [`Rounding`] mode: floor, ceiling, toward zero, or to the nearest integer with halfway cases going up, to the even neighbour, or away from zero. [`scale`] computes `value × numerator ÷ denominator` exactly in 128-bit integers and rounds once; it is how font units become BLU. Which mode reproduces Word at each step is for the Fidelity Lab to find out, not to guess (ADR-0005 §2). For that reason [`Blu`] has no `/` operator, which would round toward zero without saying so.
//!
//! ## What is here
//!
//! - [`Blu`], with exact constructors for every unit ([`Blu::from_twips`], [`Blu::from_units`], …), conversion back ([`Blu::to_units`], [`Blu::to_units_exact`]), arithmetic operators with `checked_` and `saturating_` variants, ordering, hashing, `Display` and serde support. [`LengthUnit`] and [`Dpi`] describe the units.
//! - [`Rounding`] and [`scale`].
//! - [`Fixed`], a dimensionless fixed-point number with 32 binary fraction digits, for scale factors and transforms.
//! - [`Angle`], in sixty-thousandths of a degree like DrawingML, with [`Angle::sin`] and [`Angle::cos`] computed in integer arithmetic: the same bits on every platform, and correctly rounded.
//! - [`Percentage`], in thousandths of a percent, which holds both of OOXML's percentage units exactly.
//! - Geometry: [`Point`], [`Size`], [`Rect`], [`Insets`] and the fixed-point affine [`Transform`] for rotating and scaling drawing objects.
//! - [`ooxml`]: reading the measurements of OOXML attributes (twips, EMUs, half-points, eighths of a point, both percentage units, angles and Strict OOXML's universal measures such as `"1.5in"`), refusing anything malformed.
//! - [`Blu::to_f64_lossy`], the one conversion to floating point, for display and debugging only. Nothing else in this crate uses floating point.
//!
//! ```
//! use bayan_units::ooxml::{parse_twips, parse_universal_measure};
//! use bayan_units::{scale, Angle, Blu, LengthUnit, Point, Rounding, Transform};
//!
//! // A WordprocessingML indent of 720 twips is half an inch, exactly.
//! let indent = parse_twips("720")?;
//! assert_eq!(indent, Blu::from_inches(1).scale(1, 2, Rounding::Floor).unwrap());
//!
//! // Strict OOXML may write the same length as a universal measure.
//! let strict = parse_universal_measure("0.5in")?;
//! assert_eq!(strict.to_blu_exact(), Some(indent));
//! assert_eq!(strict.to_units(LengthUnit::Twip, Rounding::HalfEven), 720);
//!
//! // A glyph advance of 1,229 font units in a 2,048-unit em at 11 points.
//! let advance = scale(1_229, Blu::from_half_points(22).0, 2_048, Rounding::HalfEven);
//! assert_eq!(advance, Some(167_667));
//!
//! // Turning a point a quarter turn clockwise on the page.
//! let turned = Transform::rotation(Angle::from_degrees(90))
//!     .map_point(Point::new(Blu::INCH, Blu::ZERO), Rounding::HalfEven);
//! assert_eq!(turned, Some(Point::new(Blu::ZERO, Blu::INCH)));
//! # Ok::<(), bayan_units::ooxml::ParseError>(())
//! ```
//!
//! ## Overflow
//!
//! Operators such as `+` behave like those of `i64`: they panic on overflow in builds with overflow checks (debug builds, by default) and wrap around otherwise. Values that come from a document can be anything that fits, so code that handles them uses the `checked_` and `saturating_` methods of [`Blu`], or first brings them into the range Word itself accepts; the geometry types compute with the operators and have no checked methods of their own yet. The parsers in [`ooxml`] refuse values that do not fit, and the methods that round return `None` when a result does not fit.
//!
//! ## Layer
//!
//! bayan-units belongs to the **Foundation** layer.
//!
//! bayan-core's crates are arranged in layers, from lowest to highest: Foundation → Model and formats → Text → Layout → Output → Interaction → Engine → Bindings and tools. **A crate may depend only on crates in its own layer or below; lower layers never know about higher ones.** This keeps the engine testable and lets the server reuse the lower crates it needs without the layout engine ([architecture §4][architecture]). bayan-units may therefore depend only on other Foundation crates and on external libraries.
//!
//! [ADR-0005]: https://github.com/BayanDocs/docs/blob/HEAD/adr/0005-layout-units-and-deterministic-math.md
//! [architecture]: https://github.com/BayanDocs/docs/blob/HEAD/plan/03-architecture.md#4-inside-bayan-core

#![forbid(unsafe_code)]

mod angle;
mod blu;
mod fixed;
mod geometry;
mod lossy;
pub mod ooxml;
mod percentage;
mod rounding;
mod transform;
mod trig;
mod unit;

pub use angle::Angle;
pub use blu::Blu;
pub use fixed::Fixed;
pub use geometry::{Insets, Point, Rect, Size};
pub use percentage::Percentage;
pub use rounding::{Rounding, scale};
pub use transform::Transform;
pub use unit::{Dpi, LengthUnit};
