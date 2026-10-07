//! Property-based tests of bayan-units (CORE-002): each property is checked on many generated inputs, in particular against independent reference implementations written with arbitrary-precision integers.
//!
//! proptest runs here without its default features, so every run uses the same fixed seed: a failure reproduces on every platform, including WebAssembly, where these tests also run (`cargo xtask verify`, step `test`).

mod ooxml;
mod reference;
mod rounding;
mod transform;
mod trig;
mod units;
