/// Clamps a colour channel to `0.0..=1.0` the sanctioned way: with explicit comparisons, because `f32::clamp` is forbidden (it may return `+0.0` or `-0.0` non-deterministically). With constant bounds, this is exactly the pattern Clippy's `manual_clamp` lint would reject in favour of `clamp`.
#[must_use]
pub fn clamp_channel(x: f32) -> f32 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

/// The same for `f64`.
#[must_use]
pub fn clamp_unit(x: f64) -> f64 {
    if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}
