//! # lint-canary
//!
//! Deliberately broken code that proves the workspace's lint rules work (CORE-001, acceptance criterion AC-2).
//!
//! A normal build compiles an empty crate. `cargo xtask verify` compiles one case at a time by passing `--cfg bayan_lint_canary="<case>"` to the compiler, and checks that Clippy or the compiler rejects exactly what it must: platform floating-point math, hash maps and sets, `unsafe` code in a crate that is not a binding crate, and lint exceptions that are not written as `#[expect(…, reason = "…")]`. It also checks that the one sanctioned exception is accepted.
//!
//! This crate is configured exactly like a core crate (`[lints] workspace = true` in its manifest and the root `clippy.toml`), so whatever is rejected here is rejected in every core crate. Unlike the core crates, it has no `#![forbid(unsafe_code)]` attribute of its own: the `unsafe_block` case proves that the workspace lint table alone rejects unsafe code.
//!
//! When you add an entry to `clippy.toml`, use it in the matching case below; `cargo xtask verify` fails until every entry is proven to trigger.

/// Calls every method listed in `disallowed-methods` in `clippy.toml`; Clippy must reject each call.
#[cfg(bayan_lint_canary = "disallowed_methods")]
pub mod disallowed_methods;

/// Uses every type listed in `disallowed-types` in `clippy.toml`; Clippy must reject each use.
#[cfg(bayan_lint_canary = "disallowed_types")]
pub mod disallowed_types;

/// The sanctioned exception, `#[expect(lint, reason = "…")]`; Clippy must accept it.
#[cfg(bayan_lint_canary = "expect_with_reason")]
pub mod expect_with_reason;

/// An exception without a justification; Clippy must reject it.
#[cfg(bayan_lint_canary = "expect_without_reason")]
pub mod expect_without_reason;

/// An exception written with `#[allow]` instead of `#[expect]`; Clippy must reject it.
#[cfg(bayan_lint_canary = "allow_with_reason")]
pub mod allow_with_reason;

/// An `unsafe` block; the compiler must refuse to build it.
#[cfg(bayan_lint_canary = "unsafe_block")]
pub mod unsafe_block;

/// An attempt to switch the forbidden `unsafe_code` lint off locally; the compiler must refuse to build it.
#[cfg(bayan_lint_canary = "unsafe_expect")]
pub mod unsafe_expect;
