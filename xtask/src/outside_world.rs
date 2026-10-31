//! The code that ships of the engine's crates reaches the outside world through none of the ways that Rust's standard library offers for it.
//!
//! The engine never touches the outside world itself (ADR-0012 §5): clocks, environment variables, files, sockets, the console and other processes are the host's, and the host passes the engine what it needs through the protocol's host services. Nothing that affects the engine's output may depend on the clock or the environment (ADR-0005 §5), and the engine reads no environment variables (architecture §12). This check enforces the part of those rules that the source text shows. The code that ships may name none of the five modules of Rust's standard library that exist for the outside world, `std::env` (environment variables, the command line, the current folder), `std::fs` (files), `std::net` (sockets), `std::process` (other processes, and ending this one) and `std::time` (clocks), and none of the items of [`ITEMS`], through which std reaches the outside world outside those modules: the file system through the methods of `std::path::Path`, the operating system's randomness through `RandomState`, the environment through threads started without a stack size and through backtraces, the clock through sleeping and waiting with a timeout, the operating system through `available_parallelism`, and the console through `std::io::stdin`, `stdout` and `stderr`. The exceptions are the workspace members that `OUTSIDE_WORLD` in `verify.rs` lists, each with the modules, or the items of them, that it may use and the reason, and the uses of items that `APPROVED_ITEMS` there approves for one file each, with the reason why that use cannot reach the outside world. Dependencies reach the outside world too (see "Known limitations" below), so passing it means that the workspace's own code reaches the outside world through none of these ways, not that nothing it calls can.
//!
//! The environment that the code that ships reads while it is compiled, with `env!` and `option_env!`, is checked separately, from the compiler's record of it (`dep_info.rs`).
//!
//! # Why it is not a lint
//!
//! A lint at `deny` or `warn` can be switched off by `#[allow(…)]` or `#[expect(…)]`, and a macro can assemble such an attribute in a form that neither the source check nor Clippy's `allow_attributes` lint sees (`sources.rs`, "Known limitation"). Only a lint at `forbid` cannot be lowered, but Clippy reads one configuration for the whole workspace (`clippy.toml`) and gives a lint one level in every crate, so a ban of these modules and items there would also hold for the members that must reach the outside world, such as xtask and the command-line tools, with no way to exempt them. This check is therefore not a lint at all: xtask reads the source text itself, so no attribute changes what it finds, however it is written or assembled.
//!
//! # The code it reads
//!
//! It reads every line of exactly the Rust files that the compiler compiled into code that ships. After its runs, the Clippy step hands it, for every member, the files that the dependency files list for the member's units that are not tests (not compiled as unit tests, and not a test, benchmark, example or build-script target), among the files that the source check read (`dep_info.rs`). A module that only the tests use, declared with `#[cfg(test)] mod name;`, is left out by that record itself, because the compiler does not read its file for those units. That, or an integration test, is where tests that measure time, read fixtures, start threads or run programs belong. Inside the files that remain, nothing is skipped, also not a module written in place under `#[cfg(test)]`: the check reads the source text, and a macro can drop that condition from a module it is handed and ship the module's code, as in `macro_rules! shipped { (#[cfg(test)] mod $name:ident { $($body:tt)* }) => { $($body)* }; }`, so only the compiler's record can tell which code only the tests compile.
//!
//! It also reads the strings in attributes, because a derive macro can read a string of its attribute as Rust and make it code that ships: `#[serde(default = "path")]` makes serde's derive call the function at `path`. Each such string is decoded as the compiler decodes them (escapes, raw strings) and checked by the same rules as code, also for the ways of naming std below, except the two rules for a `::` or a `.` on its own, which in prose are only punctuation. Prose in an attribute, such as the `reason` of an `#[expect]`, is checked the same way, since most prose reads as a series of Rust tokens: it names these modules and items in words, not as paths, and does not write `std` on its own. A string that cannot be read as Rust at all (an unclosed quote, comment or bracket) is skipped, because a derive macro that reads its string as Rust, as serde's and schemars' do, rejects it too. Documentation is left out (`#[doc = "…"]`, which `///` and `//!` comments become, and `#[doc(…)]`): nothing compiles it into code that ships, and its examples are compiled only as doctests.
//!
//! # What it rejects
//!
//! In the code that ships of every member, unless its entry of `OUTSIDE_WORLD` lists the module (an entry that lists whole modules) or the item (an entry that lists items):
//!
//! - a path from `std` into one of the five modules, such as `std::time::Instant`, `::std::env::var` or `use std::{fs, io}`, also in a group inside a group (`use std::{{net}}`), whatever spaces, comments or line breaks stand between its words, and also with raw identifiers (`r#std::r#env`). For an entry that lists items, the path must name a listed item right after its module (`std::time::Instant`, `use std::time::{Instant}`); the module on its own (`use std::time;`), any other of its items and `std::time::*` are rejected.
//!
//! In the code that ships of every member that `OUTSIDE_WORLD` does not list with whole modules, unless `APPROVED_ITEMS` approves the item for the file:
//!
//! - an item of [`ITEMS`], found as [`Written`] describes for each: an item written as a path by the last name of its module, `::` and its own name, wherever that pair stands (`std::thread::spawn`, and also `thread::spawn` once `use std::thread;` has bound the module, or `crate::thread::spawn` once a re-export has), also as an entry of a `use` group (`use std::thread::{spawn}`) and when the item is renamed (`use std::thread::spawn as go;`); a method of `std::path::Path` by its name after a single `.`, so that a call on a value of any origin counts, also on the `&Path` that a dependency returns, and after `Path::` or `>::`; and a method with a name of its own (`recv_timeout`, `wait_timeout`, …) wherever its name stands.
//!
//! In the code that ships of every member, ways to name std, or to build a path in a macro, that would let a later path reach those modules without writing `std` in front of them:
//!
//! - `std` on its own, not followed by `::`: renamed (`use std as host;`, `extern crate std as host;`), handed to a macro (`m!(std)`), or used as a name;
//! - `std::*`, and `*` in a group after `std::`, which import every module of std under its own name, so that `env::var` reaches `std::env`;
//! - `self` in a group after `std::`, as in `use std::{self as host}`, which renames std;
//! - `std::` followed by anything but a name or a group, such as a macro's metavariable (`std::$module`);
//! - in a macro, a path that continues from a metavariable into one of the five names or into another metavariable (`$root::env`, `$root::$module`), because a macro can be handed `std` as the first word of a longer path; and a metavariable anywhere in a `use` or `extern crate` declaration (`use $root as host;`, `use {$root as host};`, `use $root::*;`), which could rename std or import all of it. `$crate`, which always names the macro's own crate, is fine.
//!
//! In the code that ships of every member whose code is checked for [`ITEMS`], the ways that would let a later path or call reach an item without its module's name and its own standing together, or a method's name after its `.`:
//!
//! - a module that holds items of [`ITEMS`] (`thread`, `io`, `hash`, `hash_map`, `path` and `backtrace`, whatever path leads to it) renamed in a `use` declaration (`use std::thread as t;`, `use std::thread::{self as t};`), or followed by `*` (`use std::io::*;`), which imports its items under their own names;
//! - in a macro, a path from a metavariable into the name of an item or of such a module (`$module::spawn`, `$root::thread`), a path from such a module into a metavariable (`thread::$item`), and a method or field named by a metavariable (`$value.$method`);
//! - an item's name after a `::` that follows no name (`::spawn`), and a `::` or a `.` that stands on its own instead of in a path or before a method, a field or a tuple's index, as when it is handed to a macro: with them, a macro handed `thread`, `::` and `spawn` separately could put them together.
//!
//! In the preflight, before anything is built, a member that `OUTSIDE_WORLD` does not name may not depend on one that it names, except for its tests (`[dev-dependencies]`), so that code which ships cannot reach the outside world through a member that may.
//!
//! The lists cannot go stale: an entry of `OUTSIDE_WORLD` for a member that does not exist, an entry without modules or items, a module that is not one of the five, an item that is not one of a module of the five, and a module or item that the member's code that ships no longer uses all fail; so does an entry of `APPROVED_ITEMS` for a member that does not exist or whose code is not checked for [`ITEMS`], an item that is not one of [`ITEMS`], and an approved item that the file no longer uses.
//!
//! # Known limitations
//!
//! These rules model Rust's tokens and paths; they are not the compiler's name resolution, and they read only the workspace's own code.
//!
//! - A path that never writes the module's name is not seen: a dependency that re-exports a module or an item of std under another name, or a macro of a dependency that expands to such a path. Dependencies are reviewed when they are added (ADR-0017).
//! - Functions of dependencies that reach the outside world themselves are not seen either, such as cfb's `cfb::open(path)`, `open_rw(path)` and `create(path)`, which open files with `std::fs`, and the clock of web-time; bayan-opc calls only `CompoundFile::open` on the bytes it is handed (a `Cursor`).
//! - A method of `std::path::Path` counts wherever its name follows a single `.`, because the check cannot tell the types of values: a method or field of another type with one of these names (`exists`, `metadata`, `is_dir`, …) fails too. Rename it; for a dependency's method that does not touch the file system, such as cfb's in-memory `CompoundFile::exists`, `APPROVED_ITEMS` can approve the name for the file, with the reason, in a reviewed pull request.
//! - The print macros (`print!`, `println!`, `eprint!`, `eprintln!` and `dbg!`), which write to the console without naming `std::io`, are left to the lints `print_stdout`, `print_stderr` and `dbg_macro`, whose exceptions the lint audit checks (CORE-009); a panic's message, which std's default panic hook writes to the console, is the bindings' panic hooks' concern.
//! - Items that only the nightly toolchain offers, such as `std::thread::sleep_until` and `std::random`, are not listed, because stable code cannot use them and the gate rejects every way to unlock them (`RUSTC_BOOTSTRAP` and `-Z` options). A toolchain that makes such an item stable is adopted in the monthly dependency session, which adds it to [`ITEMS`].
//! - An attribute macro on an enclosing module, or any other procedural macro, receives the code it is applied to and can write different code: it could drop `#[cfg(test)]` from a module as `shipped!` does above, or write a path that the source does not show. No workspace member is a procedural-macro crate (`PROC_MACROS` in `verify.rs` is empty), and no dependency of the members offers an attribute macro for modules today (wasm-bindgen's `#[wasm_bindgen]`, the only attribute macro among them, refuses modules); the macros of dependencies are reviewed with them (ADR-0017).
//! - A string in an attribute that a macro assembles from a split form, keeping the `#` of one attribute and writing it before a bracket group handed in separately (AGENTS.md, "Lint rules and exceptions"), is not read as an attribute's string, because the source does not show that bracket group as an attribute. Reviewers check every macro that takes a `#` or a bracket group as a `tt`, as AGENTS.md asks.
//!
//! In the WebAssembly build, where std has no clock, files or environment of its own, the import check of `cargo xtask wasm-package` (`wasm_imports.rs`) shows every function that the module asks of its JavaScript host, whoever calls it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::policy::MemberDependency;
use crate::sources::{self, Kind, Token};

/// The five modules of Rust's standard library that exist for the outside world, as code writes them.
pub const MODULES: [&str; 5] = [
    "std::env",
    "std::fs",
    "std::net",
    "std::process",
    "std::time",
];

