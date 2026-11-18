//! Which code raised a panic, for the panic hook of `crdt_import` (work package CORE-010).
//!
//! libfuzzer-sys's own hook aborts the process on every panic, before anything can catch it, so that a panic which the code under test would catch still fails the run (`initialize` in libfuzzer-sys 0.4.13's `src/lib.rs`). That is right for every target but `crdt_import`: `bayan_crdt::Doc::import` contains the panics of Loro, which the owner accepted with that guard (ADR-0008, amendment of 2026-10-08), and poisons the document. So `crdt_import` replaces the hook with one that lets a panic unwind only when it is Loro's, and hands every other panic to libfuzzer-sys's hook, which aborts.
//!
//! **Loro's** panics are decided by crate, as the brief defines them: Loro's packages are `loro` and the packages that only Loro brings into `fuzz/Cargo.lock` ([`loro_packages`], which reads the lockfile compiled into the target, so that the list cannot go stale), not packages that BayanDocs' own crates or the fuzzing harness also use. A panic is Loro's when, counting outward from the frame that raised it, the first frame outside the standard library belongs to one of those packages ([`owner`]). The frame that raised it is the one that the backtrace lists right after the frames of `std::panicking` and `core::panicking`; the frames before those are the hook's own and the backtrace's, which must not count, and `std::panicking` appears again further out wherever a caller catches panics, which must not count either. So [`owner`] starts at the first frame of `std::panicking` or `core::panicking` and skips every frame in the standard library's files, the rest of the panicking machinery and a frame of std that raised the panic for its caller (such as `Option::unwrap`) among them. A frame belongs to the package whose source folder holds its file, as the target's debug information names it: `…/registry/src/<index>/<name>-<version>/…` for a package from crates.io, and one of the standard library's two folders (below) for the standard library. The panic's location alone is not enough: a panic that std, alloc or one of Loro's own dependencies raises for Loro carries a location outside the `loro` crate, and a panic that BayanDocs code raises inside a call from Loro carries Loro's packages further out, which must not count either.
//!
//! **The standard library's files** have one of two names in the debug information ([`StdLibrary`]). The code that the toolchain compiled in advance, such as `std::panicking` and `core::option::unwrap_failed`, is always named `/rustc/<commit>/library/…`. The standard library's code that the compiler compiles into the target's own crates, its generic functions such as `Option::unwrap` and the functions it inlines, is named the same way unless the toolchain has its rust-src component, the standard library's source code, which `scripts/dev-setup.sh` installs on the fuzz nightly for Miri: the compiler then names that code by its place in the component, `<sysroot>/lib/rustlib/src/rust/library/…`, such as `/root/.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/option.rs`. That folder is not written into the code, and no environment variable or file is read for it: when the hook is installed, [`StdLibrary::of_this_program`] reads it from the program's own debug information, from the frame of `Option::get_or_insert_with`, a generic function of the standard library that the compiler compiled into this crate, in a backtrace that it takes through that function. So only the very folder in which the compiler that built the program found the standard library counts, never another folder of that shape, such as one inside a package of the lockfile, inside BayanDocs' crates or the fuzz targets, or in a build script's output. The names that this module reads also depend on the process's current folder: in the text form of a backtrace (`{:?}`), which [`frames`] reads, std writes every file below the current folder as `./…`. Such a name never matches either folder, so the current folder can only make the hook refuse to install, or count a frame as other code, never accept more: a target started from `/` refuses to start (`/rustc/…` becomes `./rustc/…`), and where rust-src is installed, so does one started from a folder above the toolchain, such as the home folder that holds `~/.rustup`. The targets are therefore started from inside the repository, as the commands in `fuzz/README.md` are. In both folders a file counts as the standard library's only while it stays below the folder once `..` is resolved: the standard library names some of its own files with `..` (`core/src/../../stdarch/…`), and a name with `..` could otherwise lead out of the folder. Cargo does not rebuild a crate when rust-src is installed or removed, so a target whose crates were compiled partly before and partly after such a change can name the standard library's code in a folder that its own frame does not show; that code then counts as other code and fails the run, and building the targets again from clean (`cargo clean` in `fuzz/`) puts it right.
//!
//! Everything that this module cannot read for certain counts as not Loro's, so that it fails the run: a backtrace that std did not capture, a frame without a file where the decision falls, a file in no package of the lockfile, a lockfile in a layout that the parser does not know, and a backtrace through `Option::get_or_insert_with` that does not show where the standard library is (for these two, the hook is then not installed and the target stops before fuzzing).

use std::backtrace::Backtrace;
use std::collections::BTreeSet;
use std::fmt;

/// A package of a lockfile: its name and version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Package {
    pub name: String,
    pub version: String,
}

impl fmt::Display for Package {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.name, self.version)
    }
}

/// One package of a lockfile with the dependencies it lists, as Cargo writes them (`name`, or `name version` when the lockfile has several versions of that name).
#[derive(Debug, Clone)]
struct Entry {
    package: Package,
    dependencies: Vec<String>,
}

/// The packages of a `Cargo.lock`, read in the layout that Cargo writes (format version 4).
#[derive(Debug, Clone)]
pub struct Lockfile {
    entries: Vec<Entry>,
}

