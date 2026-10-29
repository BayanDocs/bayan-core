//! The compiler's own record of the files it read, compared with the files that the source check read.
//!
//! The source check (`sources.rs`) reads every `.rs` file below the members' folders before anything is built, and rejects every way it knows to make the compiler read another file: `include!`, `#[path = "…"]`, and the attributes that macros assemble. Its rules for assembled attributes are a model of how the compiler reads attributes, and a model can miss a form. A macro that keeps the `#` of an attribute written in full where it is called, drops that attribute's bracket and writes the `#` before a bracket group handed in separately, called as `module_at!(#[doc = "…"], [path = "…"])`, builds a `#[path]` that no rule reads (bayan-core#7).
//!
//! The compiler keeps a record that settles the question for every such form. For each unit it compiles, rustc writes a dependency file (dep-info, `.d`) that lists every file it read: the crate's source files, the files read with `include_str!` and `include_bytes!`, and the files that Clippy reads for its configuration. After each Clippy run, the Clippy step reads the dependency file of every unit of a workspace member and fails if it lists a file that the source check did not read, unless that file is
//!
//! - one of Clippy's own two, named exactly as Clippy names them: `Cargo.toml`, relative to the compiler's working folder, which is the workspace root for members, and the root `clippy.toml`, by the absolute path that `std::fs::canonicalize` gives it. The same files named in any other way, such as `crates/bayan-units/src/../../../Cargo.toml`, count like any other file, because a file can be valid TOML and valid Rust at once. An absolute `#[path]` can name the root `clippy.toml` exactly as Clippy does, though, so a `clippy.toml` made valid Rust could be compiled as code and pass on the one machine whose path it names. It cannot pass CI, whose three checkouts have different roots, and `clippy.toml` is a reviewed policy file: like the data files, it is trusted by review;
//! - or a data file that `DATA_FILES` in `verify.rs` lists for the unit's member, with the reason. A dependency file does not say whether a file was read as data or compiled as code, so reviewers check that every listed file is data.
//!
//! Files are compared after `std::fs::canonicalize`, so that `..`, links in parent folders and the `\\?\` prefix of Windows compare equal. The source check stays the first and faster line, with the clearer messages; this record backs it wherever a form that it misses makes the compiler read a file. `DATA_FILES` cannot go stale: an entry fails when a member it names read the file in none of the runs.
//!
//! # Units and their dependency files
//!
//! The units come from Cargo, not from the build folder, which keeps the dependency files of earlier builds and of the gate's other steps. Clippy runs with `--message-format=json-render-diagnostics`: Cargo still prints the diagnostics for people on standard error, and writes one JSON message per line on standard output. A `compiler-artifact` message names each unit of the run, fresh ones included, with its target and its `.rmeta` file, and rustc writes the dependency file next to that, under the same name without the `lib` in front and with the extension `.d` (`deps/libbayan_units-<hash>.rmeta` and `deps/bayan_units-<hash>.d`). A unit that Cargo reused (`"fresh": true`) keeps the dependency file of the build that compiled it; one that Cargo compiled in the run (`"fresh": false`) must have a dependency file written since the run began, so that a file left over from an earlier build, by whatever means, can never stand in for the record of this one. The run's start is rounded down to the second, so that a file system that stores whole seconds cannot make a file written in the run's first second look older.
//!
//! The unit that Cargo builds to run a build script or a procedural macro of a workspace member, an executable or a shared library, has no `.rmeta` file, so it would make the step fail ("with 0 `.rmeta` files instead of one"). `BUILD_SCRIPTS` and `PROC_MACROS` in `verify.rs` are empty today; whoever lists a member in either must first teach [`units`] where that unit's dependency file is.
//!
//! # Failing loudly
//!
//! Whatever the check cannot follow is an error, never a pass: a run that reports no unit for a member it checks, a unit without a dependency file, a unit that Cargo compiled in the run whose dependency file was not written during the run, a line of one that it cannot read, a listed file that does not exist, and a unit whose list does not contain its own target's root file, which shows that the list was read.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::json::{self, Value};
use crate::policy::Package;

/// Files that workspace members may read as data, with `include_str!` or `include_bytes!`, although the source check does not read them: one entry of `DATA_FILES` in `verify.rs`.
#[derive(Debug, Clone, Copy)]
pub struct DataFiles {
    /// The files, relative to the workspace root, with `/` between folders.
    pub files: &'static [&'static str],
    /// The workspace members that read them.
    pub members: &'static [&'static str],
    /// Why they read them.
    pub reason: &'static str,
}

/// The name under which Clippy lists the manifest it reads for every unit: relative to the compiler's working folder, which is the workspace root for members.
const CLIPPY_MANIFEST: &str = "Cargo.toml";

/// Clippy's configuration file, at the workspace root, which Clippy lists by its absolute path.
const CLIPPY_CONFIG: &str = "clippy.toml";

/// How the files that the source check never read are reported: the heading of their list, and what to do.
const UNREAD: (&str, &str) = (
    "the compiler read code that the source check never checked",
    "The source check (xtask/src/sources.rs) reads every `.rs` file below the members' folders before anything is built, and Clippy checks only the code that the compiler reads, so the compiler may read nothing else, apart from Clippy's own configuration and the data files that DATA_FILES lists. For the units above it read files that the source check never read, so code in them could hide from Clippy, for example behind `#[cfg(not(clippy))]`: a macro can make the compiler read such a file by assembling an attribute such as `#[path = \"…\"]` in a form that the source check does not recognize. Keep every Rust file of a crate inside the crate's folder, and declare its modules with a plain `mod name;`. If a file is data that a member reads with `include_str!` or `include_bytes!`, list it in DATA_FILES in xtask/src/verify.rs, with the members that read it and the reason, in a reviewed pull request; the compiler's dependency files do not say whether a file was read as data or compiled as code, so reviewers check that every listed file is data.",
);

/// How the entries of `DATA_FILES` that no named member read are reported: the heading of their list, and what to do.
const STALE: (&str, &str) = (
    "DATA_FILES lists files that a member it names read in none of the Clippy runs",
    "Each entry of DATA_FILES in xtask/src/verify.rs names files that the source check does not read and the members that may read them, and the compiler's record shows that the members above did not read the files above. Remove each file from its entry, or the member, so that the list names only what the members read and cannot go stale.",
);

/// A unit that Cargo compiled for a workspace member in a Clippy run: one target, compiled once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The member.
    pub package: String,
    /// Its target, for a person, such as "lib target `bayan_units`, as unit tests".
    pub target: String,
    /// The file the compiler starts from (`src_path` in `cargo metadata`).
    pub root_file: PathBuf,
    /// The dependency file that the compiler wrote for the unit.
    pub dep_info: PathBuf,
    /// Whether Cargo reused the unit from an earlier build (`"fresh": true`) instead of compiling it in this run, so that its dependency file dates from that build.
    pub fresh: bool,
}