/// How the check finds an item of [`ITEMS`] in the source text, which shows names but not the types of values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// Named by a path: the last name of its module, `::` and its own name, wherever that pair stands, also as an entry of a `use` group (`thread::{spawn}`). So `std::thread::spawn` counts, and so does `thread::spawn` once `use std::thread;`, or a re-export anywhere, has bound the module under its own name.
    Path,
    /// A method of a type that [`ITEMS`] forbids by name too: its name after a single `.`, which calls it on a value of any origin (also one that a dependency hands over, such as the `&Path` of cfb's `Entry::path`), or after its type's name, or a `>`, and `::` (`Path::exists(path)`, `<Path>::exists(path)`). A method or field of the same name on another type counts too, because the check cannot tell the types of values.
    Method,
    /// A method of a type that is not forbidden, with a name that only it has: its name wherever it stands, so that neither another name for its type (`type Queue = Receiver<Job>;`) nor a macro that is handed the name can hide a call.
    Name,
}

/// A way to the outside world that Rust's standard library offers outside [`MODULES`]: one entry of [`ITEMS`].
#[derive(Debug, Clone, Copy)]
pub struct Item {
    /// The item, as code writes it from `std`; a method as its type's path and its name, such as `std::path::Path::exists`.
    pub path: &'static str,
    /// How the check finds it.
    pub written: Written,
    /// What it reaches, and so why it is forbidden.
    pub reaches: &'static str,
}

impl Item {
    /// Its own name: the last of its path.
    fn name(&self) -> &'static str {
        self.path.rsplit("::").next().unwrap_or(self.path)
    }

    /// The name before its own: the last name of its module, or for a method its type's name.
    fn owner(&self) -> &'static str {
        self.path.rsplit("::").nth(1).unwrap_or("")
    }
}

/// What the methods of `std::path::Path` that ask the file system reach.
const FILE_SYSTEM: &str = "the file system, which it asks about a path";

/// What `RandomState` reaches.
const RANDOMNESS: &str = "the operating system's randomness, with which it seeds itself, so that the order of a hash map or set built with it changes from run to run (ADR-0005 §5)";

/// What a thread started without an explicit stack size reaches.
const STACK: &str = "the environment: a thread started without an explicit stack size takes its size from the RUST_MIN_STACK variable, so the environment would decide how deeply an input may nest before the thread runs out of stack";

/// What sleeping and waiting with a timeout reach.
const CLOCK: &str = "the clock, which it reads to know when the time is up";

/// What the standard streams reach.
const CONSOLE: &str = "the console: the process's standard input, output and error";

/// The items of Rust's standard library that reach the outside world outside [`MODULES`], each with what it reaches. The code that ships of a member that `OUTSIDE_WORLD` lists with whole modules, a tool that reaches the outside world anyway, is not checked for them; that of every other member may use one only where `APPROVED_ITEMS` approves it for the file. `std::path::PathBuf` and `std::path::Component`, which only build paths, are not forbidden: a `PathBuf` reaches the methods of `Path` only after a `.` (`PathBuf::exists(&path)` does not compile), where they count.
pub const ITEMS: [Item; 30] = [
    Item {
        path: "std::path::Path",
        written: Written::Path,
        reaches: "the file system, which its methods exists, try_exists, is_file, is_dir, is_symlink, metadata, symlink_metadata, read_dir, read_link and canonicalize ask about a path; it is forbidden by name so that none of them can be called as `Path::exists(path)`",
    },
    Item {
        path: "std::path::Path::exists",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::try_exists",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::is_file",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::is_dir",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::is_symlink",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::metadata",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::symlink_metadata",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::read_dir",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::read_link",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::Path::canonicalize",
        written: Written::Method,
        reaches: FILE_SYSTEM,
    },
    Item {
        path: "std::path::absolute",
        written: Written::Path,
        reaches: "the current folder, which it reads to complete a relative path",
    },
    Item {
        path: "std::hash::RandomState",
        written: Written::Path,
        reaches: RANDOMNESS,
    },
    Item {
        path: "std::collections::hash_map::RandomState",
        written: Written::Path,
        reaches: RANDOMNESS,
    },
    Item {
        path: "std::thread::spawn",
        written: Written::Path,
        reaches: STACK,
    },
    Item {
        path: "std::thread::scope",
        written: Written::Path,
        reaches: STACK,
    },
    Item {
        path: "std::thread::Builder",
        written: Written::Path,
        reaches: "the environment, unless the builder sets the stack size: a thread that it starts without `stack_size` takes its size from the RUST_MIN_STACK variable, so the environment would decide how deeply an input may nest before the thread runs out of stack",
    },
    Item {
        path: "std::thread::available_parallelism",
        written: Written::Path,
        reaches: "the operating system, whose files and calls it reads to tell how many processors the program may use",
    },
    Item {
        path: "std::thread::sleep",
        written: Written::Path,
        reaches: CLOCK,
    },
    Item {
        path: "std::thread::sleep_ms",
        written: Written::Path,
        reaches: CLOCK,
    },
    Item {
        path: "std::thread::park_timeout",
        written: Written::Path,
        reaches: CLOCK,
    },
    Item {
        path: "std::thread::park_timeout_ms",
        written: Written::Path,
        reaches: CLOCK,
    },
    Item {
        path: "std::sync::mpsc::Receiver::recv_timeout",
        written: Written::Name,
        reaches: CLOCK,
    },
    Item {
        path: "std::sync::Condvar::wait_timeout",
        written: Written::Name,
        reaches: CLOCK,
    },
    Item {
        path: "std::sync::Condvar::wait_timeout_ms",
        written: Written::Name,
        reaches: CLOCK,
    },
    Item {
        path: "std::sync::Condvar::wait_timeout_while",
        written: Written::Name,
        reaches: CLOCK,
    },
    Item {
        path: "std::backtrace::Backtrace",
        written: Written::Path,
        reaches: "the environment, whose RUST_BACKTRACE and RUST_LIB_BACKTRACE decide whether `Backtrace::capture` captures, and the program's own files, which showing a backtrace reads for the names of its functions",
    },
    Item {
        path: "std::io::stdin",
        written: Written::Path,
        reaches: CONSOLE,
    },
    Item {
        path: "std::io::stdout",
        written: Written::Path,
        reaches: CONSOLE,
    },
    Item {
        path: "std::io::stderr",
        written: Written::Path,
        reaches: CONSOLE,
    },
];

/// The name through which code reaches Rust's standard library.
const STD: &str = "std";

/// A workspace member whose code that ships may reach the outside world: one entry of `OUTSIDE_WORLD` in `verify.rs`.
#[derive(Debug, Clone, Copy)]
pub struct Exemption {
    /// The member.
    pub member: &'static str,
    /// What its code that ships may use.
    pub uses: Uses,
    /// Why it needs it.
    pub reason: &'static str,
}

/// What the code that ships of a member that `OUTSIDE_WORLD` names may use.
#[derive(Debug, Clone, Copy)]
pub enum Uses {
    /// Whole modules of [`MODULES`], as code writes them, such as `std::fs`. Such a member is a tool that reaches the outside world anyway, so its code is not checked for [`ITEMS`], which reach the same outside world outside these modules; and with `std::env` among them, its code that ships may read any environment variable while it is compiled too (`dep_info.rs`).
    Modules(&'static [&'static str]),
    /// Exactly these items of [`MODULES`], as code writes them, such as `std::process::abort`, and nothing else that reaches the outside world: no other item of their modules, no module on its own and none of [`ITEMS`].
    Items(&'static [&'static str]),
}

impl Uses {
    /// The modules or items, as written.
    fn listed(self) -> &'static [&'static str] {
        match self {
            Self::Modules(listed) | Self::Items(listed) => listed,
        }
    }
}

/// Items of [`ITEMS`] that one file of a member's code that ships may use, because that use cannot reach the outside world: one entry of `APPROVED_ITEMS` in `verify.rs`.
#[derive(Debug, Clone, Copy)]
pub struct Approved {
    /// The member.
    pub member: &'static str,
    /// The file, relative to the workspace root, with `/` between folders.
    pub file: &'static str,
    /// The items it may use, as [`ITEMS`] writes them.
    pub items: &'static [&'static str],
    /// Why that use cannot reach the outside world.
    pub reason: &'static str,
}

/// The members whose code that ships may read any environment variable while it is compiled: those that `exemptions` lists with the whole module `std::env`, whose code may read any variable while it runs anyway. The Clippy step checks the compile-time environment of every other member's code that ships (`dep_info.rs`).
pub fn environment_exempt(exemptions: &[Exemption]) -> Vec<&'static str> {
    exemptions
        .iter()
        .filter(|exemption| {
            matches!(exemption.uses, Uses::Modules(modules) if modules.contains(&"std::env"))
        })
        .map(|exemption| exemption.member)
        .collect()
}

/// Why the modules are rejected, and what to do instead.
const WHY: &str = "The engine never touches the outside world itself (ADR-0012 §5): clocks, environment variables, files, sockets and other processes are the host's, which passes the engine what it needs through the protocol's host services, and nothing that affects the engine's output may depend on the clock or the environment (ADR-0005 §5; the engine reads no environment variables, architecture §12). So the code that ships may name none of the five modules of Rust's standard library that exist for the outside world, std::env, std::fs, std::net, std::process and std::time, except in the members that OUTSIDE_WORLD in xtask/src/verify.rs lists, each with the modules, or the items of them, that it may use; this includes the strings of attributes, which a derive macro such as serde's can read as paths. Ask the host for what the code needs instead (a time, the bytes of a file, a setting). For a length of time without a clock, use core::time::Duration, the same type as std::time::Duration. If only tests need the module, move them out of the files that ship: into an integration test (tests/), or into a module declared with `#[cfg(test)] mod name;` and written in its own file, which the compiler does not read for the code that ships. A test module written in place, `#[cfg(test)] mod tests { … }`, counts as code that ships, because a macro could drop its condition. A member that really needs a module, or an item of one, is added to OUTSIDE_WORLD, with it and the reason, in a reviewed pull request; an entry that lists items allows exactly those, named right after their module (`std::time::Instant`, not `use std::time;`).";

/// How the uses of the modules are reported: the heading of their list.
const USED: &str = "the code that ships names modules of the standard library that exist for the outside world, or items of them, that OUTSIDE_WORLD does not allow for its member";

/// How the uses of [`ITEMS`] are reported: the heading of their list, and what to do.
const ITEMS_USED: (&str, &str) = (
    "the code that ships uses items of the standard library that reach the outside world outside those modules (ITEMS in xtask/src/outside_world.rs)",
    "Outside the five modules, the standard library reaches the outside world through these items too, which the code that ships may not use either (ADR-0012 §5, ADR-0005 §5, architecture §12): the file system through std::path::Path and its methods, the current folder through std::path::absolute, the operating system's randomness through RandomState, the environment through threads started without a stack size (std::thread::spawn, scope and Builder) and through std::backtrace::Backtrace, the operating system through std::thread::available_parallelism, the clock through sleeping and waiting with a timeout (std::thread::sleep and park_timeout, recv_timeout, wait_timeout), and the console through std::io::stdin, stdout and stderr. Ask the host for what the code needs instead; keep maps and sets ordered (BTreeMap, BTreeSet); start engine threads with bayan-engine's EngineThread, which sets the stack size. The check reads the source text and cannot tell the types of values, so it finds a method of std::path::Path by its name after a `.`: if the name above is a method or field of another type, rename it; if you cannot, because the method belongs to a dependency and does not touch the file system, approve it for the file in APPROVED_ITEMS in xtask/src/verify.rs, with the reason, in a reviewed pull request, as for every other use that cannot reach the outside world. If only tests need the item, move them out of the files that ship: into an integration test (tests/), or into a module declared with `#[cfg(test)] mod name;` and written in its own file.",
);