impl Lockfile {
    /// Reads a lockfile, and fails on any line that is not part of the layout that Cargo writes: the header comments, `version = 4`, and `[[package]]` tables of `name`, `version`, `source`, `checksum` and `dependencies`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut entries: Vec<Entry> = Vec::new();
        let mut version_seen = false;
        let mut in_dependencies = false;
        for (number, line) in text.lines().enumerate() {
            let number = number + 1;
            let unexpected = || format!("fuzz/Cargo.lock, line {number}: unexpected line `{line}`");
            if in_dependencies {
                if line == "]" {
                    in_dependencies = false;
                    continue;
                }
                let dependency = line
                    .strip_prefix(" \"")
                    .and_then(|rest| rest.strip_suffix("\","))
                    .ok_or_else(unexpected)?;
                entries
                    .last_mut()
                    .ok_or_else(unexpected)?
                    .dependencies
                    .push(dependency.to_owned());
                continue;
            }
            if line.is_empty() || line.starts_with("# ") {
                continue;
            }
            if line == "version = 4" && entries.is_empty() && !version_seen {
                version_seen = true;
                continue;
            }
            if line == "[[package]]" {
                entries.push(Entry {
                    package: Package {
                        name: String::new(),
                        version: String::new(),
                    },
                    dependencies: Vec::new(),
                });
                continue;
            }
            let entry = entries.last_mut().ok_or_else(unexpected)?;
            if let Some(name) = quoted(line, "name") {
                entry.package.name = name.to_owned();
            } else if let Some(version) = quoted(line, "version") {
                entry.package.version = version.to_owned();
            } else if quoted(line, "source").is_some() || quoted(line, "checksum").is_some() {
            } else if line == "dependencies = [" {
                in_dependencies = true;
            } else {
                return Err(unexpected());
            }
        }
        if !version_seen || in_dependencies {
            return Err("fuzz/Cargo.lock is not a lockfile of format version 4 that ends where Cargo ends one".to_owned());
        }
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.package.name.is_empty() || entry.package.version.is_empty())
        {
            return Err(format!(
                "fuzz/Cargo.lock has a package without a name or a version ({entry:?})"
            ));
        }
        Ok(Self { entries })
    }

    /// Every package of the lockfile.
    pub fn packages(&self) -> impl Iterator<Item = &Package> {
        self.entries.iter().map(|entry| &entry.package)
    }

    /// The entry that a dependency of a lockfile names: `name`, or `name version` when the name is ambiguous.
    fn resolve(&self, dependency: &str) -> Result<usize, String> {
        let mut parts = dependency.split(' ');
        let name = parts.next().unwrap_or_default();
        let version = parts.next();
        let candidates: Vec<usize> = (0..self.entries.len())
            .filter(|&index| {
                let package = &self.entries[index].package;
                package.name == name && version.is_none_or(|version| package.version == version)
            })
            .collect();
        match candidates.as_slice() {
            [index] => Ok(*index),
            _ => Err(format!(
                "fuzz/Cargo.lock lists the dependency `{dependency}`, which names {} of its packages, not one",
                candidates.len()
            )),
        }
    }

    /// The only package called `name`.
    fn only(&self, name: &str) -> Result<usize, String> {
        self.resolve(name)
    }

    /// The packages that `from` reaches through the dependencies of the lockfile, without passing through `avoid`.
    fn reach(&self, from: usize, avoid: Option<usize>) -> Result<BTreeSet<usize>, String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(index) = stack.pop() {
            if Some(index) == avoid || !seen.insert(index) {
                continue;
            }
            for dependency in &self.entries[index].dependencies {
                stack.push(self.resolve(dependency)?);
            }
        }
        Ok(seen)
    }
}

/// The value of `key = "value"`.
fn quoted<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.strip_prefix(key)?
        .strip_prefix(" = \"")?
        .strip_suffix('"')
}

/// Loro's packages in `lockfile`, whose root package is `root`: `loro` and every package that `loro` reaches and `root` does not reach without passing through `loro`.
pub fn loro_packages(lockfile: &Lockfile, root: &str) -> Result<BTreeSet<Package>, String> {
    let root = lockfile.only(root)?;
    let loro = lockfile.only("loro")?;
    let without_loro = lockfile.reach(root, Some(loro))?;
    let through_loro = lockfile.reach(loro, None)?;
    if through_loro.contains(&root) {
        return Err("loro depends on the fuzzing workspace itself".to_owned());
    }
    Ok(through_loro
        .difference(&without_loro)
        .map(|&index| lockfile.entries[index].package.clone())
        .collect())
}

/// One frame of a backtrace, as std's `Backtrace` writes it with `{:?}`: the function's name and, with debug information, its file and line. An inlined function is a frame of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// Reads the frames of `format!("{:?}", backtrace)` for a backtrace that std captured: `Backtrace [{ fn: "name", file: "path", line: 12 }, …]`, where `fn: <unknown>` stands for a frame without a name and the file and line are left out when there is no debug information. Anything else, such as `<disabled>` or `<unsupported>`, is not a backtrace this module can read.
pub fn frames(debug: &str) -> Result<Vec<Frame>, String> {
    let list = debug
        .strip_prefix("Backtrace [")
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| {
            format!(
                "std captured no backtrace that can be read: {}",
                debug.chars().take(40).collect::<String>()
            )
        })?;
    if list.is_empty() {
        return Ok(Vec::new());
    }
    let inner = list
        .strip_prefix("{ ")
        .and_then(|rest| rest.strip_suffix(" }"))
        .ok_or("a backtrace whose frames are not written as `{ … }`")?;
    inner.split(" }, { ").map(frame).collect()
}