/// The units of the workspace members `members` (from `cargo metadata`) in `messages`, what Cargo printed on standard output for a run with `--message-format=json-render-diagnostics`. Units of other packages, such as dependencies from crates.io, are left out. Every line must be a JSON message, and each member named in `required`, the members that the run checks, must have a unit.
pub fn units(messages: &str, members: &[Package], required: &[&str]) -> Result<Vec<Unit>, String> {
    let mut units = Vec::new();
    for line in messages.lines() {
        let message = json::parse(line).map_err(|error| {
            format!("Cargo printed a line that is not one of its JSON messages ({error}): {line}")
        })?;
        let reason = message
            .get("reason")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Cargo printed a message without a `reason`: {line}"))?;
        if reason != "compiler-artifact" {
            continue;
        }
        let manifest = message
            .get("manifest_path")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Cargo reported a unit without a `manifest_path`: {line}"))?;
        let folder = Path::new(manifest).parent();
        if let Some(member) = members
            .iter()
            .find(|member| Some(member.folder.as_path()) == folder)
        {
            units.push(unit(&message, member)?);
        }
    }
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|member| !units.iter().any(|unit| unit.package == *member))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "Cargo reported no unit of {}, so the gate cannot tell which files the compiler read for it",
            missing.join(", ")
        ));
    }
    Ok(units)
}

/// The unit of the member `package` that a `compiler-artifact` message describes.
fn unit(message: &Value, package: &Package) -> Result<Unit, String> {
    let invalid = |what: &str| format!("Cargo reported a unit of {} {what}", package.name);
    let target = message
        .get("target")
        .ok_or_else(|| invalid("without a target"))?;
    let name = target
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("whose target has no name"))?;
    let kinds = strings(target.get("kind"))
        .ok_or_else(|| invalid(&format!("whose target `{name}` has no kind")))?;
    let root_file = package
        .targets
        .iter()
        .find(|known| known.name == name && known.kinds == kinds)
        .map(|known| known.root_file.clone())
        .ok_or_else(|| {
            invalid(&format!(
                "for a target `{name}` that `cargo metadata` does not list"
            ))
        })?;
    // `--all-targets` also compiles each library and binary as its own unit tests; tests and benchmarks always are.
    let as_tests = message
        .get("profile")
        .and_then(|profile| profile.get("test"))
        == Some(&Value::Bool(true))
        && !kinds.iter().any(|kind| kind == "test" || kind == "bench");
    let described = format!(
        "{} target `{name}`{}",
        kinds.join(", "),
        if as_tests { ", as unit tests" } else { "" }
    );
    let files = strings(message.get("filenames"))
        .ok_or_else(|| invalid(&format!("({described}) without its files")))?;
    let rmeta: Vec<&String> = files
        .iter()
        .filter(|file| file.ends_with(".rmeta"))
        .collect();
    let [rmeta] = rmeta.as_slice() else {
        return Err(invalid(&format!(
            "({described}) with {} `.rmeta` files instead of one",
            rmeta.len()
        )));
    };
    let dep_info = dep_info_next_to(Path::new(rmeta)).ok_or_else(|| {
        invalid(&format!(
            "({described}) whose `.rmeta` file, {rmeta}, is not named lib<name>.rmeta"
        ))
    })?;
    let fresh = match message.get("fresh") {
        Some(Value::Bool(fresh)) => *fresh,
        _ => {
            return Err(invalid(&format!(
                "({described}) without saying whether it is fresh"
            )));
        }
    };
    Ok(Unit {
        package: package.name.clone(),
        target: described,
        root_file,
        dep_info,
        fresh,
    })
}

/// The strings of `value`, if it is an array of strings.
fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

/// The dependency file that rustc writes next to the `.rmeta` file `rmeta`: the same name without `lib` in front, with the extension `.d`.
fn dep_info_next_to(rmeta: &Path) -> Option<PathBuf> {
    let name = rmeta.file_name()?.to_str()?;
    let stem = name.strip_prefix("lib")?.strip_suffix(".rmeta")?;
    Some(rmeta.with_file_name(format!("{stem}.d")))
}

/// The files that a dependency file lists, in order, as rustc names them: relative to the compiler's working folder or absolute, and not normalized (`crates/bayan-units/src/../../../shared/sine.rs`).
///
/// rustc writes Makefile rules: first one rule per output, `<output>: <file> <file> …`, each followed by an empty line; then one rule `<file>:` per file; then, after an empty line, comments that start with `#`, such as `# env-dep:CLIPPY_CONF_DIR` for an environment variable it read. In a file's name it writes a space as `\ ` and changes nothing else (on Windows a path has a `:` after the drive letter), so a file's rule never contains `: `, and an output's rule always does; it writes the outputs' names as they are. The files are therefore read from their own rules, which hold one name each, and every output's rule must list exactly those files, in the same order, so that a misreading cannot pass. Any other line is an error, and so is a dependency file without files or outputs: never an empty list.
pub fn listed_files(text: &str) -> Result<Vec<String>, String> {
    let mut outputs = Vec::new();
    let mut files = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.contains(": ") {
            outputs.push((number, line));
        } else if let Some(name) = line.strip_suffix(':').filter(|name| !name.is_empty()) {
            let bare_space = name
                .char_indices()
                .any(|(at, character)| character == ' ' && !name[..at].ends_with('\\'));
            if bare_space {
                return Err(format!(
                    "line {number} names a file with a space that `\\` does not escape: {line}"
                ));
            }
            files.push(name);
        } else {
            return Err(format!(
                "line {number} is neither a rule nor a comment: {line}"
            ));
        }
    }
    if files.is_empty() {
        return Err("it lists no files".to_owned());
    }
    if outputs.is_empty() {
        return Err("it has no rule for an output".to_owned());
    }
    let list = format!(": {}", files.join(" "));
    for (number, line) in outputs {
        if !line
            .strip_suffix(&list)
            .is_some_and(|output| !output.is_empty())
        {
            return Err(format!(
                "line {number}, the rule of an output, does not list exactly the files of the rules after it"
            ));
        }
    }
    Ok(files.iter().map(|name| name.replace("\\ ", " ")).collect())
}

/// A data file of `DATA_FILES`.
#[derive(Debug)]
struct Data {
    /// The file, as the file system names it.
    file: PathBuf,
    /// The file as the list writes it, relative to the root.
    written: &'static str,
    /// The members that may read it.
    members: &'static [&'static str],
    /// Why they read it.
    reason: &'static str,
}

