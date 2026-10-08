//! Transforms on realistic page coordinates: exact where the mathematics is exact, within one BLU of it elsewhere.

use bayan_units::{Angle, Blu, Fixed, Point, Rounding, Transform};
use proptest::prelude::*;

/// The largest page Word allows is 22 inches; drawings may extend a little beyond it.
const PAGE: i64 = 2 * 22 * 1_828_800;

fn points() -> impl Strategy<Value = Point> {
    (-PAGE..=PAGE, -PAGE..=PAGE).prop_map(|(x, y)| Point::new(Blu(x), Blu(y)))
}

fn roundings() -> impl Strategy<Value = Rounding> {
    proptest::sample::select(Rounding::ALL.to_vec())
}

/// Whether two points are at most `tolerance` BLU apart on each axis.
fn near(a: Point, b: Point, tolerance: i64) -> bool {
    (a.x.0 - b.x.0).abs() <= tolerance && (a.y.0 - b.y.0).abs() <= tolerance
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn quarter_turns_move_points_exactly(x in -(1_i64 << 62)..(1 << 62), y in -(1_i64 << 62)..(1 << 62), quarter_turns in -8_i64..=8, rounding in roundings()) {
        let turned = Transform::rotation(Angle(quarter_turns * 5_400_000)).map_point(Point::new(Blu(x), Blu(y)), rounding);
        let expected = match quarter_turns.rem_euclid(4) {
            0 => (x, y),
            1 => (-y, x),
            2 => (-x, -y),
            _ => (y, -x),
        };
        prop_assert_eq!(turned, Some(Point::new(Blu(expected.0), Blu(expected.1))));
    }

    #[test]
    fn translations_and_whole_scalings_are_exact(point in points(), dx in -PAGE..=PAGE, dy in -PAGE..=PAGE, sx in -1_000_i32..=1_000, sy in -1_000_i32..=1_000, rounding in roundings()) {
        prop_assert_eq!(Transform::translation(Blu(dx), Blu(dy)).map_point(point, rounding), Some(Point::new(point.x + Blu(dx), point.y + Blu(dy))));
        prop_assert_eq!(
            Transform::scaling(Fixed::from_integer(sx), Fixed::from_integer(sy)).map_point(point, rounding),
            Some(Point::new(point.x * i64::from(sx), point.y * i64::from(sy)))
        );
    }

    #[test]
    fn turning_back_returns_within_one_blu(point in points(), units in -21_600_000_i64..=21_600_000, rounding in roundings()) {
        let forth = Transform::rotation(Angle(units));
        let back = Transform::rotation(Angle(-units));
        let there = forth.map_point(point, Rounding::HalfEven).unwrap();
        let again = back.map_point(there, Rounding::HalfEven).unwrap();
        prop_assert!(near(again, point, 1), "{:?} → {:?} → {:?}", point, there, again);
        // Composed into one transform, the two turns are the identity to within the rounding of its coefficients.
        let round_trip = forth.then(back, rounding).unwrap().map_point(point, Rounding::HalfEven).unwrap();
        prop_assert!(near(round_trip, point, 1), "{:?} → {:?}", point, round_trip);
    }

    #[test]
    fn rotations_keep_the_distance_from_the_origin(point in points(), units in any::<i64>()) {
        let turned = Transform::rotation(Angle(units)).map_point(point, Rounding::HalfEven).unwrap();
        let square = |p: Point| i128::from(p.x.0) * i128::from(p.x.0) + i128::from(p.y.0) * i128::from(p.y.0);
        // Rounding each coordinate moves the point by at most half a BLU on each axis, and the coefficients add at most 2^-33 relative error, so the turned point is less than 0.75 BLU from the exact one, and the squared distance from the origin changes by less than 2r + 2, where r is the distance. In integers: (difference − 2)² ≤ 4r².
        let difference = (square(turned) - square(point)).abs();
        prop_assert!(difference <= 2 || (difference - 2) * (difference - 2) <= 4 * square(point), "{:?} → {:?}", point, turned);
    }

    #[test]
    fn a_composed_transform_maps_like_its_parts_in_sequence(point in points(), units in any::<i64>(), sx in -1_000_000_i64..=1_000_000, dx in -PAGE..=PAGE, dy in -PAGE..=PAGE) {
        let rounding = Rounding::HalfEven;
        let scale = Transform::scaling(Fixed::from_ratio(sx, 100_000, rounding).unwrap(), Fixed::ONE);
        let turn = Transform::rotation(Angle(units));
        let shift = Transform::translation(Blu(dx), Blu(dy));
        let composed = scale.then(turn, rounding).unwrap().then(shift, rounding).unwrap();
        let in_sequence = [scale, turn, shift]
            .into_iter()
            .try_fold(point, |p, t| t.map_point(p, rounding))
            .unwrap();
        // Each step of the sequence rounds once (up to 1.5 BLU in all); the composed coefficients are off by a few 2^-32, which at |x| ≤ 10 × PAGE is a few hundredths of a BLU.
        prop_assert!(near(composed.map_point(point, rounding).unwrap(), in_sequence, 2), "{:?}", point);
    }
}