fn frame(text: &str) -> Result<Frame, String> {
    let unreadable = || format!("a frame of the backtrace that cannot be read: {text}");
    let rest = text.strip_prefix("fn: ").ok_or_else(unreadable)?;
    let (function, mut rest) = if let Some(rest) = rest.strip_prefix("<unknown>") {
        (None, rest)
    } else {
        let rest = rest.strip_prefix('"').ok_or_else(unreadable)?;
        // A name ends at the quote before the next field, or at the end of the frame.
        let end = [rest.find("\", file: \""), rest.find("\", line: ")]
            .into_iter()
            .flatten()
            .min()
            .or_else(|| rest.strip_suffix('"').map(str::len))
            .ok_or_else(unreadable)?;
        (Some(rest[..end].to_owned()), &rest[end + 1..])
    };
    let mut file = None;
    if let Some(after) = rest.strip_prefix(", file: \"") {
        let end = after
            .find("\", line: ")
            .or_else(|| after.strip_suffix('"').map(str::len))
            .ok_or_else(unreadable)?;
        file = Some(after[..end].to_owned());
        rest = &after[end + 1..];
    }
    let mut line = None;
    if let Some(after) = rest.strip_prefix(", line: ") {
        line = Some(after.parse().map_err(|_| unreadable())?);
        rest = "";
    }
    if !rest.is_empty() {
        return Err(unreadable());
    }
    Ok(Frame {
        function,
        file,
        line,
    })
}

/// The two folders of the standard library's files in the program's debug information (see the module's documentation): `/rustc/<commit>/library/`, and the `library` folder of the toolchain's rust-src component when the compiler named the standard library's code that it compiled into the program's own crates there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdLibrary {
    /// The `library` folder of the toolchain's rust-src component, `<sysroot>/lib/rustlib/src/rust/library`, when the program's debug information names the standard library's code that the compiler compiled into its crates there; `None` when it names that code `/rustc/<commit>/library/…` too.
    rust_src: Option<String>,
}

impl StdLibrary {
    /// Where this program's debug information puts the standard library's files, read from a backtrace that this function takes through `Option::get_or_insert_with`. That function of the standard library is generic, so the compiler compiled it into this crate and named its file as it named the standard library's code in every crate of the program; it calls `Backtrace::force_capture` itself, so the backtrace starts in it (std leaves out its own frames that take the backtrace), and this function's frame follows. No environment variable is read for it, and no file but the program's debug information, which every backtrace reads. The names also depend on the current folder, below which std writes every file as `./…` in the text form of a backtrace; such a name matches neither folder, so a program started from `/`, or with rust-src from a folder above the toolchain, gets an error here and never a folder that it should not accept (see the module's documentation).
    ///
    /// # Errors
    ///
    /// When std captured no backtrace that can be read, or its frames do not show the standard library's `core/src/option.rs` in one of the two folders right before this function's own frame ([`StdLibrary::from_frames`]).
    pub fn of_this_program() -> Result<Self, String> {
        // Not `Backtrace::force_capture()` itself: the standard library's code must call it, so that the backtrace starts in that code.
        let mut slot = None;
        let backtrace = slot.get_or_insert_with(Backtrace::force_capture);
        Self::from_frames(&frames(&format!("{backtrace:?}"))?, file!())
    }

    /// Where the standard library's files are, from the frames of the backtrace that [`StdLibrary::of_this_program`] takes in the file `own_file` (`file!()`, this module's file as the compiler was given it). The frames before the first one in that file must all be the standard library's code that called `Backtrace::force_capture`, the last of them `Option::get_or_insert_with` in `<library>/core/src/option.rs`, where `<library>` is either `/rustc/<commit>/library` or the `library` folder of a rust-src component, an absolute folder that ends in `lib/rustlib/src/rust/library` and has no empty part, `.` or `..`.
    fn from_frames(frames: &[Frame], own_file: &str) -> Result<Self, String> {
        let unreadable = |what: String| {
            format!(
                "the backtrace through `Option::get_or_insert_with` does not show where the standard library's files are: {what}"
            )
        };
        let is_own = |frame: &Frame| {
            frame.file.as_deref().is_some_and(|file| {
                file.strip_suffix(own_file)
                    .is_some_and(|folder| folder.is_empty() || folder.ends_with('/'))
            })
        };
        let own = frames
            .iter()
            .position(is_own)
            .ok_or_else(|| unreadable(format!("none of its frames is in {own_file}")))?;
        let Some(caller) = own.checked_sub(1).map(|index| &frames[index]) else {
            return Err(unreadable(format!("it starts in {own_file}")));
        };
        let library = caller
            .file
            .as_deref()
            .and_then(|file| file.strip_suffix("/core/src/option.rs"))
            .ok_or_else(|| unreadable(format!("the frame that called {own_file} is {caller:?}")))?;
        let std_library = if is_rustc_library(library) {
            Self { rust_src: None }
        } else if is_rust_src_library(library) {
            Self {
                rust_src: Some(library.to_owned()),
            }
        } else {
            return Err(unreadable(format!(
                "`core/src/option.rs` is in {library}, which is neither `/rustc/<commit>/library` nor the `library` folder of a rust-src component"
            )));
        };
        if let Some(frame) = frames[..own].iter().find(|frame| {
            !frame
                .file
                .as_deref()
                .is_some_and(|file| std_library.holds(file))
        }) {
            return Err(unreadable(format!(
                "it starts with {frame:?}, which is not in the standard library's folders"
            )));
        }
        Ok(std_library)
    }

    /// Whether `file` is one of the standard library's files: below `/rustc/<commit>/library/` or below the rust-src component's `library` folder, and still below it once `..` is resolved.
    fn holds(&self, file: &str) -> bool {
        let below_rustc = file
            .strip_prefix("/rustc/")
            .and_then(|rest| rest.split_once('/'))
            .filter(|(commit, _)| is_commit(commit))
            .and_then(|(_, rest)| rest.strip_prefix("library/"));
        let below_rust_src = self
            .rust_src
            .as_deref()
            .and_then(|library| file.strip_prefix(library)?.strip_prefix('/'));
        below_rustc.is_some_and(stays_below) || below_rust_src.is_some_and(stays_below)
    }
}

/// Whether `commit` is a commit as the toolchain names it in `/rustc/<commit>/`: 40 hexadecimal digits.
fn is_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `folder` is `/rustc/<commit>/library`.
fn is_rustc_library(folder: &str) -> bool {
    folder
        .strip_prefix("/rustc/")
        .and_then(|rest| rest.strip_suffix("/library"))
        .is_some_and(is_commit)
}