/// How the ways of naming std that the check cannot follow are reported: the heading of their list, and what to do.
const HIDDEN: (&str, &str) = (
    "the code that ships names `std`, or builds a path in a macro, in a way that could reach those modules unseen",
    "This check reads the source text, not the compiler's name resolution, so it finds a module of std only where a path names it after `std::`. Naming std in any other way could reach std::env, std::fs, std::net, std::process or std::time without writing that path: `use std as host;` and then `host::env::var`, `use std::*;` and then `env::var`, or a macro that is handed `std` and writes `$root::env`. So std is named only as the first word of a path written out in full, `std::name` or `std::{…}`. Write the path out in full, and rename an item or a variable that happens to be called `std`.",
);

/// How the ways of reaching [`ITEMS`] that the check cannot follow are reported: the heading of their list, and what to do.
const HIDDEN_ITEMS: (&str, &str) = (
    "the code that ships names a module that holds such items, or builds a path or a method call in a macro, in a way that could reach them unseen",
    "This check reads the source text, not the compiler's name resolution, so it finds an item of ITEMS where the last name of its module and its own name stand together (`thread::spawn`), also after `use std::thread;`, and a method of std::path::Path where its name follows a `.`. Renaming a module that holds such items (`use std::thread as t;`, `use std::thread::{self as t};`), importing all of it (`use std::thread::*;`), and macros that build a path or a method call from what they are handed (`$module::spawn`, `thread::$item`, `$value.$method`, or a `::` or `.` handed to a macro on its own) could reach them without that text. Import such a module under its own name, name the items you use, write paths and method calls out in full, and hand a macro a whole path or expression instead of its pieces.",
);

/// How the modules and items that a member no longer uses are reported: the heading of their list, and what to do.
const STALE: (&str, &str) = (
    "OUTSIDE_WORLD or APPROVED_ITEMS lists modules or items that the code that ships does not use",
    "Each entry of OUTSIDE_WORLD in xtask/src/verify.rs names a member and the modules of the standard library, or the items of them, that its code that ships may use, and each entry of APPROVED_ITEMS names a file and the items of ITEMS that it may use. Remove each module or item above from its entry, and the entry when nothing is left, so that the lists name only what the code needs and cannot go stale.",
);

/// How the dependencies of other members on the listed ones are reported: the heading of their list, and what to do.
const DEPENDS: (&str, &str) = (
    "members that OUTSIDE_WORLD does not name depend on members that it names",
    "A member that OUTSIDE_WORLD in xtask/src/verify.rs names may reach the outside world, so code that ships of any other member could reach it through such a dependency, without naming a module of the standard library itself. Only tests may depend on them (`[dev-dependencies]`). Move the code that both need into a member outside the list, or remove the dependency.",
);

/// What follows a module of [`MODULES`] in a path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Next {
    /// Nothing, or `self` in a group after it: the module itself, as in `use std::time;`.
    Module,
    /// One of its items, by name, as in `std::time::Instant`.
    Item(String),
    /// Something that the check does not read as the name of an item: `*`, a metavariable, or other punctuation.
    Other,
}