/// The comparison of the files that the compiler read for each unit with the files it may read, over the Clippy runs. It records which data files each member read, so that the entries of `DATA_FILES` that no named member read can be reported once all runs are done.
#[derive(Debug)]
pub struct Comparison {
    /// The workspace root, as the file system names it. Names in a dependency file that are not absolute are relative to it, the compiler's working folder for members.
    root: PathBuf,
    /// Every file that the source check read, as the file system names it.
    sources: BTreeSet<PathBuf>,
    /// The absolute name under which Clippy lists the root clippy.toml: the one `std::fs::canonicalize` gives it, as Clippy finds the file.
    clippy_config: OsString,
    /// The listed data files.
    data: Vec<Data>,
    /// The data files (as positions in `data`) that each member read in the runs so far.
    read: BTreeSet<(usize, String)>,
}

impl Comparison {
    /// Prepares the comparison for the workspace at `root`, whose members are named `members`. `sources` are the files that the source check read, and `data` the entries of `DATA_FILES`. Fails if an entry names no member, a member that does not exist, a file that does not exist or that the source check reads anyway, or a file that another entry lists too.
    pub fn new(
        root: &Path,
        sources: &BTreeSet<PathBuf>,
        data: &[DataFiles],
        members: &[&str],
    ) -> Result<Self, String> {
        let root = canonical(root)?;
        let sources = sources
            .iter()
            .map(|file| canonical(file))
            .collect::<Result<BTreeSet<_>, _>>()?;
        let clippy_config = canonical(&root.join(CLIPPY_CONFIG))?.into_os_string();
        let mut problems = Vec::new();
        let mut listed: Vec<Data> = Vec::new();
        for entry in data {
            if entry.members.is_empty() {
                problems.push(format!(
                    "  the entry for {} names no member",
                    entry.files.join(", ")
                ));
            }
            for member in entry.members {
                if !members.contains(member) {
                    problems.push(format!("  `{member}` is not a workspace member"));
                }
            }
            for &written in entry.files {
                let Some(path) = file_at(&root, written) else {
                    problems.push(format!(
                        "  {written} is not a path relative to the root, with `/` between folders and without `.` or `..`"
                    ));
                    continue;
                };
                let file = match std::fs::canonicalize(&path) {
                    Ok(file) if file.is_file() => file,
                    _ => {
                        problems.push(format!("  {written} is not a file"));
                        continue;
                    }
                };
                if sources.contains(&file) {
                    problems.push(format!(
                        "  {written} is read by the source check anyway; remove it"
                    ));
                } else if listed.iter().any(|other| other.file == file) {
                    problems.push(format!("  {written} is listed twice"));
                } else {
                    listed.push(Data {
                        file,
                        written,
                        members: entry.members,
                        reason: entry.reason,
                    });
                }
            }
        }
        if !problems.is_empty() {
            return Err(format!(
                "DATA_FILES in xtask/src/verify.rs is not right:\n{}\nEach entry names files below the root, relative to it, that the source check does not read, the workspace members that read them with `include_str!` or `include_bytes!`, and the reason.",
                problems.join("\n")
            ));
        }
        Ok(Self {
            root,
            sources,
            clippy_config,
            data: listed,
            read: BTreeSet::new(),
        })
    }

    /// Checks the dependency file of every unit of one Clippy run, which began at `run_started`: the dependency file of a unit that Cargo compiled in the run must have been written since then, each file it lists must be one that the source check read, one of Clippy's own two, or a data file listed for the unit's member, and the list must contain the unit's root file. Records the data files that each member read. Returns one line describing what was checked, or the problem: every unit that read another file, with each such file relative to the root, or the first thing it could not follow.
    pub fn check(&mut self, units: &[Unit], run_started: SystemTime) -> Result<String, String> {
        // Rounded down to the second, so that a file system that stores whole seconds cannot make a file written in the run's first second look older than the run.
        let run_started = whole_second(run_started);
        let mut unread = Vec::new();
        let mut data_read = BTreeSet::new();
        for unit in units {
            let unit_name = format!("{}, {}", unit.package, unit.target);
            let dep_info = self.shown(&unit.dep_info);
            let text = std::fs::read_to_string(&unit.dep_info).map_err(|error| {
                format!(
                    "{unit_name}: cannot read the dependency file that the compiler wrote for it, {dep_info} ({error}), so the gate cannot tell which files the compiler read"
                )
            })?;
            if !unit.fresh {
                let written = std::fs::metadata(&unit.dep_info)
                    .and_then(|metadata| metadata.modified())
                    .map_err(|error| {
                        format!(
                            "{unit_name}: cannot tell when its dependency file, {dep_info}, was written ({error})"
                        )
                    })?;
                if written < run_started {
                    return Err(format!(
                        "{unit_name}: Cargo compiled it in this run, but its dependency file, {dep_info}, was last written before the run began, so it is not the record of this compilation and the gate cannot tell which files the compiler read"
                    ));
                }
            }
            let files = listed_files(&text).map_err(|problem| {
                format!("{unit_name}: cannot read its dependency file, {dep_info}: {problem}")
            })?;
            let root_file =
                canonical(&unit.root_file).map_err(|problem| format!("{unit_name}: {problem}"))?;
            let mut lists_root_file = false;
            for name in &files {
                if self.is_clippys(name) {
                    continue;
                }
                let file = std::fs::canonicalize(self.root.join(name)).map_err(|error| {
                    format!(
                        "{unit_name}: its dependency file, {dep_info}, lists {name}, which the gate cannot find ({error})"
                    )
                })?;
                lists_root_file |= file == root_file;
                if self.sources.contains(&file) {
                    continue;
                }
                match self.data.iter().position(|data| data.file == file) {
                    Some(index) if self.data[index].members.contains(&unit.package.as_str()) => {
                        data_read.insert(index);
                        self.read.insert((index, unit.package.clone()));
                    }
                    Some(index) => unread.push(format!(
                        "  {unit_name}: {} (DATA_FILES lists it for {} only)",
                        self.data[index].written,
                        self.data[index].members.join(", ")
                    )),
                    None => unread.push(format!("  {unit_name}: {}", self.shown(&file))),
                }
            }
            if !lists_root_file {
                return Err(format!(
                    "{unit_name}: its dependency file, {dep_info}, does not list the target's root file, {}, so the gate cannot be sure that it read the list",
                    self.shown(&root_file)
                ));
            }
        }
        if !unread.is_empty() {
            return Err(format!(
                "{}:\n{}\n{}",
                UNREAD.0,
                unread.join("\n"),
                UNREAD.1
            ));
        }
        Ok(format!(
            "the {} units of workspace members: the compiler read only files that the source check read, besides Clippy's clippy.toml and Cargo.toml and {} data files that DATA_FILES lists for their members",
            units.len(),
            data_read.len()
        ))
    }

