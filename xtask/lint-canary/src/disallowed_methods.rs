/// Calls every disallowed `f32` method once.
#[must_use]
pub fn f32_methods(x: f32, y: f32) -> f32 {
    x.sin()
        + x.cos()
        + x.tan()
        + x.asin()
        + x.acos()
        + x.atan()
        + x.atan2(y)
        + x.sinh()
        + x.cosh()
        + x.tanh()
        + x.exp()
        + x.exp2()
        + x.exp_m1()
        + x.ln()
        + x.log(y)
        + x.log2()
        + x.log10()
        + x.ln_1p()
        + x.powf(y)
        + x.powi(2)
        + x.cbrt()
        + x.hypot(y)
        + x.sin_cos().0
        + x.asinh()
        + x.acosh()
        + x.atanh()
}

/// Calls every disallowed `f64` method once.
#[must_use]
pub fn f64_methods(x: f64, y: f64) -> f64 {
    x.sin()
        + x.cos()
        + x.tan()
        + x.asin()
        + x.acos()
        + x.atan()
        + x.atan2(y)
        + x.sinh()
        + x.cosh()
        + x.tanh()
        + x.exp()
        + x.exp2()
        + x.exp_m1()
        + x.ln()
        + x.log(y)
        + x.log2()
        + x.log10()
        + x.ln_1p()
        + x.powf(y)
        + x.powi(2)
        + x.cbrt()
        + x.hypot(y)
        + x.sin_cos().0
        + x.asinh()
        + x.acosh()
        + x.atanh()
}

/// The same method called as a path instead of with a dot; Clippy must catch this spelling too.
#[must_use]
pub fn path_call(x: f64) -> f64 {
    f64::sin(x)
}
