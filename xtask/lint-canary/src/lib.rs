//! # lint-canary
//!
//! Deliberately broken code that proves the workspace's lint rules work (CORE-001, acceptance criterion AC-2).
//!
//! A normal build compiles an empty crate. `cargo xtask verify` compiles one case at a time by passing `--cfg bayan_lint_canary="<case>"` to the compiler, in its own build folder `target/lint-canary/<case>`, and checks that Clippy or the compiler rejects exactly what it must: platform floating-point math, hash maps and sets, `unsafe` code in a crate that is not a binding crate, lint exceptions that are not written as `#[expect(…, reason = "…")]`, and any exception to the float bans. It also checks that what must be allowed is accepted: the one sanctioned exception, and the explicit comparison that replaces the forbidden `clamp`.
//!
//! This crate is configured like a core crate: `[lints] workspace = true` in its manifest, and the root `clippy.toml`. That alone does not prove the rules reach every core crate, because each crate could be configured differently: Clippy does not merge configuration files, so a `clippy.toml` below the root would replace the root one for the crates beneath it without affecting this crate. The gate therefore also checks that every crate inherits the workspace lints (the guardrails step) and that Clippy reads no configuration other than the root `clippy.toml` (the preflight). The preflight also rejects build scripts that xtask does not list, and compiler flags that lower lint levels, and the cases are compiled with the project's real compiler flags, so a flag such as `--cap-lints=allow` makes a case compile and the gate fail. Together with those checks, whatever is rejected here is rejected in every core crate. That includes exceptions written in a crate's own code: `clippy::disallowed_methods` is forbidden, so the compiler rejects `#[expect(clippy::disallowed_methods, …)]` and exceptions for the groups that contain it, which the `expect_disallowed_methods` and `expect_lint_groups` cases prove. Unlike the core crates, this one has no `#![forbid(unsafe_code)]` attribute of its own: the `unsafe_block` case proves that the workspace lint table alone rejects unsafe code.
//!
//! When you add an entry to `clippy.toml`, use it in the matching case below; `cargo xtask verify` fails until every entry is proven to trigger.

// The crate-level form of an exception to the float bans, for the `expect_disallowed_methods` case; it must be rejected like the other forms in that module.
#![cfg_attr(
    bayan_lint_canary = "expect_disallowed_methods",
    expect(clippy::disallowed_methods, reason = "canary: crate")
)]

/// Calls every method listed in `disallowed-methods` in `clippy.toml`; Clippy must reject each call.
#[cfg(bayan_lint_canary = "disallowed_methods")]
pub mod disallowed_methods;

/// Clamps floats with explicit comparisons, the sanctioned replacement for the forbidden `clamp`; Clippy must accept it, which needs `manual_clamp = "allow"` in the workspace lints.
#[cfg(bayan_lint_canary = "explicit_clamp")]
pub mod explicit_clamp;

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

/// Exceptions to the forbidden float bans written in the code, at module and item level and inside `cfg_attr`; the crate-level one is at the top of this file. Clippy must reject each with error E0453.
#[cfg(bayan_lint_canary = "expect_disallowed_methods")]
pub mod expect_disallowed_methods;

/// Exceptions for the lint groups `clippy::style` and `clippy::all`, which contain the forbidden float bans; Clippy must reject each with error E0453.
#[cfg(bayan_lint_canary = "expect_lint_groups")]
pub mod expect_lint_groups;

/// An `unsafe` block; the compiler must refuse to build it.
#[cfg(bayan_lint_canary = "unsafe_block")]
pub mod unsafe_block;

/// An attempt to switch the forbidden `unsafe_code` lint off locally; the compiler must refuse to build it.
#[cfg(bayan_lint_canary = "unsafe_expect")]
pub mod unsafe_expect;