/// What the scan found in one file, each finding with the line it is on.
#[derive(Debug, Default, PartialEq, Eq)]
struct Found {
    /// The paths from std into [`MODULES`], each by the module as code writes it and what follows it.
    modules: Vec<(usize, &'static str, Next)>,
    /// The uses of [`ITEMS`], each by the item as [`ITEMS`] writes it.
    items: Vec<(usize, &'static str)>,
    /// The ways of naming std, or of building a path in a macro, that could reach [`MODULES`] unseen, each described.
    hidden: Vec<(usize, &'static str)>,
    /// The ways of naming a module that holds items of [`ITEMS`], or of building a path or a method call in a macro, that could reach them unseen, each described.
    hidden_items: Vec<(usize, &'static str)>,
}

/// `std` on its own.
const BARE_STD: &str = "`std` on its own, not followed by `::`";
/// `std::*`, or `*` in a group after `std::`.
const STD_GLOB: &str = "`*` after `std::`, which imports every module of std under its own name";
/// `self` in a group after `std::`.
const STD_SELF: &str = "`self` in a group after `std::`, which can rename std";
/// `std::` followed by something else.
const STD_UNREADABLE: &str =
    "`std::` followed by something other than a name or a group, such as a metavariable";
/// A path from a metavariable on.
const MACRO_PATH: &str =
    "a path from a macro's metavariable into one of the five names or another metavariable";
/// `use` or `extern crate` with a metavariable.
const MACRO_USE: &str = "a `use` or `extern crate` declaration with a macro's metavariable in it";
/// A module that holds items of [`ITEMS`], renamed.
const HOLDER_RENAMED: &str = "a module that holds items of ITEMS (thread, io, hash, hash_map, path or backtrace), renamed with `as`, so that its items could be named without its name";
/// `*` after a module that holds items of [`ITEMS`].
const HOLDER_GLOB: &str =
    "`*` after a module that holds items of ITEMS, which imports them under their own names";
/// A path from a module that holds items of [`ITEMS`] into a metavariable.
const HOLDER_METAVARIABLE: &str =
    "a path from a module that holds items of ITEMS into a macro's metavariable";
/// A path from a metavariable into the name of an item or of a module that holds some.
const MACRO_ITEM: &str = "a path from a macro's metavariable into the name of an item of ITEMS or of a module that holds some";
/// An item's name after a `::` that follows no name.
const LEADING_ITEM: &str = "the name of an item of ITEMS after a `::` that follows no name, as when it is handed to a macro apart from its module";
/// A method or field named by a metavariable.
const METHOD_METAVARIABLE: &str =
    "a method or field named by a macro's metavariable, which could be a method of ITEMS";
/// A `::` on its own.
const LONE_SEPARATOR: &str = "`::` on its own, not between the parts of a path, as when it is handed to a macro, which could put it between names handed in separately";
/// A `.` on its own.
const LONE_DOT: &str = "`.` on its own, not before a method, a field or a tuple's index, as when it is handed to a macro, which could put it before a method's name handed in separately";

/// The attribute of documentation, whose strings are never code (`#[doc = "…"]`, `#[doc(…)]`).
const DOC: &str = "doc";

/// The module of [`MODULES`] whose last word is `name`, if there is one.
fn module(name: &str) -> Option<&'static str> {
    MODULES
        .iter()
        .copied()
        .find(|module| module.strip_prefix("std::") == Some(name))
}

/// The items of [`ITEMS`] that are found as `written` and whose own name is `name`.
fn items_named(written: Written, name: &str) -> impl Iterator<Item = &'static Item> + '_ {
    ITEMS
        .iter()
        .filter(move |item| item.written == written && item.name() == name)
}

/// Whether `name` is the last name of a module that holds items of [`ITEMS`] written as paths, such as `thread`.
fn holds_items(name: &str) -> bool {
    ITEMS
        .iter()
        .any(|item| item.written == Written::Path && item.owner() == name)
}

/// Whether `name` is the own name of an item of [`ITEMS`], or the last name of a module that holds some.
fn names_an_item(name: &str) -> bool {
    holds_items(name) || ITEMS.iter().any(|item| item.name() == name)
}

/// Records the items of [`ITEMS`] that `owner::name`, on `line`, names: one written as a path whose module's last name is `owner`, or a method whose type's name is.
fn pair(owner: &str, name: &str, line: usize, found: &mut Found) {
    for item in &ITEMS {
        if item.written != Written::Name && item.owner() == owner && item.name() == name {
            found.items.push((line, item.path));
        }
    }
}

/// Whether the text is code, or the decoded string of an attribute, which may be prose, where `::` and `.` on their own are punctuation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Text {
    /// Rust source.
    Code,
    /// The string of an attribute, read as Rust.
    AttributeString,
}

/// What a `::` follows in a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parent<'a> {
    /// `std`.
    Std,
    /// A macro's metavariable other than `$crate`, which could stand for any path.
    Metavariable,
    /// Another name, `$crate` among them (as `crate`).
    Name(&'a str),
    /// The `>` that ends generic arguments or a qualified path (`<Path>::`), whose type the check cannot tell.
    Generic,
    /// Nothing that a path continues from: a `::` at the start of a path, or after other punctuation.
    Nothing,
}

/// Finds in Rust source the uses of [`MODULES`] and of [`ITEMS`], and the ways of naming std, of naming a module that holds items of [`ITEMS`], or of building a path or a method call in a macro, that could reach them unseen, in every line, and in the strings of attributes other than documentation, each decoded and read as Rust (see the module's documentation). Comments, other strings and character literals are skipped, as the source check skips them. Source the reader cannot follow is an error.
fn scan(source: &str) -> Result<Found, String> {
    scan_text(source, Text::Code)
}

/// [`scan`] for code or for the string of an attribute (`text`).
fn scan_text(source: &str, text: Text) -> Result<Found, String> {
    let tokens = sources::tokenize(source)?;
    let closing = sources::closing_brackets(&tokens)?;
    let reader = Reader {
        tokens: &tokens,
        closing: &closing,
        text,
    };
    let mut found = Found::default();
    reader.read(&mut found);
    let kind = |at: usize| tokens.get(at).map(|token| token.kind);
    // Each string once, also when attributes nest.
    let strings: BTreeSet<usize> = sources::attribute_contents(&tokens, &closing)
        .into_iter()
        .filter(|content| kind(content.start) != Some(Kind::Ident(DOC)))
        .flatten()
        .filter(|&at| matches!(tokens[at].kind, Kind::Str(_)))
        .collect();
    for at in strings {
        let Kind::Str(literal) = tokens[at].kind else {
            continue;
        };
        // A string that is not Rust is skipped: whatever reads it as Rust rejects it too.
        let Some(Ok(inner)) =
            string_value(literal).map(|value| scan_text(&value, Text::AttributeString))
        else {
            continue;
        };
        // Reported on the line where the string starts.
        let line = tokens[at].line;
        found.modules.extend(
            inner
                .modules
                .into_iter()
                .map(|(_, module, next)| (line, module, next)),
        );
        found
            .items
            .extend(inner.items.into_iter().map(|(_, item)| (line, item)));
        found
            .hidden
            .extend(inner.hidden.into_iter().map(|(_, what)| (line, what)));
        found
            .hidden_items
            .extend(inner.hidden_items.into_iter().map(|(_, what)| (line, what)));
    }
    found.modules.sort_by_key(|&(line, _, _)| line);
    found.items.sort_by_key(|&(line, _)| line);
    found.hidden.sort_by_key(|&(line, _)| line);
    found.hidden_items.sort_by_key(|&(line, _)| line);
    Ok(found)
}

/// Reads the tokens of one text for [`scan_text`].
struct Reader<'t, 'a> {
    /// The tokens.
    tokens: &'t [Token<'a>],
    /// For each opening bracket, the position of the bracket that closes it (`sources::closing_brackets`).
    closing: &'t [Option<usize>],
    /// Whether the tokens are code or the string of an attribute.
    text: Text,
}

impl<'a> Reader<'_, 'a> {
    /// The kind of the token at `at`, if there is one.
    fn kind(&self, at: usize) -> Option<Kind<'a>> {
        self.tokens.get(at).map(|token| token.kind)
    }

    /// The kind of the token before `at`, if there is one.
    fn before(&self, at: usize) -> Option<Kind<'a>> {
        at.checked_sub(1).and_then(|before| self.kind(before))
    }

    /// Whether the token at `at` is a macro's metavariable: a name right after `$`.
    fn metavariable(&self, at: usize) -> bool {
        matches!(self.kind(at), Some(Kind::Ident(_))) && self.before(at) == Some(Kind::Punct(b'$'))
    }

    /// Reads every token into `found`.
    fn read(&self, found: &mut Found) {
        for (at, token) in self.tokens.iter().enumerate() {
            let line = token.line;
            match token.kind {
                Kind::Ident(STD) => {
                    // `std::…` is read at its `::`.
                    if self.kind(at + 1) != Some(Kind::PathSeparator) {
                        found.hidden.push((line, BARE_STD));
                    }
                }
                Kind::PathSeparator => self.separator(at, found),
                Kind::Punct(b'.') => self.dot(at, found),
                Kind::Ident(word @ ("use" | "crate"))
                    if word == "use" || self.before(at) == Some(Kind::Ident("extern")) =>
                {
                    self.declaration(at, found);
                }
                Kind::Ident(name) => {
                    for item in items_named(Written::Name, name) {
                        found.items.push((line, item.path));
                    }
                }
                _ => {}
            }
        }
    }

    /// What the `::` at `at` follows.
    fn parent(&self, at: usize) -> Parent<'a> {
        let Some(before) = at.checked_sub(1) else {
            return Parent::Nothing;
        };
        match self.tokens[before].kind {
            Kind::Ident("crate") if self.metavariable(before) => Parent::Name("crate"),
            Kind::Ident(_) if self.metavariable(before) => Parent::Metavariable,
            Kind::Ident(STD) => Parent::Std,
            Kind::Ident(name) => Parent::Name(name),
            // `->` and `=>` end in `>` too.
            Kind::Punct(b'>') if !matches!(self.before(before), Some(Kind::Punct(b'-' | b'='))) => {
                Parent::Generic
            }
            _ => Parent::Nothing,
        }
    }

    /// Reads the path at the `::` at `at`: what it continues from, and what follows.
    fn separator(&self, at: usize, found: &mut Found) {
        let line = self.tokens[at].line;
        let next = self.kind(at + 1);
        let parent = self.parent(at);
        match parent {
            Parent::Std => match next {
                Some(Kind::Ident(name)) => self.module_path(at + 1, name, found),
                Some(Kind::Punct(b'{')) => self.std_group(at + 1, found),
                Some(Kind::Punct(b'*')) => found.hidden.push((line, STD_GLOB)),
                _ => found.hidden.push((line, STD_UNREADABLE)),
            },
            Parent::Metavariable => match next {
                Some(Kind::Ident(name)) if module(name).is_some() => {
                    found.hidden.push((line, MACRO_PATH));
                }
                Some(Kind::Punct(b'$')) => found.hidden.push((line, MACRO_PATH)),
                Some(Kind::Ident(name)) if names_an_item(name) => {
                    found.hidden_items.push((line, MACRO_ITEM));
                }
                _ => {}
            },
            Parent::Name(owner) => match next {
                Some(Kind::Ident(name)) => pair(owner, name, line, found),
                Some(Kind::Punct(b'{')) => {
                    for entry in self.entries(at + 1) {
                        let token = self.tokens[entry];
                        match token.kind {
                            Kind::Ident(name) => pair(owner, name, token.line, found),
                            Kind::Punct(b'*') if holds_items(owner) => {
                                found.hidden_items.push((token.line, HOLDER_GLOB));
                            }
                            _ => {}
                        }
                    }
                }
                Some(Kind::Punct(b'*')) if holds_items(owner) => {
                    found.hidden_items.push((line, HOLDER_GLOB));
                }
                Some(Kind::Punct(b'$')) if holds_items(owner) => {
                    found.hidden_items.push((line, HOLDER_METAVARIABLE));
                }
                _ => {}
            },
            Parent::Generic => {
                if let Some(Kind::Ident(name)) = next {
                    for item in items_named(Written::Method, name) {
                        found.items.push((line, item.path));
                    }
                }
            }
            Parent::Nothing => {
                if let Some(Kind::Ident(name)) = next
                    && items_named(Written::Path, name).next().is_some()
                {
                    found.hidden_items.push((line, LEADING_ITEM));
                }
            }
        }
        // In code, a `::` stands only between the parts of a path, and before the `+`, `?` or `*` of a macro's repetition whose separator it is; after `std` it is reported above.
        let continues = matches!(
            next,
            Some(Kind::Ident(_) | Kind::Punct(b'{' | b'*' | b'<' | b'$' | b'+' | b'?'))
        );
        if self.text == Text::Code && parent != Parent::Std && !continues {
            found.hidden_items.push((line, LONE_SEPARATOR));
        }
    }

    /// Records the path from `std` into `name`, at `tokens[at]`, if `name` is a module of [`MODULES`], with what follows it.
    fn module_path(&self, at: usize, name: &str, found: &mut Found) {
        if let Some(module) = module(name) {
            for next in self.after_module(at) {
                found.modules.push((self.tokens[at].line, module, next));
            }
        }
    }

    /// Reads the entries of the group `{ … }` that opens at `tokens[open]` after `std::`: the first word of each entry names a module of std, so it is checked against [`MODULES`]; `self` and `*` are reported, and so is anything else that is not a name.
    fn std_group(&self, open: usize, found: &mut Found) {
        for entry in self.entries(open) {
            let token = self.tokens[entry];
            match token.kind {
                Kind::Ident("self") => found.hidden.push((token.line, STD_SELF)),
                Kind::Ident(name) => self.module_path(entry, name, found),
                Kind::Punct(b'*') => found.hidden.push((token.line, STD_GLOB)),
                _ => found.hidden.push((token.line, STD_UNREADABLE)),
            }
        }
    }

    /// What follows the module at `tokens[at]`: one [`Next`] for the path, or one for each entry of a group after it.
    fn after_module(&self, at: usize) -> Vec<Next> {
        if self.kind(at + 1) != Some(Kind::PathSeparator) {
            return vec![Next::Module];
        }
        let next = |at: usize| match self.kind(at) {
            Some(Kind::Ident("self")) => Next::Module,
            Some(Kind::Ident(name)) => Next::Item(name.to_owned()),
            _ => Next::Other,
        };
        if self.kind(at + 2) == Some(Kind::Punct(b'{')) {
            self.entries(at + 2).into_iter().map(next).collect()
        } else {
            vec![next(at + 2)]
        }
    }

    /// The positions of the first tokens of the entries of the group `{ … }` that opens at `tokens[open]`. A group that is itself an entry, with no path before it (`{{env}}`), is read in the same way; the rest of each entry belongs to the path it starts.
    fn entries(&self, open: usize) -> Vec<usize> {
        // `closing_brackets` succeeded, so every bracket is closed.
        let Some(close) = self.closing[open] else {
            return Vec::new();
        };
        let mut entries = Vec::new();
        let mut at = open + 1;
        let mut starts_entry = true;
        while at < close {
            let token = self.tokens[at];
            if starts_entry {
                starts_entry = false;
                if token.kind == Kind::Punct(b'{') {
                    entries.extend(self.entries(at));
                } else {
                    entries.push(at);
                }
            }
            match token.kind {
                Kind::Punct(b',') => starts_entry = true,
                // A nested group belongs to the entry it is part of, and one that is an entry was read above.
                Kind::Punct(b'(' | b'[' | b'{') => {
                    if let Some(end) = self.closing[at] {
                        at = end;
                    }
                }
                _ => {}
            }
            at += 1;
        }
        entries
    }

    /// Reads the `.` at `at`: the method or field after it, or what makes it stand on its own.
    fn dot(&self, at: usize, found: &mut Found) {
        let before = self.before(at);
        let next = self.kind(at + 1);
        // `..`, `..=` and `...` are ranges and rest patterns.
        if before == Some(Kind::Punct(b'.')) || next == Some(Kind::Punct(b'.')) {
            return;
        }
        let line = self.tokens[at].line;
        match next {
            Some(Kind::Ident(name)) => {
                for item in items_named(Written::Method, name) {
                    found.items.push((line, item.path));
                }
            }
            Some(Kind::Punct(b'$')) => found.hidden_items.push((line, METHOD_METAVARIABLE)),
            // A tuple's field, such as `.0`.
            Some(Kind::Other) => {}
            // In code, a `.` stands before a method or a field, or ends a number such as `1.`.
            _ => {
                if self.text == Text::Code && before != Some(Kind::Other) {
                    found.hidden_items.push((line, LONE_DOT));
                }
            }
        }
    }

    /// Reads the `use` or `extern crate` declaration whose keyword is at `at`, up to its `;`, which no use tree contains: a metavariable in it, or a module that holds items of [`ITEMS`] renamed in it.
    fn declaration(&self, at: usize, found: &mut Found) {
        let end = (at + 1..self.tokens.len())
            .find(|&end| self.tokens[end].kind == Kind::Punct(b';'))
            .unwrap_or(self.tokens.len());
        let metavariable = (at + 1..end).any(|dollar| {
            self.tokens[dollar].kind == Kind::Punct(b'$')
                && self.kind(dollar + 1) != Some(Kind::Ident("crate"))
        });
        if metavariable {
            found.hidden.push((self.tokens[at].line, MACRO_USE));
        }
        for rename in (at + 1..end).filter(|&rename| self.tokens[rename].kind == Kind::Ident("as"))
        {
            let renamed = match self.before(rename) {
                Some(Kind::Ident("self")) => self.group_owner(rename - 1),
                Some(Kind::Ident(name)) if !self.metavariable(rename - 1) => Some(name),
                _ => None,
            };
            if renamed.is_some_and(holds_items) {
                found
                    .hidden_items
                    .push((self.tokens[rename].line, HOLDER_RENAMED));
            }
        }
    }

    /// The name before the `::{` of the innermost group that holds the token at `at`, if that group follows one.
    fn group_owner(&self, at: usize) -> Option<&'a str> {
        let open = (0..at).rev().find(|&open| {
            self.tokens[open].kind == Kind::Punct(b'{')
                && self.closing[open].is_some_and(|close| close > at)
        })?;
        if self.before(open) != Some(Kind::PathSeparator) {
            return None;
        }
        let name = open.checked_sub(2)?;
        match self.kind(name) {
            Some(Kind::Ident(owner)) if !self.metavariable(name) => Some(owner),
            _ => None,
        }
    }
}

