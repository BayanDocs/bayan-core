// Every way the code itself could make an exception to the float bans; the compiler, run by Clippy, must reject each one with error E0453 because the workspace lints forbid `clippy::disallowed_methods`. A fourth form, the crate-level attribute, is at the top of lib.rs. Each `reason` is unique, so the gate can tell which attribute was rejected; keep each `expect(…)` on one line for that.

// A module-level exception (an inner attribute).
#![expect(clippy::disallowed_methods, reason = "canary: module")]

/// An item-level exception.
#[must_use]
#[expect(clippy::disallowed_methods, reason = "canary: item")]
pub fn sine(x: f64) -> f64 {
    x.sin()
}

/// An item-level exception that only appears when a condition holds, here one that is always true.
#[must_use]
#[cfg_attr(all(), expect(clippy::disallowed_methods, reason = "canary: cfg_attr"))]
pub fn cosine(x: f64) -> f64 {
    x.cos()
}