    /// Checks, once all runs are done, that every member that an entry of `DATA_FILES` names read each of the entry's files in some run. Returns one line describing what was checked, or every file and member that did not.
    pub fn check_stale(&self) -> Result<String, String> {
        let mut stale = Vec::new();
        for (index, data) in self.data.iter().enumerate() {
            for member in data.members {
                if !self.read.contains(&(index, (*member).to_owned())) {
                    stale.push(format!(
                        "  {}, by {member} (listed as {})",
                        data.written, data.reason
                    ));
                }
            }
        }
        if !stale.is_empty() {
            return Err(format!("{}:\n{}\n{}", STALE.0, stale.join("\n"), STALE.1));
        }
        Ok(if self.data.is_empty() {
            "DATA_FILES is empty, and no member read a data file outside it".to_owned()
        } else {
            format!(
                "DATA_FILES: each of its {} files was read by every member that its entry names",
                self.data.len()
            )
        })
    }

    /// Whether `name`, as a dependency file lists it, is one of Clippy's own two files, named exactly as Clippy names them.
    fn is_clippys(&self, name: &str) -> bool {
        name == CLIPPY_MANIFEST || self.clippy_config == *name
    }

    /// `file` relative to the root if it lies inside, as messages show it. It is named as the file system names it first, if it exists, because the root is.
    fn shown(&self, file: &Path) -> String {
        let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        file.strip_prefix(&self.root)
            .unwrap_or(&file)
            .display()
            .to_string()
    }
}

/// `time` rounded down to a whole second.
fn whole_second(time: SystemTime) -> SystemTime {
    time.duration_since(UNIX_EPOCH).map_or(time, |since| {
        UNIX_EPOCH + Duration::from_secs(since.as_secs())
    })
}

/// `path`, as the file system names it.
fn canonical(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| format!("cannot find {}: {error}", path.display()))
}