/// The text of a string literal, written as in the source with its prefix and quotes (`"…"`, `b"…"`, `c"…"`, `r#"…"#`, `br"…"`, `cr"…"`), with its escapes decoded as the compiler decodes them. `None` for an escape that this reader does not know; the compiler rejects every such escape.
fn string_value(literal: &str) -> Option<String> {
    let literal = literal.strip_prefix(['b', 'c']).unwrap_or(literal);
    if let Some(raw) = literal.strip_prefix('r') {
        // A raw string: its text is exactly what stands between `r#…#"` and `"#…#`.
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        return raw
            .get(hashes + 1..raw.len().checked_sub(hashes + 1)?)
            .map(str::to_owned);
    }
    let mut characters = literal.strip_prefix('"')?.strip_suffix('"')?.chars();
    let mut text = String::new();
    while let Some(character) = characters.next() {
        if character != '\\' {
            text.push(character);
            continue;
        }
        match characters.next()? {
            'n' => text.push('\n'),
            'r' => text.push('\r'),
            't' => text.push('\t'),
            '0' => text.push('\0'),
            escaped @ ('\\' | '\'' | '"') => text.push(escaped),
            'x' => {
                let digits: String = characters.by_ref().take(2).collect();
                text.push(char::from(u8::from_str_radix(&digits, 16).ok()?));
            }
            'u' => {
                if characters.next()? != '{' {
                    return None;
                }
                let digits: String = characters
                    .by_ref()
                    .take_while(|&digit| digit != '}')
                    .filter(|&digit| digit != '_')
                    .collect();
                text.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            // A line break after a backslash continues the string on the next line, without the break and the spaces that begin that line.
            '\n' => {
                let rest = characters.as_str();
                characters = rest.trim_start_matches([' ', '\t', '\n', '\r']).chars();
            }
            _ => return None,
        }
    }
    Some(text)
}

/// Whether `name` is written like an identifier: ASCII letters, digits and `_`, not starting with a digit.
fn is_identifier(name: &str) -> bool {
    name.starts_with(|first: char| first.is_ascii_alphabetic() || first == '_')
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Whether `file` is a path relative to the root, with `/` between folders and without `.` or `..`, of a Rust file.
fn is_relative_rust_file(file: &str) -> bool {
    file.ends_with(".rs")
        && file.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':'])
        })
}

/// Checks `OUTSIDE_WORLD` (`exemptions`) and `APPROVED_ITEMS` (`approved`) before anything is built: each entry of `OUTSIDE_WORLD` names a workspace member (of `members`) once, and lists at least one module of [`MODULES`], or one item of such a module, each only once; each entry of `APPROVED_ITEMS` names a workspace member whose code is checked for [`ITEMS`], a Rust file once, and at least one item of [`ITEMS`], each only once; and no member that `OUTSIDE_WORLD` does not name depends on one that it names (`dependencies`, from `cargo metadata`), except as a dev-dependency. Returns one line describing what was checked, or every problem found.
pub fn check_list(
    members: &[&str],
    dependencies: &[MemberDependency],
    exemptions: &[Exemption],
    approved: &[Approved],
) -> Result<String, String> {
    let mut problems = Vec::new();
    for (index, exemption) in exemptions.iter().enumerate() {
        let member = exemption.member;
        if !members.contains(&member) {
            problems.push(format!(
                "  `{member}` is not a workspace member; update the list"
            ));
        }
        if exemptions[..index]
            .iter()
            .any(|earlier| earlier.member == member)
        {
            problems.push(format!("  `{member}` is listed twice"));
        }
        let listed = exemption.uses.listed();
        let (kind, valid): (&str, fn(&str) -> bool) = match exemption.uses {
            Uses::Modules(_) => ("module", |written| MODULES.contains(&written)),
            Uses::Items(_) => ("item", |written| {
                written
                    .rsplit_once("::")
                    .is_some_and(|(module, name)| MODULES.contains(&module) && is_identifier(name))
            }),
        };
        if listed.is_empty() {
            problems.push(format!(
                "  the entry for `{member}` lists no {kind}; remove it"
            ));
        }
        for (position, written) in listed.iter().enumerate() {
            if !valid(written) {
                problems.push(match exemption.uses {
                    Uses::Modules(_) => format!(
                        "  the entry for `{member}` lists `{written}`, which is not one of {}",
                        MODULES.join(", ")
                    ),
                    Uses::Items(_) => format!(
                        "  the entry for `{member}` lists `{written}`, which is not an item of one of {}, written as the module, `::` and the item's name",
                        MODULES.join(", ")
                    ),
                });
            }
            if listed[..position].contains(written) {
                problems.push(format!(
                    "  the entry for `{member}` lists `{written}` twice"
                ));
            }
        }
    }
    for (index, entry) in approved.iter().enumerate() {
        let (member, file) = (entry.member, entry.file);
        if !members.contains(&member) {
            problems.push(format!(
                "  APPROVED_ITEMS: `{member}` is not a workspace member; update the list"
            ));
        }
        if exemptions.iter().any(|exemption| {
            exemption.member == member && matches!(exemption.uses, Uses::Modules(_))
        }) {
            problems.push(format!(
                "  APPROVED_ITEMS: OUTSIDE_WORLD lists whole modules for `{member}`, so its code is not checked for ITEMS; remove the entry"
            ));
        }
        if !is_relative_rust_file(file) {
            problems.push(format!(
                "  APPROVED_ITEMS: {file} is not the path of a Rust file relative to the root, with `/` between folders and without `.` or `..`"
            ));
        }
        if approved[..index]
            .iter()
            .any(|earlier| earlier.member == member && earlier.file == file)
        {
            problems.push(format!(
                "  APPROVED_ITEMS: {file} of `{member}` is listed twice"
            ));
        }
        if entry.items.is_empty() {
            problems.push(format!(
                "  APPROVED_ITEMS: the entry for {file} lists no item; remove it"
            ));
        }
        for (position, item) in entry.items.iter().enumerate() {
            if !ITEMS.iter().any(|known| known.path == *item) {
                problems.push(format!(
                    "  APPROVED_ITEMS: the entry for {file} lists `{item}`, which ITEMS in xtask/src/outside_world.rs does not list"
                ));
            }
            if entry.items[..position].contains(item) {
                problems.push(format!(
                    "  APPROVED_ITEMS: the entry for {file} lists `{item}` twice"
                ));
            }
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "OUTSIDE_WORLD or APPROVED_ITEMS in xtask/src/verify.rs is not right:\n{}\nEach entry of OUTSIDE_WORLD names a workspace member, the modules of the standard library that exist for the outside world ({}), or the items of them, that its code that ships may use, and the reason. Each entry of APPROVED_ITEMS names a member whose code is checked for ITEMS, one of its Rust files, the items of ITEMS that the file may use, and why that use cannot reach the outside world.",
            problems.join("\n"),
            MODULES.join(", ")
        ));
    }
    let listed = |name: &str| exemptions.iter().any(|exemption| exemption.member == name);
    let reaching: Vec<String> = dependencies
        .iter()
        .filter(|dependency| !dependency.dev && !listed(&dependency.from) && listed(&dependency.on))
        .map(|dependency| format!("  {} depends on {}", dependency.from, dependency.on))
        .collect();
    if !reaching.is_empty() {
        return Err(format!(
            "{}:\n{}\n{}",
            DEPENDS.0,
            reaching.join("\n"),
            DEPENDS.1
        ));
    }
    let names: Vec<&str> = exemptions
        .iter()
        .map(|exemption| exemption.member)
        .collect();
    Ok(format!(
        "outside world: every member that OUTSIDE_WORLD names is a workspace member ({}), and no other member depends on one of them except for its tests; APPROVED_ITEMS approves {} items of ITEMS for {} files",
        names.join(", "),
        approved
            .iter()
            .map(|entry| entry.items.len())
            .sum::<usize>(),
        approved.len()
    ))
}

/// A module of [`MODULES`] and what follows it, as a message shows it.
fn described(module: &str, next: &Next) -> String {
    match next {
        Next::Module => format!("{module} on its own"),
        Next::Item(name) => format!("{module}::{name}"),
        Next::Other => format!("{module} followed by something other than an item's name"),
    }
}

