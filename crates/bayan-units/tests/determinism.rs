//! The fixed test vector of work package CORE-002, acceptance criterion 4: bayan-units' deterministic math produces the same bits on every platform.
//!
//! Each test computes a fixed set of results, folds their bits into a fingerprint, prints it and compares it with the fingerprint committed below. `cargo xtask verify` runs these tests on the host and, under Node.js, in WebAssembly, and its `determinism` step prints the fingerprints of both; CI runs the gate on Linux x86-64, Windows x86-64 and macOS arm64. Every run that passes produced exactly the committed bits.
//!
//! The fingerprints were recorded from this crate's own output; whether those results are *right* is what the other tests check, against independent high-precision references. A fingerprint that changes after a deliberate change to the math must be updated here, and the pull request must say so: layouts that use the math change with it (ADR-0004 §4, layout epochs). If the sine and cosine fingerprint changes, also run the exhaustive check that every sine and cosine is still correctly rounded, `cargo test -p bayan-units --release --test properties -- --ignored` (about 20 seconds), and report its result.

use bayan_units::{Angle, Blu, Fixed, Point, Rounding, Transform, scale};

/// The 64-bit FNV-1a hash: a few lines, defined bit for bit, so a fingerprint means the same on every platform.
struct Fingerprint(u64);

impl Fingerprint {
    fn new() -> Fingerprint {
        Fingerprint(0xcbf2_9ce4_8422_2325)
    }

    fn add(&mut self, value: i64) {
        for byte in value.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// A fixed sequence of pseudo-random 64-bit values (xorshift64*), the same on every platform.
struct Sequence(u64);

impl Sequence {
    fn next(&mut self) -> i64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d).cast_signed()
    }
}

/// Every degree of two whole turns, the neighbours of every multiple of 45°, the extremes of the range, and 2,000 angles from the whole range.
fn angles() -> Vec<Angle> {
    let mut angles: Vec<i64> = (-360..=360).map(|degrees| degrees * 60_000).collect();
    for eighths in -8..=8 {
        for offset in [-2, -1, 1, 2] {
            angles.push(eighths * 2_700_000 + offset);
        }
    }
    angles.extend([i64::MIN, i64::MIN + 1, i64::MAX - 1, i64::MAX]);
    let mut sequence = Sequence(0x9e37_79b9_7f4a_7c15);
    angles.extend((0..2_000).map(|_| sequence.next()));
    angles.into_iter().map(Angle).collect()
}

/// Compares a fingerprint with the committed one, with a message that shows both.
fn check(what: &str, fingerprint: &Fingerprint, committed: u64) {
    assert_eq!(
        fingerprint.0, committed,
        "{what} changed: fingerprint {:016x}, committed {committed:016x}",
        fingerprint.0
    );
}

#[test]
fn sine_and_cosine_give_the_committed_bits() {
    let angles = angles();
    let mut fingerprint = Fingerprint::new();
    for angle in &angles {
        let (sin, cos) = angle.sin_cos();
        fingerprint.add(sin.0);
        fingerprint.add(cos.0);
    }
    // `cargo xtask verify` shows this line from the host and from WebAssembly.
    println!(
        "fingerprint of sine and cosine ({} values): {:016x}",
        2 * angles.len(),
        fingerprint.0
    );
    check("sine and cosine", &fingerprint, 0xc08d_66dd_df6e_6867);
}

#[test]
fn scaling_gives_the_committed_bits() {
    let mut sequence = Sequence(0x0123_4567_89ab_cdef);
    let mut fingerprint = Fingerprint::new();
    let mut count = 0;
    for _ in 0..1_000 {
        // Full-range values, and small ones, whose results are often exact or halfway.
        let (value, numerator, denominator) = (sequence.next(), sequence.next(), sequence.next());
        let small = (value % 1_000, numerator % 100, denominator % 50);
        for (value, numerator, denominator) in [(value, numerator, denominator), small] {
            for rounding in Rounding::ALL {
                // A result that does not fit counts as i64::MIN, which no fitting result of these inputs equals.
                fingerprint.add(scale(value, numerator, denominator, rounding).unwrap_or(i64::MIN));
                count += 1;
            }
        }
    }
    println!(
        "fingerprint of scale ({count} values): {:016x}",
        fingerprint.0
    );
    check("scale", &fingerprint, 0x2d70_a155_3950_33fe);
}

#[test]
fn transforms_give_the_committed_bits() {
    let mut sequence = Sequence(0xfedc_ba98_7654_3210);
    let mut fingerprint = Fingerprint::new();
    let mut count = 0;
    let page = 2 * 22 * 1_828_800;
    for angle in angles().into_iter().take(1_000) {
        let point = Point::new(Blu(sequence.next() % page), Blu(sequence.next() % page));
        let factor = Fixed(sequence.next() % (4 << 32));
        let transform = Transform::rotation(angle)
            .then(Transform::scaling(factor, Fixed::ONE), Rounding::HalfEven)
            .and_then(|t| {
                t.then(
                    Transform::translation(Blu(sequence.next() % page), Blu::ZERO),
                    Rounding::HalfEven,
                )
            })
            .expect("realistic transforms compose");
        for rounding in Rounding::ALL {
            let mapped = transform
                .map_point(point, rounding)
                .expect("realistic points map");
            fingerprint.add(mapped.x.0);
            fingerprint.add(mapped.y.0);
            count += 2;
        }
        for coefficient in [transform.xx, transform.yx, transform.xy, transform.yy] {
            fingerprint.add(coefficient.0);
            count += 1;
        }
    }
    println!(
        "fingerprint of transforms ({count} values): {:016x}",
        fingerprint.0
    );
    check("transforms", &fingerprint, 0x965e_7afb_f695_a5d8);
}