/// Whether `folder` has the shape of a rust-src component's `library` folder: an absolute folder that ends in `lib/rustlib/src/rust/library` and has no empty part, `.` or `..`.
fn is_rust_src_library(folder: &str) -> bool {
    folder.strip_prefix('/').is_some_and(|path| {
        let parts: Vec<&str> = path.split('/').collect();
        parts.ends_with(&["lib", "rustlib", "src", "rust", "library"])
            && parts.iter().all(|part| !matches!(*part, "" | "." | ".."))
    })
}

/// Whether `path`, relative to a folder, names something below that folder: it has no empty part and no `.`, and no `..` in it leads out of the folder.
fn stays_below(path: &str) -> bool {
    let mut depth = 0_usize;
    for part in path.split('/') {
        match part {
            "" | "." => return false,
            ".." => match depth.checked_sub(1) {
                Some(up) => depth = up,
                None => return false,
            },
            _ => depth += 1,
        }
    }
    depth > 0
}

/// Whose code a file is, by the folder that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Rust's standard library (`core`, `alloc`, `std` and the crates inside it), in one of its two folders ([`StdLibrary`]): `/rustc/<commit>/library/…`, or the `library` folder of the toolchain's rust-src component that the program's own frame showed.
    Std,
    /// A package of the lockfile, in Cargo's registry: `…/registry/src/<index>/<name>-<version>/…`.
    Package(Package),
    /// Any other file, such as BayanDocs' own crates and the fuzz targets.
    Other(String),
}

/// Whose code `file` is, for the packages of `lockfile` and the standard library's folders `std_library`.
pub fn source(file: &str, lockfile: &Lockfile, std_library: &StdLibrary) -> Source {
    if std_library.holds(file) {
        return Source::Std;
    }
    let parts: Vec<&str> = file.split('/').collect();
    for window in parts.windows(4) {
        if let ["registry", "src", _index, folder] = window
            && let Some(package) = lockfile
                .packages()
                .find(|package| *folder == format!("{}-{}", package.name, package.version))
        {
            return Source::Package(package.clone());
        }
    }
    Source::Other(file.to_owned())
}

/// Whose a panic is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// Loro's: the run goes on, and `Doc::import` contains the panic.
    Loro(Package),
    /// Not Loro's, with the reason: the run fails.
    NotLoro(String),
}

