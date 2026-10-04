// Exceptions for whole lint groups that contain `clippy::disallowed_methods`; the compiler, run by Clippy, must reject each one with error E0453. Each `reason` is unique, so the gate can tell which attribute was rejected; keep each `expect(…)` on one line for that.

/// An exception for the `clippy::style` group, to which `disallowed_methods` belongs.
#[must_use]
#[expect(clippy::style, reason = "canary: style group")]
pub fn sine(x: f64) -> f64 {
    x.sin()
}

/// An exception for `clippy::all`, Clippy's default groups.
#[must_use]
#[expect(clippy::all, reason = "canary: all group")]
pub fn cosine(x: f64) -> f64 {
    x.cos()
}