/// The file at `written`, a path relative to `root` with `/` between folders, or `None` if `written` is not such a path (empty, absolute, with `.` or `..`, or with `\` or `:`).
fn file_at(root: &Path, written: &str) -> Option<PathBuf> {
    let mut path = root.to_path_buf();
    for part in written.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains(['\\', ':']) {
            return None;
        }
        path.push(part);
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Target;

    /// The start of a run that began before every file these tests write, for the tests that are not about when a dependency file was written.
    const LONG_AGO: SystemTime = UNIX_EPOCH;

    /// A dependency file as rustc 1.99 writes it for a unit that Clippy checks, on Linux: the rules of the `.d` and `.rmeta` files, one rule per file read, and the environment variables that Clippy read.
    const LINUX: &str = "/home/ana/bayan-core/target/debug/deps/bayan_units-0eb18dafa0a0f800.d: /home/ana/bayan-core/clippy.toml crates/bayan-units/src/lib.rs crates/bayan-units/src/angle.rs crates/bayan-units/src/../../../shared/sine.rs Cargo.toml

/home/ana/bayan-core/target/debug/deps/libbayan_units-0eb18dafa0a0f800.rmeta: /home/ana/bayan-core/clippy.toml crates/bayan-units/src/lib.rs crates/bayan-units/src/angle.rs crates/bayan-units/src/../../../shared/sine.rs Cargo.toml

/home/ana/bayan-core/clippy.toml:
crates/bayan-units/src/lib.rs:
crates/bayan-units/src/angle.rs:
crates/bayan-units/src/../../../shared/sine.rs:
Cargo.toml:

# env-dep:CLIPPY_ARGS=-D__CLIPPY_HACKERY__warnings__CLIPPY_HACKERY__
# env-dep:CLIPPY_CONF_DIR
";

    #[test]
    fn reads_the_files_that_a_dependency_file_lists() {
        assert_eq!(
            listed_files(LINUX).unwrap(),
            [
                "/home/ana/bayan-core/clippy.toml",
                "crates/bayan-units/src/lib.rs",
                "crates/bayan-units/src/angle.rs",
                // As the compiler reached it, not normalized.
                "crates/bayan-units/src/../../../shared/sine.rs",
                "Cargo.toml",
            ]
        );
        // A dependency file with only one output, and without comments, is read the same way.
        let rmeta_only = "/r/liba-1.rmeta: crates/a/src/lib.rs\n\ncrates/a/src/lib.rs:\n";
        assert_eq!(listed_files(rmeta_only).unwrap(), ["crates/a/src/lib.rs"]);
    }

    #[test]
    fn reads_windows_paths() {
        // Windows paths have a `:` after the drive letter, and Clippy names its configuration by the path that `std::fs::canonicalize` gives, with the `\\?\` prefix.
        let text = r"C:\Users\Ana\bayan-core\target\debug\deps\bayan_units-0eb18dafa0a0f800.d: \\?\C:\Users\Ana\bayan-core\clippy.toml crates\bayan-units\src\lib.rs crates\bayan-units\src\../../../shared/sine.rs Cargo.toml

C:\Users\Ana\bayan-core\target\debug\deps\libbayan_units-0eb18dafa0a0f800.rmeta: \\?\C:\Users\Ana\bayan-core\clippy.toml crates\bayan-units\src\lib.rs crates\bayan-units\src\../../../shared/sine.rs Cargo.toml

\\?\C:\Users\Ana\bayan-core\clippy.toml:
crates\bayan-units\src\lib.rs:
crates\bayan-units\src\../../../shared/sine.rs:
Cargo.toml:

# env-dep:CLIPPY_CONF_DIR
";
        assert_eq!(
            listed_files(text).unwrap(),
            [
                r"\\?\C:\Users\Ana\bayan-core\clippy.toml",
                r"crates\bayan-units\src\lib.rs",
                r"crates\bayan-units\src\../../../shared/sine.rs",
                "Cargo.toml",
            ]
        );
    }

    #[test]
    fn unescapes_spaces_in_the_files_but_not_in_the_outputs() {
        // rustc writes a space in a file's name as `\ ` and changes nothing else, also not a `\` before a space (`back\ slash.rs` becomes `back\\ slash.rs`); it writes the outputs' names as they are.
        let files = [
            r"/home/ana/my\ repo/clippy.toml",
            r"crates/my\ lib/src/lib.rs",
            r"crates/my\ lib/src/back\\ slash.rs",
            "Cargo.toml",
        ];
        let list = files.join(" ");
        let text = format!(
            "/home/ana/my repo/target/debug/deps/my_lib-1.d: {list}\n\n/home/ana/my repo/target/debug/deps/libmy_lib-1.rmeta: {list}\n\n{}:\n",
            files.join(":\n")
        );
        assert_eq!(
            listed_files(&text).unwrap(),
            [
                "/home/ana/my repo/clippy.toml",
                "crates/my lib/src/lib.rs",
                r"crates/my lib/src/back\ slash.rs",
                "Cargo.toml",
            ]
        );
    }

    #[test]
    fn refuses_what_it_cannot_read() {
        let output = "/r/target/debug/deps/liba-1.rmeta: crates/a/src/lib.rs Cargo.toml\n\n";
        let files = "crates/a/src/lib.rs:\nCargo.toml:\n";
        for (text, problem) in [
            (String::new(), "it lists no files"),
            (
                "\n# env-dep:CLIPPY_CONF_DIR\n".to_owned(),
                "it lists no files",
            ),
            (output.to_owned(), "it lists no files"),
            (files.to_owned(), "it has no rule for an output"),
            // The rule of an output that lists another file, the files in another order, no file, or an output without a name.
            (
                format!("{output}crates/a/src/lib.rs:\n"),
                "line 1, the rule of an output, does not list exactly the files",
            ),
            (
                format!("{output}Cargo.toml:\ncrates/a/src/lib.rs:\n"),
                "does not list exactly the files",
            ),
            (
                format!("/r/liba-1.rmeta: \n{files}"),
                "does not list exactly the files",
            ),
            (
                format!(": crates/a/src/lib.rs Cargo.toml\n{files}"),
                "does not list exactly the files",
            ),
            // Two outputs whose rules differ: a dependency file cut short, or written by two compilations.
            (
                format!("{output}/r/a-1.d: crates/a/src/lib.rs\n{files}"),
                "line 3, the rule of an output, does not list exactly the files",
            ),
            // Lines that are neither rules nor comments.
            (
                format!("{output}{files}crates/a/src/b.rs\n"),
                "line 5 is neither a rule nor a comment",
            ),
            (
                format!("{output}{files} \n"),
                "is neither a rule nor a comment",
            ),
            (
                format!("{output}{files}:\n"),
                "is neither a rule nor a comment",
            ),
            // A space that `\` does not escape.
            (
                "/r/a.rmeta: crates/my lib.rs\n\ncrates/my lib.rs:\n".to_owned(),
                "line 3 names a file with a space that `\\` does not escape",
            ),
        ] {
            let error = listed_files(&text).expect_err(&text);
            assert!(error.contains(problem), "{text:?}: {error}");
        }
    }

    /// A workspace member at `/repo/crates/<name>` whose targets are `(kind, name, root file relative to its folder)`.
    fn package(name: &str, targets: &[(&str, &str, &str)]) -> Package {
        let folder = Path::new("/repo").join("crates").join(name);
        Package {
            name: name.to_owned(),
            targets: targets
                .iter()
                .map(|(kind, target, file)| Target {
                    kinds: vec![(*kind).to_owned()],
                    name: (*target).to_owned(),
                    root_file: folder.join(file),
                })
                .collect(),
            folder,
        }
    }

    /// A `compiler-artifact` message as Cargo 1.99 prints it with `--message-format=json-render-diagnostics`, for a unit of the package whose manifest is `manifest`.
    fn artifact(manifest: &str, kind: &str, name: &str, test: bool, file: &str) -> String {
        format!(
            r#"{{"reason":"compiler-artifact","package_id":"path+file:///repo#0.0.0","manifest_path":"{manifest}","target":{{"kind":["{kind}"],"crate_types":["lib"],"name":"{name}","src_path":"/repo/src/lib.rs","edition":"2024","doc":true,"doctest":true,"test":true}},"profile":{{"opt_level":"0","debuginfo":2,"debug_assertions":true,"overflow_checks":true,"test":{test}}},"features":[],"filenames":["{file}"],"executable":null,"fresh":true}}"#
        )
    }

    #[test]
    fn takes_the_units_of_the_members_from_cargo() {
        let units_package = package(
            "bayan-units",
            &[
                ("lib", "bayan_units", "src/lib.rs"),
                ("test", "determinism", "tests/determinism.rs"),
                ("example", "tour", "examples/tour.rs"),
            ],
        );
        let model = package("bayan-model", &[("lib", "bayan_model", "src/lib.rs")]);
        let members = [units_package, model];
        let manifest = "/repo/crates/bayan-units/Cargo.toml";
        let deps = "/repo/target/debug/deps";
        let messages = [
            // A dependency from crates.io, which is not checked.
            artifact(
                "/home/ana/.cargo/registry/src/index.crates.io-1/serde-1.0.228/Cargo.toml",
                "lib",
                "serde",
                false,
                &format!("{deps}/libserde-9.rlib"),
            ),
            artifact(
                manifest,
                "lib",
                "bayan_units",
                false,
                &format!("{deps}/libbayan_units-1.rmeta"),
            ),
            artifact(
                manifest,
                "lib",
                "bayan_units",
                true,
                &format!("{deps}/libbayan_units-2.rmeta"),
            ),
            artifact(
                manifest,
                "test",
                "determinism",
                true,
                &format!("{deps}/libdeterminism-3.rmeta"),
            ),
            artifact(
                manifest,
                "example",
                "tour",
                false,
                "/repo/target/debug/examples/libtour-4.rmeta",
            ),
            // Compiled in this run, the others reused from an earlier build.
            artifact(
                "/repo/crates/bayan-model/Cargo.toml",
                "lib",
                "bayan_model",
                false,
                &format!("{deps}/libbayan_model-5.rmeta"),
            )
            .replace("\"fresh\":true", "\"fresh\":false"),
            r#"{"reason":"build-finished","success":true}"#.to_owned(),
        ]
        .join("\n");
        let listed = units(&messages, &members, &["bayan-units", "bayan-model"]).unwrap();
        let fresh: Vec<bool> = listed.iter().map(|unit| unit.fresh).collect();
        assert_eq!(fresh, [true, true, true, true, false]);
        let found: Vec<(&str, &str, PathBuf, PathBuf)> = listed
            .iter()
            .map(|unit| {
                (
                    unit.package.as_str(),
                    unit.target.as_str(),
                    unit.root_file.clone(),
                    unit.dep_info.clone(),
                )
            })
            .collect();
        let units_folder = Path::new("/repo/crates/bayan-units");
        assert_eq!(
            found,
            [
                (
                    "bayan-units",
                    "lib target `bayan_units`",
                    units_folder.join("src/lib.rs"),
                    PathBuf::from(format!("{deps}/bayan_units-1.d")),
                ),
                (
                    "bayan-units",
                    "lib target `bayan_units`, as unit tests",
                    units_folder.join("src/lib.rs"),
                    PathBuf::from(format!("{deps}/bayan_units-2.d")),
                ),
                (
                    "bayan-units",
                    "test target `determinism`",
                    units_folder.join("tests/determinism.rs"),
                    PathBuf::from(format!("{deps}/determinism-3.d")),
                ),
                (
                    "bayan-units",
                    "example target `tour`",
                    units_folder.join("examples/tour.rs"),
                    PathBuf::from("/repo/target/debug/examples/tour-4.d"),
                ),
                (
                    "bayan-model",
                    "lib target `bayan_model`",
                    Path::new("/repo/crates/bayan-model").join("src/lib.rs"),
                    PathBuf::from(format!("{deps}/bayan_model-5.d")),
                ),
            ]
        );
        // A member that the run checks must have a unit; one it does not check need not.
        let only_units: String = messages
            .lines()
            .filter(|line| !line.contains("bayan_model"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            units(&only_units, &members, &["bayan-units"])
                .unwrap()
                .len(),
            4
        );
        let problem = units(&only_units, &members, &["bayan-units", "bayan-model"]).unwrap_err();
        assert!(
            problem.contains("Cargo reported no unit of bayan-model"),
            "{problem}"
        );
    }

    #[test]
    fn refuses_messages_it_cannot_follow() {
        let members = [package("a", &[("lib", "a", "src/lib.rs")])];
        let manifest = "/repo/crates/a/Cargo.toml";
        for (message, problem) in [
            (
                "Checking a v0.0.0".to_owned(),
                "not one of its JSON messages",
            ),
            (r#"{"success":true}"#.to_owned(), "without a `reason`"),
            (
                r#"{"reason":"compiler-artifact"}"#.to_owned(),
                "without a `manifest_path`",
            ),
            (
                artifact(
                    manifest,
                    "lib",
                    "other",
                    false,
                    "/repo/target/debug/deps/libother-1.rmeta",
                ),
                "for a target `other` that `cargo metadata` does not list",
            ),
            (
                artifact(
                    manifest,
                    "lib",
                    "a",
                    false,
                    "/repo/target/debug/deps/liba-1.rlib",
                ),
                "with 0 `.rmeta` files instead of one",
            ),
            (
                artifact(
                    manifest,
                    "lib",
                    "a",
                    false,
                    "/repo/target/debug/deps/liba-1.rmeta",
                )
                .replace(",\"fresh\":true", ""),
                "without saying whether it is fresh",
            ),
            (
                artifact(
                    manifest,
                    "lib",
                    "a",
                    false,
                    "/repo/target/debug/deps/a-1.rmeta",
                ),
                "is not named lib<name>.rmeta",
            ),
        ] {
            let error = units(&message, &members, &["a"]).unwrap_err();
            assert!(error.contains(problem), "{message}: {error}");
        }
    }

    #[test]
    fn finds_the_dependency_file_next_to_the_rmeta_file() {
        for (rmeta, dep_info) in [
            (
                "target/debug/deps/libbayan_units-0123.rmeta",
                Some("target/debug/deps/bayan_units-0123.d"),
            ),
            (
                "target/debug/examples/libtour-4.rmeta",
                Some("target/debug/examples/tour-4.d"),
            ),
            // Only the first `lib` is Cargo's.
            ("deps/liblibrary-5.rmeta", Some("deps/library-5.d")),
            ("deps/bayan_units-0123.rmeta", None),
            ("deps/libbayan_units-0123.rlib", None),
        ] {
            assert_eq!(
                dep_info_next_to(Path::new(rmeta)),
                dep_info.map(PathBuf::from),
                "{rmeta}"
            );
        }
    }

    /// A scratch workspace for one test, removed again at the end: the root's Clippy configuration and manifest, two members `a` and `c` with their sources, a Rust file outside every member, and a data file.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bayan-xtask-dep-info-{}-{test}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            let scratch = Self(path);
            for (file, text) in [
                ("clippy.toml", "disallowed-methods = []\n"),
                ("Cargo.toml", "[workspace]\n"),
                ("crates/a/src/lib.rs", "//! A.\nmod b;\n"),
                ("crates/a/src/b.rs", "//! B.\n"),
                ("crates/c/src/lib.rs", "//! C.\n"),
                // The contents of a file are never shown.
                (
                    "shared/sine.rs",
                    "#[cfg(not(clippy))] pub fn sine() {} // never shown\n",
                ),
                ("data/table.txt", "1 2 3\n"),
            ] {
                scratch.write(file, text);
            }
            scratch
        }

        /// Writes `text` to the file at `file`, relative to the root with `/` between folders, and returns its path.
        fn write(&self, file: &str, text: &str) -> PathBuf {
            let path = self.path(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        }

        fn path(&self, file: &str) -> PathBuf {
            file.split('/')
                .fold(self.0.clone(), |path, part| path.join(part))
        }

        /// The files that the source check would read: the members' `.rs` files.
        fn sources(&self) -> BTreeSet<PathBuf> {
            [
                "crates/a/src/lib.rs",
                "crates/a/src/b.rs",
                "crates/c/src/lib.rs",
            ]
            .iter()
            .map(|file| self.path(file))
            .collect()
        }

        /// The name under which Clippy lists the root clippy.toml.
        fn clippy_config(&self) -> String {
            std::fs::canonicalize(self.path("clippy.toml"))
                .unwrap()
                .display()
                .to_string()
        }

        /// A unit of the library of the member `member`, whose dependency file, `target/debug/deps/<member>-<number>.d`, lists `files` (as rustc writes them, with `\ ` for a space), Clippy's two files after them.
        fn unit(&self, member: &str, number: u32, files: &[&str]) -> Unit {
            let clippy_config = self.clippy_config();
            let mut listed: Vec<String> = files.iter().map(|file| (*file).to_owned()).collect();
            listed.extend([clippy_config, "Cargo.toml".to_owned()]);
            let listed: Vec<String> = listed.iter().map(|file| file.replace(' ', "\\ ")).collect();
            let deps = self.0.join("target").join("debug").join("deps");
            let dep_info = deps.join(format!("{member}-{number}.d"));
            let rmeta = deps.join(format!("lib{member}-{number}.rmeta"));
            let list = listed.join(" ");
            let text = format!(
                "{}: {list}\n\n{}: {list}\n\n{}:\n\n# env-dep:CLIPPY_CONF_DIR\n",
                dep_info.display(),
                rmeta.display(),
                listed.join(":\n")
            );
            std::fs::create_dir_all(&deps).unwrap();
            std::fs::write(&dep_info, text).unwrap();
            Unit {
                package: member.to_owned(),
                target: format!("lib target `{member}`"),
                root_file: self.path(&format!("crates/{member}/src/lib.rs")),
                dep_info,
                fresh: false,
            }
        }

        /// The comparison for this workspace, with the members `a` and `c` and the data files `data`.
        fn comparison(&self, data: &[DataFiles]) -> Comparison {
            Comparison::new(&self.0, &self.sources(), data, &["a", "c"])
                .unwrap_or_else(|problem| panic!("{problem}"))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A path relative to the root, built from separate parts, so that the expected messages match on Windows too.
    fn relative(parts: &[&str]) -> String {
        parts.iter().collect::<PathBuf>().display().to_string()
    }

    /// `data/table.txt`, listed for the members `members`.
    fn table(members: &'static [&'static str]) -> DataFiles {
        DataFiles {
            files: &["data/table.txt"],
            members,
            reason: "a table that the tests read",
        }
    }

    #[test]
    fn accepts_the_files_that_the_source_check_read() {
        let scratch = Scratch::new("read");
        let mut comparison = scratch.comparison(&[]);
        let units = [
            scratch.unit("a", 1, &["crates/a/src/lib.rs", "crates/a/src/b.rs"]),
            // Named as the compiler reached them, also through `..`, and by an absolute path.
            scratch.unit(
                "a",
                2,
                &[
                    "crates/a/src/lib.rs",
                    "crates/a/src/../src/b.rs",
                    &scratch.path("crates/c/src/lib.rs").display().to_string(),
                ],
            ),
        ];
        let line = comparison
            .check(&units, LONG_AGO)
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.starts_with("the 2 units of workspace members: the compiler read only files that the source check read, besides Clippy's clippy.toml and Cargo.toml and 0 data files"),
            "{line}"
        );
    }

    #[test]
    fn rejects_files_that_the_source_check_never_read() {
        let scratch = Scratch::new("unread");
        let mut comparison = scratch.comparison(&[]);
        let units = [
            scratch.unit(
                "a",
                1,
                &[
                    "crates/a/src/lib.rs",
                    "crates/a/src/../../../shared/sine.rs",
                ],
            ),
            scratch.unit("c", 2, &["crates/c/src/lib.rs"]),
            scratch.unit(
                "c",
                3,
                &[
                    "crates/c/src/lib.rs",
                    "crates/c/src/../../../data/table.txt",
                ],
            ),
        ];
        let problem = comparison.check(&units, LONG_AGO).unwrap_err();
        let expected = format!(
            "{}:\n  a, lib target `a`: {}\n  c, lib target `c`: {}\n{}",
            UNREAD.0,
            relative(&["shared", "sine.rs"]),
            relative(&["data", "table.txt"]),
            UNREAD.1
        );
        assert_eq!(problem, expected);
        // It says what to do, and never shows what a file holds.
        assert!(
            problem.contains("list it in DATA_FILES in xtask/src/verify.rs"),
            "{problem}"
        );
        assert!(problem.contains("a plain `mod name;`"), "{problem}");
        assert!(!problem.contains("never shown"), "{problem}");
    }

    #[test]
    fn accepts_clippys_two_files_for_every_member_only_as_clippy_names_them() {
        let scratch = Scratch::new("clippy");
        let mut comparison = scratch.comparison(&[]);
        // Every unit that `Scratch::unit` writes lists them, as Clippy does.
        let units = [
            scratch.unit("a", 1, &["crates/a/src/lib.rs"]),
            scratch.unit("c", 2, &["crates/c/src/lib.rs"]),
        ];
        comparison
            .check(&units, LONG_AGO)
            .unwrap_or_else(|problem| panic!("{problem}"));
        // The same files named in another way may have been compiled as code, such as a module that `#[path = "../../../Cargo.toml"]` reads.
        let other_names = format!("{}/./clippy.toml", scratch.0.display());
        let units = [scratch.unit(
            "a",
            3,
            &[
                "crates/a/src/lib.rs",
                "crates/a/src/../../../Cargo.toml",
                &other_names,
            ],
        )];
        let problem = comparison.check(&units, LONG_AGO).unwrap_err();
        assert!(
            problem.contains("  a, lib target `a`: Cargo.toml\n  a, lib target `a`: clippy.toml\n"),
            "{problem}"
        );
    }

    #[test]
    fn accepts_a_listed_data_file_only_for_its_members() {
        let scratch = Scratch::new("data");
        let data = [table(&["a"])];
        let mut comparison = scratch.comparison(&data);
        let read_by_a = scratch.unit(
            "a",
            1,
            &[
                "crates/a/src/lib.rs",
                "crates/a/src/../../../data/table.txt",
            ],
        );
        let line = comparison
            .check(std::slice::from_ref(&read_by_a), LONG_AGO)
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.contains("and 1 data files that DATA_FILES lists"),
            "{line}"
        );
        let read_by_c = scratch.unit(
            "c",
            2,
            &[
                "crates/c/src/lib.rs",
                "crates/c/src/../../../data/table.txt",
            ],
        );
        let problem = comparison
            .check(&[read_by_a, read_by_c], LONG_AGO)
            .unwrap_err();
        assert!(
            problem.contains(&format!(
                "{}:\n  c, lib target `c`: data/table.txt (DATA_FILES lists it for a only)\n",
                UNREAD.0
            )),
            "{problem}"
        );
        assert!(!problem.contains("a, lib target"), "{problem}");
    }

    #[test]
    fn reports_entries_that_a_named_member_did_not_read_in_any_run() {
        let scratch = Scratch::new("stale");
        let data = [table(&["a", "c"])];
        let mut comparison = scratch.comparison(&data);
        let read_by_a = scratch.unit(
            "a",
            1,
            &[
                "crates/a/src/lib.rs",
                "crates/a/src/../../../data/table.txt",
            ],
        );
        let read_by_c = scratch.unit(
            "c",
            2,
            &[
                "crates/c/src/lib.rs",
                "crates/c/src/../../../data/table.txt",
            ],
        );
        // In the first run only a reads the file.
        comparison
            .check(&[read_by_a], LONG_AGO)
            .unwrap_or_else(|problem| panic!("{problem}"));
        let problem = comparison.check_stale().unwrap_err();
        assert_eq!(
            problem,
            format!(
                "{}:\n  data/table.txt, by c (listed as a table that the tests read)\n{}",
                STALE.0, STALE.1
            )
        );
        // Once c reads it in another run, nothing is stale.
        comparison
            .check(&[read_by_c], LONG_AGO)
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert_eq!(
            comparison.check_stale().unwrap(),
            "DATA_FILES: each of its 1 files was read by every member that its entry names"
        );
        // A list that nothing needs is not stale either.
        assert_eq!(
            scratch.comparison(&[]).check_stale().unwrap(),
            "DATA_FILES is empty, and no member read a data file outside it"
        );
    }

    #[test]
    fn requires_the_targets_root_file_in_the_list() {
        let scratch = Scratch::new("root-file");
        let mut comparison = scratch.comparison(&[]);
        // A list that lacks the unit's own root file was misread, or belongs to another unit.
        let problem = comparison
            .check(&[scratch.unit("a", 1, &["crates/a/src/b.rs"])], LONG_AGO)
            .unwrap_err();
        let dep_info = relative(&["target", "debug", "deps", "a-1.d"]);
        let root_file = relative(&["crates", "a", "src", "lib.rs"]);
        assert_eq!(
            problem,
            format!(
                "a, lib target `a`: its dependency file, {dep_info}, does not list the target's root file, {root_file}, so the gate cannot be sure that it read the list"
            )
        );
    }

    #[test]
    fn fails_on_what_it_cannot_follow() {
        let scratch = Scratch::new("cannot-follow");
        let mut comparison = scratch.comparison(&[]);
        // No dependency file.
        let mut missing = scratch.unit("a", 1, &["crates/a/src/lib.rs"]);
        missing.dep_info.set_file_name("a-9.d");
        let problem = comparison.check(&[missing], LONG_AGO).unwrap_err();
        assert!(
            problem.contains("cannot read the dependency file that the compiler wrote for it")
                && problem.contains("so the gate cannot tell which files the compiler read"),
            "{problem}"
        );
        // A line it cannot read.
        let garbled = scratch.unit("a", 2, &["crates/a/src/lib.rs"]);
        let mut text = std::fs::read_to_string(&garbled.dep_info).unwrap();
        text.push_str("crates/a/src/b.rs\n");
        std::fs::write(&garbled.dep_info, text).unwrap();
        let problem = comparison.check(&[garbled], LONG_AGO).unwrap_err();
        assert!(
            problem.contains("cannot read its dependency file")
                && problem.contains("is neither a rule nor a comment"),
            "{problem}"
        );
        // A file that does not exist.
        let problem = comparison
            .check(
                &[scratch.unit("a", 3, &["crates/a/src/lib.rs", "crates/a/src/gone.rs"])],
                LONG_AGO,
            )
            .unwrap_err();
        assert!(
            problem.contains("lists crates/a/src/gone.rs, which the gate cannot find"),
            "{problem}"
        );
    }

    /// Sets when `file` was last written.
    fn last_written(file: &Path, time: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(file)
            .and_then(|file| file.set_modified(time))
            .unwrap();
    }

    #[test]
    fn requires_a_dependency_file_written_in_the_run_from_a_unit_that_cargo_compiled() {
        let scratch = Scratch::new("written-in-the-run");
        let mut comparison = scratch.comparison(&[]);
        let run_started = SystemTime::now();
        let mut unit = scratch.unit("a", 1, &["crates/a/src/lib.rs"]);
        // A dependency file left over from an earlier build, an hour before the run, is not the record of a compilation in the run.
        last_written(&unit.dep_info, run_started - Duration::from_secs(3600));
        let problem = comparison
            .check(std::slice::from_ref(&unit), run_started)
            .unwrap_err();
        let dep_info = relative(&["target", "debug", "deps", "a-1.d"]);
        assert_eq!(
            problem,
            format!(
                "a, lib target `a`: Cargo compiled it in this run, but its dependency file, {dep_info}, was last written before the run began, so it is not the record of this compilation and the gate cannot tell which files the compiler read"
            )
        );
        // A unit that Cargo reused from an earlier build keeps the dependency file of that build.
        unit.fresh = true;
        comparison
            .check(std::slice::from_ref(&unit), run_started)
            .unwrap_or_else(|problem| panic!("{problem}"));
        // A file system that stores whole seconds gives a file written in the run's first second the start of that second, which counts as written in the run; the second before does not.
        unit.fresh = false;
        let second_began = whole_second(run_started);
        last_written(&unit.dep_info, second_began);
        comparison
            .check(std::slice::from_ref(&unit), run_started)
            .unwrap_or_else(|problem| panic!("{problem}"));
        last_written(&unit.dep_info, second_began - Duration::from_secs(1));
        let problem = comparison
            .check(std::slice::from_ref(&unit), run_started)
            .unwrap_err();
        assert!(
            problem.contains("was last written before the run began"),
            "{problem}"
        );
    }

    #[test]
    fn checks_the_entries_of_data_files() {
        let scratch = Scratch::new("entries");
        for (entry, problem) in [
            (table(&[]), "the entry for data/table.txt names no member"),
            (table(&["b"]), "`b` is not a workspace member"),
            (
                DataFiles {
                    files: &["data/missing.txt"],
                    members: &["a"],
                    reason: "",
                },
                "data/missing.txt is not a file",
            ),
            (
                DataFiles {
                    files: &["data"],
                    members: &["a"],
                    reason: "",
                },
                "data is not a file",
            ),
            (
                DataFiles {
                    files: &["crates/a/src/b.rs"],
                    members: &["a"],
                    reason: "",
                },
                "crates/a/src/b.rs is read by the source check anyway",
            ),
            (
                DataFiles {
                    files: &["crates/a/../../data/table.txt"],
                    members: &["a"],
                    reason: "",
                },
                "is not a path relative to the root",
            ),
            (
                DataFiles {
                    files: &["/data/table.txt"],
                    members: &["a"],
                    reason: "",
                },
                "is not a path relative to the root",
            ),
            (
                DataFiles {
                    files: &["data\\table.txt"],
                    members: &["a"],
                    reason: "",
                },
                "is not a path relative to the root",
            ),
        ] {
            let error =
                Comparison::new(&scratch.0, &scratch.sources(), &[entry], &["a", "c"]).unwrap_err();
            assert!(error.contains(problem), "{error}");
            assert!(
                error.starts_with("DATA_FILES in xtask/src/verify.rs is not right:"),
                "{error}"
            );
        }
        let twice = [table(&["a"]), table(&["c"])];
        let error =
            Comparison::new(&scratch.0, &scratch.sources(), &twice, &["a", "c"]).unwrap_err();
        assert!(error.contains("data/table.txt is listed twice"), "{error}");
    }

    #[test]
    fn the_real_data_files_are_listed_correctly() {
        let root = crate::workspace_root();
        let packages = crate::policy::packages(&crate::policy::metadata(&root).unwrap()).unwrap();
        let allowed = crate::sources::Allowed {
            conditions: &crate::verify::APPROVED_CONDITIONS,
            canary_switch: crate::verify::CANARY_SWITCH,
            canary_package: crate::verify::CANARY_PACKAGE,
        };
        let sources = crate::sources::check(&root, &packages, &allowed)
            .unwrap_or_else(|problem| panic!("{problem}"))
            .files;
        let members: Vec<&str> = packages
            .iter()
            .map(|package| package.name.as_str())
            .collect();
        Comparison::new(&root, &sources, &crate::verify::DATA_FILES, &members)
            .unwrap_or_else(|problem| panic!("{problem}"));
        for entry in &crate::verify::DATA_FILES {
            assert!(!entry.reason.is_empty(), "{entry:?}");
        }
    }
}