/// Checks the code that ships of every workspace member at `root`: `shipped` maps each member to the Rust files that the compiler compiled into its units that are not tests (`dep_info::Comparison::shipped`). A member that `exemptions` does not name may use none of [`MODULES`], one that it names with modules only those modules, and one that it names with items exactly those items; every member that it does not name with whole modules may use none of [`ITEMS`], except where `approved` approves an item for the file; and every member names std only as the first word of a path written out in full, and, where its code is checked for [`ITEMS`], their modules only under their own names and their items, methods and modules never through a macro's pieces. Each module and item that an entry of `exemptions` or `approved` lists must be used. Returns one line describing what was checked, or every problem found.
pub fn check_code(
    root: &Path,
    shipped: &BTreeMap<String, BTreeSet<PathBuf>>,
    exemptions: &[Exemption],
    approved: &[Approved],
) -> Result<String, String> {
    let root = std::fs::canonicalize(root)
        .map_err(|error| format!("cannot find {}: {error}", root.display()))?;
    let relative = |file: &Path| file.strip_prefix(&root).unwrap_or(file).to_path_buf();
    let shown = |file: &Path| relative(file).display().to_string();
    // As APPROVED_ITEMS writes files: relative to the root, with `/` between folders.
    let written = |file: &Path| {
        relative(file)
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut module_places = Vec::new();
    let mut item_places = Vec::new();
    let mut hidden_places = Vec::new();
    let mut hidden_item_places = Vec::new();
    // The modules and items that each member used as its entry lists them, and the approved items that each entry of `approved` approved.
    let mut used: BTreeSet<(String, String)> = BTreeSet::new();
    let mut approved_used: BTreeSet<(usize, &str)> = BTreeSet::new();
    // The files read, of the members that OUTSIDE_WORLD does not name, of those it names with items, and of those it names with modules.
    let mut files_read = [0_usize; 3];
    for (member, files) in shipped {
        let uses = exemptions
            .iter()
            .find(|exemption| exemption.member == member)
            .map(|exemption| exemption.uses);
        let items_checked = !matches!(uses, Some(Uses::Modules(_)));
        for file in files {
            let source = std::fs::read_to_string(file)
                .map_err(|error| format!("cannot read {}: {error}", shown(file)))?;
            let found = scan(&source).map_err(|error| {
                format!(
                    "{}: {error}. The gate must be able to read every Rust file to check it.",
                    shown(file)
                )
            })?;
            files_read[match uses {
                None => 0,
                Some(Uses::Items(_)) => 1,
                Some(Uses::Modules(_)) => 2,
            }] += 1;
            for (line, module, next) in found.modules {
                match uses {
                    Some(Uses::Modules(listed)) if listed.contains(&module) => {
                        used.insert((member.clone(), module.to_owned()));
                    }
                    Some(Uses::Modules(listed)) => module_places.push(format!(
                        "  {}:{line}: {module} (OUTSIDE_WORLD lists only {} for {member})",
                        shown(file),
                        listed.join(", ")
                    )),
                    Some(Uses::Items(listed)) => {
                        let item = match &next {
                            Next::Item(name) => format!("{module}::{name}"),
                            Next::Module | Next::Other => String::new(),
                        };
                        if listed.contains(&item.as_str()) {
                            used.insert((member.clone(), item));
                        } else {
                            module_places.push(format!(
                                "  {}:{line}: {} (OUTSIDE_WORLD lists only the items {} for {member})",
                                shown(file),
                                described(module, &next),
                                listed.join(", ")
                            ));
                        }
                    }
                    None => module_places.push(format!("  {}:{line}: {module}", shown(file))),
                }
            }
            for (line, what) in found.hidden {
                hidden_places.push(format!("  {}:{line}: {what}", shown(file)));
            }
            if !items_checked {
                continue;
            }
            let file_written = written(file);
            for (line, item) in found.items {
                let approval = approved.iter().position(|entry| {
                    entry.member == member
                        && entry.file == file_written
                        && entry.items.contains(&item)
                });
                if let Some(index) = approval {
                    approved_used.insert((index, item));
                } else {
                    let reaches = ITEMS
                        .iter()
                        .find(|known| known.path == item)
                        .map_or("the outside world", |known| known.reaches);
                    item_places.push(format!(
                        "  {}:{line}: {item}, which reaches {reaches}",
                        shown(file)
                    ));
                }
            }
            for (line, what) in found.hidden_items {
                hidden_item_places.push(format!("  {}:{line}: {what}", shown(file)));
            }
        }
    }
    let mut stale = Vec::new();
    for exemption in exemptions {
        for listed in exemption.uses.listed() {
            if !used.contains(&(exemption.member.to_owned(), (*listed).to_owned())) {
                stale.push(format!(
                    "  {listed}, for {} (listed because {})",
                    exemption.member, exemption.reason
                ));
            }
        }
    }
    for (index, entry) in approved.iter().enumerate() {
        for item in entry.items {
            if !approved_used.contains(&(index, *item)) {
                stale.push(format!(
                    "  {item}, in {} of {} (approved because {})",
                    entry.file, entry.member, entry.reason
                ));
            }
        }
    }
    let problems: Vec<String> = [
        (module_places, USED, WHY),
        (item_places, ITEMS_USED.0, ITEMS_USED.1),
        (hidden_places, HIDDEN.0, HIDDEN.1),
        (hidden_item_places, HIDDEN_ITEMS.0, HIDDEN_ITEMS.1),
        (stale, STALE.0, STALE.1),
    ]
    .into_iter()
    .filter(|(places, _, _)| !places.is_empty())
    .map(|(places, heading, why)| format!("{heading}:\n{}\n{why}", places.join("\n")))
    .collect();
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let named = |modules: bool| -> Vec<&str> {
        exemptions
            .iter()
            .filter(|exemption| matches!(exemption.uses, Uses::Modules(_)) == modules)
            .map(|exemption| exemption.member)
            .filter(|member| shipped.contains_key(*member))
            .collect()
    };
    let others: Vec<&str> = shipped
        .keys()
        .map(String::as_str)
        .filter(|member| {
            !exemptions
                .iter()
                .any(|exemption| exemption.member == *member)
        })
        .collect();
    Ok(format!(
        "outside world: the code that ships of the members that OUTSIDE_WORLD does not name ({}; {} files) names none of {} and uses none of the {} items of ITEMS but the {} that APPROVED_ITEMS approves for their files; that of the members it names with items ({}; {} files) names exactly the items it lists for them and uses none of ITEMS; and that of the members it names with whole modules ({} files) names exactly the modules it lists for them",
        others.join(", "),
        files_read[0],
        MODULES.join(", "),
        ITEMS.len(),
        approved_used.len(),
        named(false).join(", "),
        files_read[1],
        files_read[2]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `scan` finds in `source`, which it must be able to read.
    fn scanned(source: &str) -> Found {
        scan(source).unwrap_or_else(|problem| panic!("{problem}"))
    }

    /// The modules that `scan` finds in `source`, which must name std in no hidden way.
    fn uses(source: &str) -> Vec<&'static str> {
        let found = scanned(source);
        assert!(found.hidden.is_empty(), "{source}: {:?}", found.hidden);
        found
            .modules
            .into_iter()
            .map(|(_, module, _)| module)
            .collect()
    }

    /// The modules that `scan` finds in `source`, each with what follows it.
    fn module_paths(source: &str) -> Vec<String> {
        scanned(source)
            .modules
            .iter()
            .map(|(_, module, next)| match next {
                Next::Module => (*module).to_owned(),
                Next::Item(name) => format!("{module}::{name}"),
                Next::Other => format!("{module}::?"),
            })
            .collect()
    }

    /// The descriptions of the hidden ways that `scan` finds in `source`.
    fn hidden(source: &str) -> Vec<&'static str> {
        scanned(source)
            .hidden
            .into_iter()
            .map(|(_, what)| what)
            .collect()
    }

    /// The items of ITEMS that `scan` finds in `source`, which must reach them in no hidden way.
    fn items(source: &str) -> Vec<&'static str> {
        let found = scanned(source);
        assert!(
            found.hidden_items.is_empty(),
            "{source}: {:?}",
            found.hidden_items
        );
        found.items.into_iter().map(|(_, item)| item).collect()
    }

    /// The descriptions of the hidden ways of reaching ITEMS that `scan` finds in `source`.
    fn hidden_items(source: &str) -> Vec<&'static str> {
        scanned(source)
            .hidden_items
            .into_iter()
            .map(|(_, what)| what)
            .collect()
    }

    #[test]
    fn finds_every_path_into_the_five_modules() {
        assert_eq!(
            uses("fn now() -> u64 { std::time::Instant::now(); 0 }"),
            ["std::time"]
        );
        assert_eq!(uses("use ::std::env::var;"), ["std::env"]);
        assert_eq!(
            uses("use std::{fs, io::{self, Read}, net::TcpStream};"),
            ["std::fs", "std::net"]
        );
        // A group inside a group, with no path before it, still names std's modules.
        assert_eq!(
            uses("use std::{{process}, {{time as clock}}};"),
            ["std::process", "std::time"]
        );
        // Spaces, comments, line breaks and raw identifiers change nothing.
        assert_eq!(
            uses("let home = std /* the library */ ::\n    r#env :: var(\"HOME\");"),
            ["std::env"]
        );
        assert_eq!(uses("r#std::fs::read(path)"), ["std::fs"]);
        // In a group, only the first word of an entry is a module of std; `self` inside a nested group belongs to that module.
        assert_eq!(
            uses("use std::{io::{self, Write}, sync::Arc};"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn reads_what_follows_each_module() {
        // The item right after the module, one for each entry of a group after it, the module itself, and anything else.
        assert_eq!(
            module_paths(
                "use std::time::Instant; std::process::abort(); use std::{time::{Instant as Clock, SystemTime, self}}; use std::process; use std::time::*; fn f() { std::time::$item }"
            ),
            [
                "std::time::Instant",
                "std::process::abort",
                "std::time::Instant",
                "std::time::SystemTime",
                "std::time",
                "std::process",
                "std::time::?",
                "std::time::?",
            ]
        );
    }

    #[test]
    fn reports_the_line_of_each_use() {
        let found = scanned(
            "use std::sync::Arc;\n\nuse std::{\n    collections::BTreeMap,\n    time::Duration,\n};\nfn f() { std::process::abort() }\n",
        );
        assert_eq!(
            found,
            Found {
                modules: vec![
                    (5, "std::time", Next::Item("Duration".to_owned())),
                    (7, "std::process", Next::Item("abort".to_owned())),
                ],
                ..Found::default()
            }
        );
    }

    #[test]
    fn leaves_other_modules_and_other_crates_alone() {
        assert_eq!(
            uses(
                "use std::collections::BTreeMap; use std::sync::{Arc, Mutex}; use std::thread::Builder; use core::time::Duration; use crate::time::Clock; let fs = 1; fn process() {} const NAME: &str = \"std::env\"; // std::fs\n/* std::net */"
            ),
            Vec::<&str>::new()
        );
        // The compile-time macros `env!` and `option_env!` read the environment while the crate is compiled, which the compiler's record shows (`dep_info.rs`), not the environment of the running engine.
        assert_eq!(
            uses("pub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn reads_every_line_also_under_cfg_test() {
        // The compiler's record leaves out the files of the modules that only the tests declare (`#[cfg(test)] mod name;`); in the files that ship, every line counts, also a test module written in place.
        assert_eq!(
            uses(
                "use std::sync::Arc;\n#[cfg(test)]\n#[cfg(not(target_arch = \"wasm32\"))]\nmod tests {\n    use std::time::Duration;\n}\nfn after() { std::env::args(); }\n"
            ),
            ["std::time", "std::env"]
        );
        assert_eq!(uses("#[cfg(test)] use std::time::Instant;"), ["std::time"]);
        assert_eq!(
            uses("#[cfg(any(test, debug_assertions))] mod checks { use std::env; }"),
            ["std::env"]
        );
        assert_eq!(
            items("#[cfg(test)]\nmod tests {\n    fn wait() { std::thread::sleep(WAIT); }\n}\n"),
            ["std::thread::sleep"]
        );
    }

    #[test]
    fn reads_the_test_modules_that_a_macro_could_ship() {
        // A macro can drop `#[cfg(test)]` from the module it is handed and ship the module's code, so the test module in its call counts.
        let shipped = "macro_rules! shipped {\n    (#[cfg(test)] mod $name:ident { $($body:tt)* }) => { $($body)* };\n}\nshipped! {\n    #[cfg(test)]\n    mod tests {\n        pub fn now() -> std::time::Instant { std::time::Instant::now() }\n    }\n}\n";
        assert_eq!(uses(shipped), ["std::time", "std::time"]);
        // The same with a matcher that takes any attributes.
        let any_attributes = "macro_rules! shipped {\n    ($(#[$meta:meta])* mod $name:ident { $($body:tt)* }) => { $($body)* };\n}\nshipped! { #[cfg(test)] mod tests { pub use std::env::var; } }\n";
        assert_eq!(uses(any_attributes), ["std::env"]);
    }

    #[test]
    fn reads_the_strings_of_attributes_as_code() {
        // serde's derive reads these strings as paths: the first names no module of std, the second calls `std::process::id`.
        assert_eq!(
            uses(
                "#[derive(Serialize)]\nstruct Answer {\n    #[serde(default, skip_serializing_if = \"std::ops::Not::not\")]\n    flag: bool,\n}\n"
            ),
            Vec::<&str>::new()
        );
        assert_eq!(
            scanned(
                "#[derive(Deserialize)]\nstruct Request {\n    #[serde(default = \"std::process::id\")]\n    id: u32,\n}\n"
            ),
            Found {
                modules: vec![(3, "std::process", Next::Item("id".to_owned()))],
                ..Found::default()
            }
        );
        // Escapes, raw strings, a line continued after a backslash, an attribute inside `cfg_attr` and an inner attribute are read as the compiler reads them.
        assert_eq!(
            uses(
                "#[serde(default = \"std\\x3a\\x3aprocess::id\")]\n#[serde(with = r#\"std::time\"#)]\n#[cfg_attr(test, serde(default = \"std::\\\n    env::args\"))]\n#![serde(default = \"\\u{73}td::fs::read\")]\n"
            ),
            ["std::process", "std::time", "std::env", "std::fs"]
        );
        // So are the ways of naming std, also in a `[…]` after a metavariable, which can become an attribute.
        assert_eq!(hidden("#[serde(with = \"std\")]"), [BARE_STD]);
        assert_eq!(
            hidden("macro_rules! m { ($hash:tt) => { $hash[serde(with = \"std::*\")] }; }"),
            [STD_GLOB]
        );
        // And the items of ITEMS.
        assert_eq!(
            items("#[serde(default = \"std::thread::available_parallelism\")]"),
            ["std::thread::available_parallelism"]
        );
        // Prose that reads as Rust is checked like code; a string that cannot be read as Rust, documentation and the strings outside attributes are not.
        assert_eq!(
            uses("#[expect(dead_code, reason = \"never calls std::process::exit\")]"),
            ["std::process"]
        );
        assert_eq!(
            uses(
                "#[expect(dead_code, reason = \"it's 'quoted' prose about std::fs, which no derive reads\")]\n#[doc = \"Calls std::env::var.\"]\n/// Calls std::time::Instant::now.\n#[doc(alias = \"std::net\")]\nconst PATH: &str = \"std::process::id\";\n"
            ),
            Vec::<&str>::new()
        );
        // In prose, a `::` or a `.` on its own is punctuation.
        assert_eq!(
            hidden_items(
                "#[expect(dead_code, reason = \"see the notes. Or e.g. `x`, or the path: ::, or the end.\")]"
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn decodes_strings_as_the_compiler_does() {
        for (literal, text) in [
            ("\"plain\"", "plain"),
            ("\"a\\n\\r\\t\\0\\\\\\'\\\"z\"", "a\n\r\t\0\\'\"z"),
            ("\"\\x41\\u{1F600}\\u{6_1}\"", "A\u{1F600}a"),
            ("\"one \\\n      two\"", "one two"),
            ("b\"bytes\"", "bytes"),
            ("c\"text\"", "text"),
            ("r\"no \\n escape\"", "no \\n escape"),
            ("r##\"a \"# quote\"##", "a \"# quote"),
            ("br#\"raw bytes\"#", "raw bytes"),
            ("cr\"raw text\"", "raw text"),
        ] {
            assert_eq!(string_value(literal).as_deref(), Some(text), "{literal}");
        }
        // An escape the compiler does not know is not read.
        assert_eq!(string_value("\"\\q\""), None);
    }

    #[test]
    fn rejects_every_other_way_to_name_std() {
        for (source, expected) in [
            (
                "use std as host; fn f() { host::env::var(\"X\"); }",
                BARE_STD,
            ),
            ("extern crate std as host;", BARE_STD),
            ("use ::std as host;", BARE_STD),
            ("library!(std);", BARE_STD),
            ("let std = 1;", BARE_STD),
            ("use std::*;", STD_GLOB),
            ("use std::{sync::Arc, *};", STD_GLOB),
            ("use std::{self as host};", STD_SELF),
            (
                "macro_rules! m { ($module:ident) => { std::$module::var(\"X\") }; }",
                STD_UNREADABLE,
            ),
            ("library!(std::);", STD_UNREADABLE),
        ] {
            assert_eq!(hidden(source), [expected], "{source}");
        }
    }

    #[test]
    fn rejects_macros_that_could_continue_a_path_from_std() {
        for source in [
            "macro_rules! m { ($root:ident) => { $root::env::var(\"X\") }; }",
            "macro_rules! m { ($root:ident, $module:ident) => { $root::$module::var(\"X\") }; }",
        ] {
            assert_eq!(hidden(source), [MACRO_PATH], "{source}");
        }
        for source in [
            "macro_rules! m { ($root:ident) => { use $root as host; }; }",
            "macro_rules! m { ($root:ident) => { use ::$root as host; }; }",
            "macro_rules! m { ($root:ident) => { use {$root as host}; }; }",
            "macro_rules! m { ($root:ident) => { pub use $root::{env}; }; }",
            "macro_rules! m { ($root:ident) => { use $root::*; }; }",
            "macro_rules! m { ($root:ident) => { extern crate $root as host; }; }",
            "macro_rules! m { ($($path:ident)::+) => { use $($path)::+ as host; }; }",
        ] {
            assert_eq!(hidden(source), [MACRO_USE], "{source}");
        }
        // `$crate` names the macro's own crate, and other paths from a metavariable reach no module of std.
        assert_eq!(
            hidden(
                "macro_rules! m { ($name:ident, $value:expr) => { use $crate::limits::MAX; $name::new($value as u64); }; }"
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn refuses_source_it_cannot_follow() {
        assert!(scan("fn f() { /* never closed }").is_err());
        assert!(scan("fn f() { (] }").is_err());
    }

    /// The module of `item`, as code writes it, and the last name of that module.
    fn module_of(item: &Item) -> (&'static str, &'static str) {
        let (module, _) = item.path.rsplit_once("::").unwrap();
        (module, item.owner())
    }

    #[test]
    fn finds_every_item_named_directly_through_use_through_a_group_and_through_a_rename() {
        for item in ITEMS.iter().filter(|item| item.written == Written::Path) {
            let (module, holder) = module_of(item);
            let name = item.name();
            for source in [
                // Directly, and with spaces, comments, line breaks and raw identifiers between the words.
                format!("fn f() {{ let _ = {}; }}", item.path),
                format!("fn f() {{ let _ = ::{}; }}", item.path),
                format!(
                    "fn f() {{ let _ = {} /* a comment */ ::\n r#{name}; }}",
                    module.replace("::", " :: ")
                ),
                // Through `use`, through a group and through a group in a group, and renamed.
                format!("use {}; fn f() {{ let _ = {name}; }}", item.path),
                format!("use {module}::{{self, {name}}};"),
                format!("use std::{{{}::{{{name}}}}};", &module["std::".len()..]),
                format!("use {module}::{{{{{name}}}}};"),
                format!("use {} as renamed;", item.path),
                format!("use {module}::{{{name} as renamed}};"),
                // Through its module, bound under its own name in this file or by a re-export anywhere.
                format!("use {module}; fn f() {{ let _ = {holder}::{name}; }}"),
                format!("fn f() {{ let _ = crate::reexported::{holder}::{name}; }}"),
            ] {
                assert_eq!(items(&source), [item.path], "{source}");
            }
        }
    }

    #[test]
    fn finds_the_methods_of_path_on_any_value_and_through_their_type() {
        for item in ITEMS.iter().filter(|item| item.written == Written::Method) {
            let name = item.name();
            // Its type is forbidden by name, so that a call through it is seen too.
            assert!(
                ITEMS
                    .iter()
                    .any(|other| other.written == Written::Path && other.name() == item.owner()),
                "{}",
                item.path
            );
            for source in [
                // On a value of any origin, such as the `&Path` that cfb's `Entry::path` returns.
                format!("fn f(entry: &Entry) -> bool {{ entry.path().{name}() }}"),
                format!(
                    "fn f(entry: &Entry) -> bool {{ entry . path ( ) /* the path */ .\n r#{name}() }}"
                ),
                // A macro that is handed the call whole, or the method with its `.`.
                format!("check!(entry.path().{name});"),
                format!("check!(entry.path(), .{name});"),
                // Through its type, also in angle brackets.
                format!("fn f(entry: &Entry) -> bool {{ <_>::{name}(entry.path()) }}"),
            ] {
                assert_eq!(items(&source), [item.path], "{source}");
            }
            assert_eq!(
                items(&format!(
                    "fn f(path: &Path) -> bool {{ Path::{name}(path) }}"
                )),
                [item.path]
            );
        }
    }

    #[test]
    fn finds_the_methods_with_names_of_their_own_wherever_their_names_stand() {
        for item in ITEMS.iter().filter(|item| item.written == Written::Name) {
            let name = item.name();
            // No other item has its name, so that wherever it stands it is this method.
            assert_eq!(
                ITEMS.iter().filter(|other| other.name() == name).count(),
                1,
                "{}",
                item.path
            );
            for source in [
                format!("fn f(queue: &Queue) {{ queue.{name}(WAIT); }}"),
                format!(
                    "type Queue = Receiver<u8>; fn f(queue: &Queue) {{ Queue::{name}(queue, WAIT); }}"
                ),
                format!(
                    "macro_rules! call {{ ($value:expr, $method:ident) => {{ $value.$method(WAIT) }}; }} fn f(queue: &Queue) {{ call!(queue, {name}); }}"
                ),
            ] {
                assert!(scanned(&source).items.contains(&(1, item.path)), "{source}");
            }
        }
    }

    #[test]
    fn leaves_other_items_methods_and_dots_alone() {
        assert_eq!(
            items(
                "use std::path::{Component, PathBuf};\nuse std::io::{self, Read, Write};\nuse std::io::prelude::*;\nuse std::thread::{JoinHandle, ThreadId};\nuse bayan_engine::thread::{EngineThread, on_engine_thread};\nfn f(entry: &Entry, scope: Scope) -> io::Result<()> {\n    let path = PathBuf::from(\"/\");\n    let _ = std::thread::current().id();\n    let thread = EngineThread::spawn(config, blobs)?;\n    scope.push(1);\n    let exists = path.components().count() > 0;\n    if !exists { return Ok(()); }\n    let _ = tiny_skia::Path::from_rect(rect);\n    let _ = canonical::canonicalize(&source, &limits);\n    let _ = (0..n, a..=b, x.0, 1.0, 1., self.metadata_part, entry.path);\n    let point = Point { ..Default::default() };\n    Ok(())\n}\nstruct Builder;\npub use canonical::canonicalize;\n"
            ),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn rejects_the_ways_that_could_reach_an_item_unseen() {
        for (source, expected) in [
            // A module that holds items, renamed, whatever path leads to it.
            ("use std::thread as threads;", HOLDER_RENAMED),
            ("use std::{io as console};", HOLDER_RENAMED),
            (
                "use std::thread::{self as threads, current};",
                HOLDER_RENAMED,
            ),
            ("use std::collections::hash_map as maps;", HOLDER_RENAMED),
            ("use crate::reexported::path as paths;", HOLDER_RENAMED),
            // Or all of it imported.
            ("use std::thread::*;", HOLDER_GLOB),
            ("use std::io::{Read, *};", HOLDER_GLOB),
            // Macros that could build a path or a call from their pieces.
            (
                "macro_rules! m { ($item:ident) => { std::thread::$item(work) }; }",
                HOLDER_METAVARIABLE,
            ),
            (
                "macro_rules! m { ($module:path) => { $module::spawn(work) }; }",
                MACRO_ITEM,
            ),
            (
                "macro_rules! m { ($root:ident) => { $root::thread::current() }; }",
                MACRO_ITEM,
            ),
            (
                "macro_rules! m { ($value:expr) => { $value::exists() }; }",
                MACRO_ITEM,
            ),
            (
                "macro_rules! m { ($value:expr, $method:ident) => { $value.$method() }; }",
                METHOD_METAVARIABLE,
            ),
            ("join!(thread, ::spawn);", LEADING_ITEM),
            ("join!(thread, ::, spawn);", LONE_SEPARATOR),
            ("join!(thread::, spawn);", LONE_SEPARATOR),
            ("call!(entry.path(), ., exists);", LONE_DOT),
            ("call!(entry.path()., exists);", LONE_DOT),
        ] {
            assert_eq!(hidden_items(source), [expected], "{source}");
        }
        // A repetition's separator, a turbofish, `->` and `=>` are paths' punctuation, and a number may end in `.`.
        assert_eq!(
            hidden_items(
                "macro_rules! m { ($($part:ident)::+) => { $($part)::+ }; }\nfn f() -> ::core::primitive::u8 { let _ = Vec::<u8>::new(); match 1. { _ => ::core::primitive::u8::MAX } }"
            ),
            Vec::<&str>::new()
        );
    }

    /// An entry of `OUTSIDE_WORLD` for `member` with the whole modules `modules`.
    fn exemption(member: &'static str, modules: &'static [&'static str]) -> Exemption {
        Exemption {
            member,
            uses: Uses::Modules(modules),
            reason: "a tool that runs on the developer's machine",
        }
    }

    /// An entry of `OUTSIDE_WORLD` for `member` with the items `items`.
    fn item_exemption(member: &'static str, items: &'static [&'static str]) -> Exemption {
        Exemption {
            member,
            uses: Uses::Items(items),
            reason: "the desktop host's side of the boundary",
        }
    }

    /// An entry of `APPROVED_ITEMS` for `file` of `member`.
    fn approval(
        member: &'static str,
        file: &'static str,
        items: &'static [&'static str],
    ) -> Approved {
        Approved {
            member,
            file,
            items,
            reason: "it sets the stack size",
        }
    }

    /// A dependency of `from` on `on`.
    fn dependency(from: &str, on: &str, dev: bool) -> MemberDependency {
        MemberDependency {
            from: from.to_owned(),
            on: on.to_owned(),
            dev,
        }
    }

    #[test]
    fn accepts_a_list_of_members_whose_dependents_are_only_tests() {
        let line = check_list(
            &["engine", "tool", "binding"],
            &[
                dependency("tool", "engine", false),
                dependency("engine", "tool", true),
            ],
            &[
                exemption("tool", &["std::fs", "std::env"]),
                item_exemption("binding", &["std::process::abort", "std::time::Instant"]),
            ],
            &[approval(
                "engine",
                "crates/engine/src/thread.rs",
                &["std::thread::Builder"],
            )],
        )
        .unwrap();
        assert!(
            line.contains("is a workspace member (tool, binding)")
                && line.ends_with("APPROVED_ITEMS approves 1 items of ITEMS for 1 files"),
            "{line}"
        );
    }

    #[test]
    fn refuses_a_list_that_is_not_right() {
        let problem = check_list(
            &["engine", "tool", "binding"],
            &[],
            &[
                exemption("gone", &["std::fs"]),
                exemption("tool", &[]),
                exemption("tool", &["std::io", "std::fs", "std::fs"]),
                item_exemption(
                    "binding",
                    &[
                        "std::process",
                        "std::io::stdout",
                        "std::time::Instant::now",
                        "std::time::Instant",
                        "std::time::Instant",
                    ],
                ),
            ],
            &[
                approval("gone", "crates/gone/src/lib.rs", &["std::thread::Builder"]),
                approval("tool", "crates/tool/src/main.rs", &["std::io::stdout"]),
                approval("engine", "crates/engine/../engine/src/lib.rs", &[]),
                approval(
                    "engine",
                    "crates/engine/src/a.rs",
                    &[
                        "std::thread::Builder",
                        "std::thread::current",
                        "std::thread::Builder",
                    ],
                ),
                approval("engine", "crates/engine/src/a.rs", &["std::thread::spawn"]),
            ],
        )
        .unwrap_err();
        for expected in [
            "`gone` is not a workspace member",
            "`tool` is listed twice",
            "the entry for `tool` lists no module",
            "lists `std::io`, which is not one of std::env, std::fs, std::net, std::process, std::time",
            "lists `std::fs` twice",
            "lists `std::process`, which is not an item of one of",
            "lists `std::io::stdout`, which is not an item of one of",
            "lists `std::time::Instant::now`, which is not an item of one of",
            "lists `std::time::Instant` twice",
            "APPROVED_ITEMS: `gone` is not a workspace member",
            "APPROVED_ITEMS: OUTSIDE_WORLD lists whole modules for `tool`",
            "APPROVED_ITEMS: crates/engine/../engine/src/lib.rs is not the path of a Rust file",
            "APPROVED_ITEMS: the entry for crates/engine/../engine/src/lib.rs lists no item",
            "lists `std::thread::current`, which ITEMS in xtask/src/outside_world.rs does not list",
            "the entry for crates/engine/src/a.rs lists `std::thread::Builder` twice",
            "APPROVED_ITEMS: crates/engine/src/a.rs of `engine` is listed twice",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
    }

    #[test]
    fn refuses_code_that_ships_depending_on_a_listed_member() {
        let problem = check_list(
            &["engine", "tool"],
            &[dependency("engine", "tool", false)],
            &[exemption("tool", &["std::fs"])],
            &[],
        )
        .unwrap_err();
        assert!(problem.contains("  engine depends on tool"), "{problem}");
    }

    #[test]
    fn exempts_from_the_compile_time_environment_only_the_members_with_all_of_std_env() {
        assert_eq!(
            environment_exempt(&[
                exemption("tool", &["std::fs", "std::env"]),
                exemption("spike", &["std::fs", "std::time"]),
                item_exemption("binding", &["std::env::var", "std::time::Instant"]),
            ]),
            ["tool"]
        );
    }

    /// A folder for one test's files, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let folder = std::env::temp_dir()
                .join(format!("bayan-outside-world-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            Self(std::fs::canonicalize(&folder).unwrap())
        }

        /// Writes `text` to `name` in the folder and returns its path.
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn checks_the_code_that_ships_against_the_list() {
        let scratch = Scratch::new("code");
        let engine = scratch.write("engine.rs", "use std::sync::Arc;\nfn now() { std::time::Instant::now(); }\n#[cfg(test)]\nmod tests { use std::fs; }\n");
        let tool = scratch.write(
            "tool.rs",
            "use std::{env, fs};\nfn main() { let _ = std::net::TcpStream::connect(\"x:1\"); std::thread::sleep(WAIT); }\n",
        );
        let hiding = scratch.write("hiding.rs", "use std as host;\n");
        let shipped = BTreeMap::from([
            ("engine".to_owned(), BTreeSet::from([engine])),
            ("tool".to_owned(), BTreeSet::from([tool])),
            ("other".to_owned(), BTreeSet::from([hiding])),
        ]);
        let problem = check_code(
            &scratch.0,
            &shipped,
            &[exemption("tool", &["std::env", "std::fs", "std::process"])],
            &[],
        )
        .unwrap_err();
        for expected in [
            USED,
            "  engine.rs:2: std::time\n",
            // A test module written in place ships as far as the check can tell.
            "  engine.rs:4: std::fs\n",
            "  tool.rs:2: std::net (OUTSIDE_WORLD lists only std::env, std::fs, std::process for tool)",
            HIDDEN.0,
            "  hiding.rs:1: `std` on its own",
            STALE.0,
            "  std::process, for tool",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
        // A tool that may use whole modules is not checked for ITEMS.
        assert!(!problem.contains("std::thread::sleep"), "{problem}");
    }

    #[test]
    fn allows_an_entry_with_items_exactly_those_items() {
        let scratch = Scratch::new("items");
        let binding = scratch.write(
            "binding.rs",
            "use std::time::Instant;\nfn stop() { std::process::abort(); }\n",
        );
        let shipped = BTreeMap::from([("binding".to_owned(), BTreeSet::from([binding.clone()]))]);
        let exemptions = [item_exemption(
            "binding",
            &["std::process::abort", "std::time::Instant"],
        )];
        let line = check_code(&scratch.0, &shipped, &exemptions, &[])
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.contains("that of the members it names with items (binding; 1 files) names exactly the items it lists for them"),
            "{line}"
        );
        // Another item of an allowed module, the module on its own, all of it, and an item of ITEMS fail.
        std::fs::write(
            &binding,
            "use std::time::Instant;\nfn stop() { std::process::abort(); }\nfn run() { std::process::Command::new(\"x\"); }\nfn now() { std::time::SystemTime::now(); }\nuse std::time;\nuse std::process::*;\nfn wait() { std::thread::sleep(WAIT); }\n",
        )
        .unwrap();
        let problem = check_code(&scratch.0, &shipped, &exemptions, &[]).unwrap_err();
        for expected in [
            USED,
            "  binding.rs:3: std::process::Command (OUTSIDE_WORLD lists only the items std::process::abort, std::time::Instant for binding)",
            "  binding.rs:4: std::time::SystemTime (OUTSIDE_WORLD lists only the items",
            "  binding.rs:5: std::time on its own (OUTSIDE_WORLD lists only the items",
            "  binding.rs:6: std::process followed by something other than an item's name (",
            ITEMS_USED.0,
            "  binding.rs:7: std::thread::sleep, which reaches the clock",
        ] {
            assert!(problem.contains(expected), "{expected}:\n{problem}");
        }
        // An item that the code no longer uses is stale.
        std::fs::write(&binding, "use std::time::Instant;\n").unwrap();
        let problem = check_code(&scratch.0, &shipped, &exemptions, &[]).unwrap_err();
        assert!(
            problem.contains(&format!(
                "{}:\n  std::process::abort, for binding (listed because the desktop host's side of the boundary)\n",
                STALE.0
            )),
            "{problem}"
        );
    }

    #[test]
    fn allows_an_item_only_in_the_file_that_approves_it() {
        let scratch = Scratch::new("approved");
        std::fs::create_dir_all(scratch.0.join("src")).unwrap();
        let thread = scratch.write(
            "src/thread.rs",
            "fn spawn() { std::thread::Builder::new().stack_size(SIZE).spawn(run); }\n",
        );
        let other = scratch.write("src/other.rs", "fn other() {}\n");
        let shipped =
            BTreeMap::from([("engine".to_owned(), BTreeSet::from([thread, other.clone()]))]);
        let approved = [approval(
            "engine",
            "src/thread.rs",
            &["std::thread::Builder"],
        )];
        let line = check_code(&scratch.0, &shipped, &[], &approved)
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.contains("(engine; 2 files) names none of std::env, std::fs, std::net, std::process, std::time and uses none of the 30 items of ITEMS but the 1 that APPROVED_ITEMS approves for their files"),
            "{line}"
        );
        // The same item in another file fails, and so does an approval that approves nothing.
        std::fs::write(
            &other,
            "fn other() { let _ = std::thread::Builder::new(); }\n",
        )
        .unwrap();
        let problem = check_code(&scratch.0, &shipped, &[], &approved).unwrap_err();
        let other_file = ["src", "other.rs"]
            .iter()
            .collect::<PathBuf>()
            .display()
            .to_string();
        assert!(
            problem.contains(&format!(
                "  {other_file}:1: std::thread::Builder, which reaches the environment, unless the builder sets the stack size"
            )),
            "{problem}"
        );
        let stale = [approval("engine", "src/other.rs", &["std::thread::spawn"])];
        let problem = check_code(&scratch.0, &shipped, &[], &stale).unwrap_err();
        assert!(
            problem.contains("  std::thread::spawn, in src/other.rs of engine (approved because it sets the stack size)"),
            "{problem}"
        );
    }

    #[test]
    fn passes_code_that_uses_exactly_what_the_list_says() {
        let scratch = Scratch::new("passes");
        let engine = scratch.write(
            "engine.rs",
            "use std::sync::Arc;\npub const VERSION: &str = env!(\"CARGO_PKG_VERSION\");\n",
        );
        let tool = scratch.write(
            "tool.rs",
            "use std::{env, fs};\nfn main() { let _ = std::io::stdout(); }\n",
        );
        let shipped = BTreeMap::from([
            ("engine".to_owned(), BTreeSet::from([engine])),
            ("tool".to_owned(), BTreeSet::from([tool])),
        ]);
        let line = check_code(
            &scratch.0,
            &shipped,
            &[exemption("tool", &["std::env", "std::fs"])],
            &[],
        )
        .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.contains(
                "the members that OUTSIDE_WORLD does not name (engine; 1 files) names none of std::env, std::fs, std::net, std::process, std::time and uses none of the 30 items of ITEMS"
            ) && line.ends_with("that of the members it names with whole modules (1 files) names exactly the modules it lists for them"),
            "{line}"
        );
    }
}