/// Whose a panic is, from the frames of a backtrace captured in the panic hook (see the module's documentation), in a program whose standard library's files are in `std_library`.
pub fn owner(
    frames: &[Frame],
    lockfile: &Lockfile,
    loro: &BTreeSet<Package>,
    std_library: &StdLibrary,
) -> Owner {
    let is_panicking = |frame: &Frame| {
        frame.function.as_deref().is_some_and(|name| {
            name.starts_with("std::panicking::") || name.starts_with("core::panicking::")
        })
    };
    // The first such frame from the hook's end of the backtrace: further out, std::panicking appears again wherever a caller catches panics (std::panicking::catch_unwind, in `Doc::import`, in libfuzzer-sys and in the test harness).
    let Some(first) = frames.iter().position(is_panicking) else {
        return Owner::NotLoro(
            "the backtrace has no frame of std::panicking or core::panicking".to_owned(),
        );
    };
    // The rest of the panicking machinery, the frame that raised the panic if std raised it for its caller (such as `Option::unwrap`), and std's frames around it are all in the standard library's files.
    for frame in &frames[first + 1..] {
        let Some(file) = &frame.file else {
            return Owner::NotLoro(format!(
                "the frame {:?} has no file",
                frame.function.as_deref().unwrap_or("<unknown>")
            ));
        };
        match source(file, lockfile, std_library) {
            Source::Std => {}
            Source::Package(package) if loro.contains(&package) => return Owner::Loro(package),
            Source::Package(package) => {
                return Owner::NotLoro(format!(
                    "raised in {package}, which is not one of Loro's packages"
                ));
            }
            Source::Other(file) => return Owner::NotLoro(format!("raised in {file}")),
        }
    }
    Owner::NotLoro("every frame after the panic's is in the standard library".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCKFILE: &str = "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"bayan-crdt\"\nversion = \"0.0.0\"\ndependencies = [\n \"loro\",\n]\n\n[[package]]\nname = \"bayan-fuzz\"\nversion = \"0.0.0\"\ndependencies = [\n \"bayan-crdt\",\n \"serde_json\",\n]\n\n[[package]]\nname = \"generic-btree\"\nversion = \"0.10.7\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\ndependencies = [\n \"serde_json\",\n]\n\n[[package]]\nname = \"loro\"\nversion = \"1.16.2\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\ndependencies = [\n \"generic-btree\",\n \"serde_json\",\n]\n\n[[package]]\nname = \"serde_json\"\nversion = \"1.0.151\"\nsource = \"registry+https://github.com/rust-lang/crates.io-index\"\nchecksum = \"00\"\n";

    const REGISTRY: &str = "/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f";
    const STD: &str = "/rustc/0123456789abcdef0123456789abcdef01234567/library";
    /// The `library` folder of a toolchain's rust-src component, where the compiler names the standard library's code that it compiles into the program's crates once that component is installed.
    const RUST_SRC: &str = "/root/.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library";
    /// This module's file, as `file!()` gives it.
    const OWN_FILE: &str = "src/loro_panics.rs";

    fn lockfile() -> Lockfile {
        Lockfile::parse(LOCKFILE).unwrap()
    }

    /// A program whose crates name the standard library's code `/rustc/<commit>/library/…`, as where the toolchain has no rust-src component.
    fn without_rust_src() -> StdLibrary {
        StdLibrary { rust_src: None }
    }

    /// A program whose crates name the standard library's code in [`RUST_SRC`].
    fn with_rust_src() -> StdLibrary {
        StdLibrary {
            rust_src: Some(RUST_SRC.to_owned()),
        }
    }

    fn loro() -> BTreeSet<Package> {
        loro_packages(&lockfile(), "bayan-fuzz").unwrap()
    }

    fn at(function: &str, file: &str) -> Frame {
        Frame {
            function: Some(function.to_owned()),
            file: Some(file.to_owned()),
            line: Some(7),
        }
    }

    /// The frames that every backtrace captured in the hook starts with: the hook's own, then std's and core's panicking machinery.
    fn panicking() -> Vec<Frame> {
        vec![
            at("bayan_fuzz::loro_panics::hook", "./src/loro_panics.rs"),
            at(
                "<alloc::boxed::Box<F,A> as core::ops::function::Fn<Args>>::call",
                &format!("{STD}/alloc/src/boxed.rs"),
            ),
            at(
                "std::panicking::panic_with_hook",
                &format!("{STD}/std/src/panicking.rs"),
            ),
            at(
                "std::panicking::panic_handler::{{closure}}",
                &format!("{STD}/std/src/panicking.rs"),
            ),
            at(
                "std::sys::backtrace::__rust_end_short_backtrace",
                &format!("{STD}/std/src/sys/backtrace.rs"),
            ),
            at(
                "__rustc::rust_begin_unwind",
                &format!("{STD}/std/src/panicking.rs"),
            ),
            at(
                "core::panicking::panic_fmt",
                &format!("{STD}/core/src/panicking.rs"),
            ),
            at(
                "core::panicking::panic",
                &format!("{STD}/core/src/panicking.rs"),
            ),
        ]
    }

    fn panic_from(frames: &[Frame]) -> Owner {
        panic_in(&with_rust_src(), frames)
    }

    /// Whose a panic is in a program whose standard library's files are in `std_library`, with `frames` after the panicking ones.
    fn panic_in(std_library: &StdLibrary, frames: &[Frame]) -> Owner {
        owner(
            &[panicking(), frames.to_vec()].concat(),
            &lockfile(),
            &loro(),
            std_library,
        )
    }

    /// The frames of the backtrace that [`StdLibrary::of_this_program`] takes, in a program whose crates name the standard library's code below `library` and this module's file `own`.
    fn through_get_or_insert_with(library: &str, own: &str) -> Vec<Frame> {
        vec![
            at(
                "<<std::backtrace::Backtrace>::force_capture as core::ops::function::FnOnce<()>>::call_once",
                &format!("{library}/core/src/ops/function.rs"),
            ),
            at(
                "<core::option::Option<std::backtrace::Backtrace>>::get_or_insert_with::<<std::backtrace::Backtrace>::force_capture>",
                &format!("{library}/core/src/option.rs"),
            ),
            at("bayan_fuzz::loro_panics::StdLibrary::of_this_program", own),
            at("bayan_fuzz::crdt::install_panic_hook_for", "./src/crdt.rs"),
        ]
    }

    #[test]
    fn loro_s_packages_are_those_only_loro_brings_in() {
        let names: Vec<String> = loro().into_iter().map(|package| package.name).collect();
        assert_eq!(names, ["generic-btree", "loro"]);
    }

    #[test]
    fn the_real_lockfile_gives_loro_and_its_own_dependencies() {
        let lockfile = Lockfile::parse(crate::LOCKFILE).unwrap();
        let loro = loro_packages(&lockfile, "bayan-fuzz").unwrap();
        let names: BTreeSet<&str> = loro.iter().map(|package| package.name.as_str()).collect();
        for name in [
            "loro",
            "loro-internal",
            "loro-common",
            "loro-delta",
            "generic-btree",
        ] {
            assert!(names.contains(name), "{name}");
        }
        // Used by BayanDocs' own crates too, so not Loro's.
        for name in [
            "serde_json",
            "serde",
            "memchr",
            "libc",
            "bayan-crdt",
            "bayan-fuzz",
            "libfuzzer-sys",
        ] {
            assert!(!names.contains(name), "{name}");
        }
    }

    #[test]
    fn refuses_a_lockfile_in_another_layout() {
        assert!(Lockfile::parse("version = 3\n").is_err());
        assert!(Lockfile::parse(&LOCKFILE.replace("checksum = \"00\"", "checksum = 00")).is_err());
        assert!(Lockfile::parse(&LOCKFILE.replace(" \"loro\",\n]", " \"loro\",\n")).is_err());
        assert!(Lockfile::parse(&format!("{LOCKFILE}\n[metadata]\n")).is_err());
        assert!(
            loro_packages(
                &Lockfile::parse(&LOCKFILE.replace(
                    "\"serde_json\",\n]\n\n[[package]]\nname = \"generic",
                    "\"serde_json 9\",\n]\n\n[[package]]\nname = \"generic"
                ))
                .unwrap(),
                "bayan-fuzz"
            )
            .is_err()
        );
    }

    #[test]
    fn reads_the_frames_std_writes() {
        let text = format!(
            "Backtrace [{{ fn: \"loro_internal::oplog::OpLog::import\", file: \"{REGISTRY}/loro-internal-1.16.2/src/oplog.rs\", line: 412 }}, {{ fn: <unknown> }}, {{ fn: \"main\" }}, {{ fn: \"<T as core::convert::Into<U>>::into\", file: \"./src/lib.rs\", line: 3 }}]"
        );
        assert_eq!(
            frames(&text).unwrap(),
            [
                Frame {
                    function: Some("loro_internal::oplog::OpLog::import".to_owned()),
                    file: Some(format!("{REGISTRY}/loro-internal-1.16.2/src/oplog.rs")),
                    line: Some(412),
                },
                Frame {
                    function: None,
                    file: None,
                    line: None
                },
                Frame {
                    function: Some("main".to_owned()),
                    file: None,
                    line: None
                },
                Frame {
                    function: Some("<T as core::convert::Into<U>>::into".to_owned()),
                    file: Some("./src/lib.rs".to_owned()),
                    line: Some(3)
                },
            ]
        );
        assert_eq!(frames("Backtrace []").unwrap(), []);
        for unreadable in [
            "<disabled>",
            "<unsupported>",
            "Backtrace [{ fn: x }]",
            "Backtrace [{ fn: \"x\", line: y }]",
            "Backtrace [{ fn: \"x\" }",
        ] {
            assert!(frames(unreadable).is_err(), "{unreadable}");
        }
    }

    #[test]
    fn a_file_belongs_to_the_package_whose_folder_holds_it() {
        let lockfile = lockfile();
        let std_library = with_rust_src();
        assert_eq!(
            source(
                &format!("{STD}/core/src/option.rs"),
                &lockfile,
                &std_library
            ),
            Source::Std
        );
        assert_eq!(
            source(
                &format!("{REGISTRY}/generic-btree-0.10.7/src/lib.rs"),
                &lockfile,
                &std_library
            ),
            Source::Package(Package {
                name: "generic-btree".to_owned(),
                version: "0.10.7".to_owned()
            })
        );
        // A version that the lockfile does not have, a crate's own folder and a toolchain path that is not std's are someone else's.
        for other in [
            format!("{REGISTRY}/generic-btree-0.10.8/src/lib.rs"),
            "./crates/bayan-crdt/src/doc.rs".to_owned(),
            "/home/user/bayan-core/crates/bayan-crdt/src/doc.rs".to_owned(),
            "/rustc/0123/library/core/src/option.rs".to_owned(),
            "rustc/0123456789abcdef0123456789abcdef01234567/library/core/src/option.rs".to_owned(),
        ] {
            assert_eq!(
                source(&other, &lockfile, &std_library),
                Source::Other(other.clone()),
                "{other}"
            );
        }
    }

    #[test]
    fn the_standard_library_s_files_are_in_one_of_two_folders() {
        let lockfile = lockfile();
        // The code that the toolchain compiled in advance, in every program, also where the standard library names a file with `..` that stays in its folder.
        for file in [
            format!("{STD}/core/src/option.rs"),
            format!("{STD}/std/src/../../backtrace/src/backtrace/libunwind.rs"),
            format!("{STD}/core/src/../../stdarch/crates/core_arch/src/x86/sse2.rs"),
        ] {
            for std_library in [without_rust_src(), with_rust_src()] {
                assert_eq!(
                    source(&file, &lockfile, &std_library),
                    Source::Std,
                    "{file}"
                );
            }
        }
        // The code that the compiler compiled into the program's crates, where the toolchain has its rust-src component: the standard library's only in a program whose own frame showed that folder, and someone else's in one whose frame did not.
        for file in [
            format!("{RUST_SRC}/core/src/option.rs"),
            format!("{RUST_SRC}/core/src/ops/function.rs"),
            format!("{RUST_SRC}/core/src/../../portable-simd/crates/core_simd/src/vector.rs"),
        ] {
            assert_eq!(
                source(&file, &lockfile, &with_rust_src()),
                Source::Std,
                "{file}"
            );
            assert_eq!(
                source(&file, &lockfile, &without_rust_src()),
                Source::Other(file.clone()),
                "{file}"
            );
        }
    }

    #[test]
    fn nothing_else_passes_for_the_standard_library() {
        let lockfile = lockfile();
        let std_library = with_rust_src();
        for other in [
            // A folder of the same shape elsewhere: another toolchain's, BayanDocs' crates, the fuzz targets' (named from their own folder, with or without `./`), a build script's output, and the rust-src component outside its `library` folder.
            "/root/.rustup/toolchains/nightly-2026-10-02-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            "/home/user/bayan-core/crates/bayan-crdt/lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            "./lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            "lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            "/home/user/bayan-core/fuzz/target/x86_64-unknown-linux-gnu/release/build/serde_json-0123456789abcdef/out/lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            "/root/.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/src/tools/x.rs".to_owned(),
            // A folder whose name only starts like the standard library's.
            format!("{RUST_SRC}-old/core/src/option.rs"),
            format!("{RUST_SRC}2/core/src/option.rs"),
            // Names that `..` leads out of the standard library's folders.
            format!("{RUST_SRC}/../../../../../../../../home/user/bayan-core/crates/bayan-crdt/src/import.rs"),
            format!("{RUST_SRC}/core/../../library-old/core/src/option.rs"),
            format!("{STD}/../../../home/user/bayan-core/crates/bayan-crdt/src/import.rs"),
            format!("{STD}/core/../../x.rs"),
            // The folders themselves, and names with empty parts or `.`, which no file of the standard library has.
            RUST_SRC.to_owned(),
            format!("{RUST_SRC}/"),
            format!("{RUST_SRC}/core/.."),
            format!("{RUST_SRC}//core/src/option.rs"),
            format!("{RUST_SRC}/./core/src/option.rs"),
            format!("{STD}/"),
            format!("{STD}/./core/src/option.rs"),
            // The standard library's own files as std writes them below the current folder, started from `/` or from the home folder: refused, never accepted.
            "./rustc/0123456789abcdef0123456789abcdef01234567/library/core/src/option.rs"
                .to_owned(),
            "./.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
        ] {
            assert_eq!(
                source(&other, &lockfile, &std_library),
                Source::Other(other.clone()),
                "{other}"
            );
        }
        // A package of the lockfile that holds a folder of that shape is that package.
        assert_eq!(
            source(
                &format!(
                    "{REGISTRY}/generic-btree-0.10.7/lib/rustlib/src/rust/library/core/src/option.rs"
                ),
                &lockfile,
                &std_library
            ),
            Source::Package(Package {
                name: "generic-btree".to_owned(),
                version: "0.10.7".to_owned()
            })
        );
    }

    #[test]
    fn reads_the_standard_library_s_folder_from_the_program_s_own_frames() {
        assert_eq!(
            StdLibrary::from_frames(
                &through_get_or_insert_with(STD, "./src/loro_panics.rs"),
                OWN_FILE
            ),
            Ok(without_rust_src())
        );
        assert_eq!(
            StdLibrary::from_frames(
                &through_get_or_insert_with(RUST_SRC, "./src/loro_panics.rs"),
                OWN_FILE
            ),
            Ok(with_rust_src())
        );
        // This module's file named in full, as other builds can name it.
        assert_eq!(
            StdLibrary::from_frames(
                &through_get_or_insert_with(
                    RUST_SRC,
                    "/home/user/bayan-core/fuzz/src/loro_panics.rs"
                ),
                OWN_FILE
            ),
            Ok(with_rust_src())
        );
        // The short names of cargo-fuzz's builds, whose debug information has line tables only; and without a frame for the call of `force_capture`.
        let short = [
            at(
                "call_once<fn() -> std::backtrace::Backtrace, ()>",
                &format!("{RUST_SRC}/core/src/ops/function.rs"),
            ),
            at(
                "get_or_insert_with<std::backtrace::Backtrace, fn() -> std::backtrace::Backtrace>",
                &format!("{RUST_SRC}/core/src/option.rs"),
            ),
            at("of_this_program", "./src/loro_panics.rs"),
        ];
        assert_eq!(
            StdLibrary::from_frames(&short, OWN_FILE),
            Ok(with_rust_src())
        );
        assert_eq!(
            StdLibrary::from_frames(&short[1..], OWN_FILE),
            Ok(with_rust_src())
        );
    }

    #[test]
    fn refuses_frames_that_do_not_show_where_the_standard_library_is() {
        let frames = through_get_or_insert_with(RUST_SRC, "./src/loro_panics.rs");
        let mut cases = vec![
            Vec::new(),
            // No frame in this module's file (a file whose name only ends like it, also right after a folder whose own name ends in `src`, is another file), or none of the standard library's before it.
            through_get_or_insert_with(RUST_SRC, "./src/other.rs"),
            through_get_or_insert_with(RUST_SRC, "./src/evil_loro_panics.rs"),
            through_get_or_insert_with(RUST_SRC, "./evilsrc/loro_panics.rs"),
            frames[2..].to_vec(),
        ];
        // The frame that called this module's code is not in `core/src/option.rs`, has no file, or is a package's.
        for file in [
            Some(format!("{RUST_SRC}/core/src/result.rs")),
            None,
            Some(format!("{REGISTRY}/libfuzzer-sys-0.4.13/src/lib.rs")),
        ] {
            let mut changed = frames.clone();
            changed[1].file = file;
            cases.push(changed);
        }
        // `core/src/option.rs` in a folder of another shape.
        for library in [
            "/home/user/bayan-core/crates/bayan-crdt",
            "lib/rustlib/src/rust/library",
            "./lib/rustlib/src/rust/library",
            "/root/.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/../rust/library",
            "/root/.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu//lib/rustlib/src/rust/library",
            "/root/.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library-old",
            "/rustc/0123/library",
            "/rustc/0123456789abcdef0123456789abcdef01234567/src/library",
            // As std writes the two folders below the current folder, started from `/` or from the home folder.
            "./rustc/0123456789abcdef0123456789abcdef01234567/library",
            "./.rustup/toolchains/nightly-2026-11-16-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library",
        ] {
            cases.push(through_get_or_insert_with(library, "./src/loro_panics.rs"));
        }
        // A frame before it that is not in the standard library's folders that it shows, or has no file.
        for file in [
            Some(format!("{REGISTRY}/serde_json-1.0.151/src/lib.rs")),
            Some("/root/.rustup/toolchains/nightly-2026-10-02-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/ops/function.rs".to_owned()),
            None,
        ] {
            let mut changed = frames.clone();
            changed[0].file = file;
            cases.push(changed);
        }
        for case in cases {
            assert!(
                StdLibrary::from_frames(&case, OWN_FILE).is_err(),
                "{case:?}"
            );
        }
    }

    #[test]
    fn this_program_s_standard_library_counts_as_the_standard_library() {
        // As the hook reads it when it is installed: in this test program, with or without rust-src.
        let std_library = StdLibrary::of_this_program().unwrap();
        // The standard library's code that the compiler compiled into this crate, here `Option::map`, is in the folder that the program's own frame showed.
        let backtrace = Some(()).map(|()| Backtrace::force_capture()).unwrap();
        let frames = frames(&format!("{backtrace:?}")).unwrap();
        let map = frames
            .iter()
            .find_map(|frame| {
                frame
                    .file
                    .as_deref()
                    .filter(|file| file.ends_with("/core/src/option.rs"))
            })
            .unwrap();
        assert_eq!(source(map, &lockfile(), &std_library), Source::Std, "{map}");
        assert_eq!(
            std_library.rust_src.is_some(),
            !map.starts_with("/rustc/"),
            "{map}"
        );
    }

    #[test]
    fn a_panic_raised_in_loro_is_loro_s() {
        let loro_frame = at(
            "loro::LoroDoc::import",
            &format!("{REGISTRY}/loro-1.16.2/src/lib.rs"),
        );
        assert!(
            matches!(panic_from(&[loro_frame.clone()]), Owner::Loro(package) if package.name == "loro")
        );
        // Raised by std for Loro, such as an `unwrap` or an index out of bounds: the first frame outside std decides.
        let unwrap = at(
            "core::option::unwrap_failed",
            &format!("{STD}/core/src/option.rs"),
        );
        assert!(matches!(
            panic_from(&[unwrap.clone(), loro_frame.clone()]),
            Owner::Loro(_)
        ));
        // Raised in one of Loro's own dependencies.
        let btree = at(
            "generic_btree::BTree<B>::insert",
            &format!("{REGISTRY}/generic-btree-0.10.7/src/lib.rs"),
        );
        assert!(
            matches!(panic_from(&[btree, loro_frame.clone()]), Owner::Loro(package) if package.name == "generic-btree")
        );
        // The same where the toolchain has its rust-src component: `Option::unwrap` compiled into Loro's crate, and a failed `assert_eq!` in Loro, whose `assert_failed` is generic too, are in that component's folder.
        let unwrap_in_loro = at(
            "<core::option::Option<u8>>::unwrap",
            &format!("{RUST_SRC}/core/src/option.rs"),
        );
        assert!(matches!(
            panic_from(&[unwrap.clone(), unwrap_in_loro, loro_frame.clone()]),
            Owner::Loro(_)
        ));
        let assert_failed_inner = at(
            "core::panicking::assert_failed_inner",
            &format!("{STD}/core/src/panicking.rs"),
        );
        let assert_failed = at(
            "core::panicking::assert_failed::<u8, u8>",
            &format!("{RUST_SRC}/core/src/panicking.rs"),
        );
        assert!(matches!(
            panic_from(&[assert_failed_inner, assert_failed, loro_frame.clone()]),
            Owner::Loro(_)
        ));
        // The adapter that called Loro is further out, and does not count, nor does std::panicking further out, where the adapter and libfuzzer-sys catch panics.
        let adapter = at(
            "bayan_crdt::doc::Doc::import_unguarded",
            "/home/user/bayan-core/crates/bayan-crdt/src/doc.rs",
        );
        let catching = at(
            "std::panicking::catch_unwind::do_call",
            &format!("{STD}/std/src/panicking.rs"),
        );
        let harness = at(
            "libfuzzer_sys::test_input_wrap",
            &format!("{REGISTRY}/libfuzzer-sys-0.4.13/src/lib.rs"),
        );
        assert!(matches!(
            panic_from(&[unwrap, loro_frame, adapter, catching, harness]),
            Owner::Loro(_)
        ));
    }

    #[test]
    fn a_panic_raised_anywhere_else_is_not_loro_s() {
        let loro_frame = at(
            "loro::LoroDoc::import",
            &format!("{REGISTRY}/loro-1.16.2/src/lib.rs"),
        );
        // BayanDocs code, also when Loro called it: the first frame outside std is not Loro's.
        let adapter = at(
            "bayan_crdt::import::check_header",
            "/home/user/bayan-core/crates/bayan-crdt/src/import.rs",
        );
        assert!(matches!(
            panic_from(&[adapter.clone(), loro_frame.clone()]),
            Owner::NotLoro(_)
        ));
        // A package that BayanDocs uses too, even when Loro called it.
        let shared = at(
            "serde_json::de::from_slice",
            &format!("{REGISTRY}/serde_json-1.0.151/src/de.rs"),
        );
        assert!(
            matches!(panic_from(&[shared, loro_frame.clone()]), Owner::NotLoro(reason) if reason.contains("serde_json 1.0.151"))
        );
        // A frame without a file where the decision falls, or none outside std.
        let no_file = Frame {
            function: Some("loro::LoroDoc::import".to_owned()),
            file: None,
            line: None,
        };
        assert!(matches!(
            panic_from(&[no_file, loro_frame.clone()]),
            Owner::NotLoro(_)
        ));
        assert!(matches!(panic_from(&[]), Owner::NotLoro(_)));
        // The standard library's code in the rust-src component's folder, in a program whose own frame did not show that folder: someone else's.
        let unwrap_in_loro = at(
            "<core::option::Option<u8>>::unwrap",
            &format!("{RUST_SRC}/core/src/option.rs"),
        );
        assert!(
            matches!(panic_in(&without_rust_src(), &[unwrap_in_loro, loro_frame.clone()]), Owner::NotLoro(reason) if reason.contains(RUST_SRC))
        );
        // Other code in a folder of that shape, or named with `..` that leads out of it: it raised the panic, and Loro's frame further out does not count.
        for file in [
            "/home/user/bayan-core/crates/bayan-crdt/lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            "/root/.rustup/toolchains/nightly-2026-10-02-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/option.rs".to_owned(),
            format!("{RUST_SRC}/../../../../../../../../home/user/bayan-core/crates/bayan-crdt/src/import.rs"),
        ] {
            let elsewhere = at("bayan_crdt::import::check_header", &file);
            assert!(
                matches!(panic_from(&[elsewhere, loro_frame.clone()]), Owner::NotLoro(reason) if reason == format!("raised in {file}")),
                "{file}"
            );
        }
        // A backtrace without the panicking frames: Loro's frame alone does not make it Loro's.
        assert!(matches!(
            owner(&[loro_frame], &lockfile(), &loro(), &with_rust_src()),
            Owner::NotLoro(_)
        ));
        // The hook's own frames, before the panicking ones, never count, whoever they are.
        let mut hook_in_loro = panicking();
        hook_in_loro[0] = at(
            "loro::something",
            &format!("{REGISTRY}/loro-1.16.2/src/lib.rs"),
        );
        hook_in_loro.push(adapter);
        assert!(matches!(
            owner(&hook_in_loro, &lockfile(), &loro(), &with_rust_src()),
            Owner::NotLoro(_)
        ));
    }
}
