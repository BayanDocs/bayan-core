//! The lint audit: the Clippy step fails whenever a lint that the workspace's lint tables or the compiler turn on is silenced by anything other than an `#[expect(<lint>, reason = "…")]` written out in full in the source, whatever wrote the attribute that silenced it (work package CORE-009).
//!
//! # The hole it closes
//!
//! Clippy's `allow_attributes` and `allow_attributes_without_reason` lints reject an `#[allow]` and an exception without a reason, but only where the attribute is written out in the source. A macro can assemble one from tokens that it is handed separately, such as the `#` of a doc attribute and a bracket group `[allow(clippy::disallowed_types)]`, and a macro of another crate, such as std's `thread_local!`, re-emits an `#[allow]` written in full inside its input without Clippy reporting it. Either way, every lint that is not set to `forbid` could be switched off without anyone writing an exception: the hash-map ban of ADR-0005 §5, `unwrap_used`, the numeric casts, `missing_docs`, and the lints that the compiler turns on by default, such as `unused_must_use`.
//!
//! # How it works
//!
//! After the four Clippy runs, which stay as they are, so that the compiler remains the first line of enforcement, the audit compiles the workspace again in each run's configuration, with every guarded lint passed as `--force-warn <lint>` after `--` (`guarded.rs`). A force-warned lint is reported wherever it occurs, also where an attribute sets it to `allow` or `expect`, and the report is a warning that ignores `-D warnings`, so the runs enforce nothing by their exit status: after the Clippy runs passed, every occurrence they report is one that an attribute silences in those runs. Each occurrence must lie where an exception written out in full covers it ([`coverage`]); any other fails the step, with the lint, the file, the line and the macro that wrote the code, and so does an occurrence in a file that the source check did not read, which no exception written in the workspace can cover. The compiler reports code that a macro writes at the macro's own code, the same place for every call of the macro, so an occurrence is told apart by its places together with the whole chain of macro calls that wrote the code there, and each call is checked on its own: an exception that covers one call says nothing about another, which something else may silence. Lints set to `forbid` need nothing, because the compiler rejects every exception to them (error E0453). Before the runs, the audit checks the skip list against the pinned compiler with an empty crate ([`check_refusals`]).
//!
//! An exception inside a `cfg_attr` applies only where its condition holds, and `test` is one of the conditions, so the audit must know which unit reported each occurrence: the library compiled as itself or as its unit tests. Cargo's messages do not say, because both units have the same target. Each audit run therefore runs Cargo twice in the same configuration: first for the units that are not tests (`--lib --bins --examples`), then for all targets (`--all-targets`). Where the second run replays the messages of a target's unit that is not a test from the first, because it finds that very unit unchanged, what it reports beyond the first run for that target comes from tests; integration tests and benchmarks are always tests. The audit checks the replay instead of assuming it, with Cargo's `compiler-artifact` messages, because it often does not happen: without the tests and their dev-dependencies, Cargo can resolve the features of some dependencies differently (syn's, for one), so the first run checks those dependencies again in another variant, on a cold build folder, and compiles another unit of each member that uses them, which the second run then compiles again instead of replaying. For a target whose unit that is not a test the second run did not replay (another unit, with other files, one compiled again, or none), the audit cannot tell the tests' messages from the others, so it judges every occurrence of that target both as a test and as not a test, and an exception inside `cfg_attr` counts only if it holds both ways. The second run is the one whose units are checked for every member, and a message of the first run that the second does not repeat, for a target that it replayed, is an error. The audit runs share the build folder with the Clippy runs: their arguments differ, so Cargo recompiles the members in each, and the Clippy step's check of when each dependency file was written still holds for the units that each Clippy run compiles.
//!
//! # What it cannot see
//!
//! - Attributes that change linting without setting a level, such as `#[automatically_derived]` or `#[clippy::msrv]`, and code that a lint's own implementation skips: a lint that is never reported cannot be checked, by Clippy or by the audit.
//! - Code in the expansion of a macro of another crate, which the compiler does not report for most lints, with or without force-warning.
//! - Where tokens are copied from: tokens that a derive or an attribute macro copies from its input into a separate item are judged where they are written, so an `#[expect]` on the input item counts for them, although the compiler applies it to that item only. This is accepted, because the exception is written over that very text.
//! - The lints that cannot be force-warned: the `warnings` group, which the compiler refuses (error E0602), and the unstable lints of the skip list (`guarded::UNGUARDED`), which it calls unknown.

mod coverage;
mod guarded;
mod messages;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::policy::Package;

pub use coverage::Config;
pub use guarded::{Guarded, check_refusals};

use coverage::{Expansion, Model, Place};
use messages::{Reported, Span, TargetKey, Unit};

/// How occurrences that no exception written out in full covers are reported: the heading of their list, and what to do.
const UNCOVERED: (&str, &str) = (
    "lints are silenced by something other than an `#[expect(lint, reason = \"…\")]` written out in full",
    "The audit compiled the workspace again with every lint that the lint tables or the compiler turn on force-warned, so that the compiler reports each occurrence of them also where an attribute silences it, and no exception written out in full in the source covers the occurrences above. An attribute that a macro assembles from its arguments, an `#[allow]` that a macro of another crate puts out, or a written `#[expect]` that the audit counts for nothing silences them in the Clippy runs. Write `#[expect(<lint>, reason = \"…\")]` out in full on the smallest item, statement, field or match arm that holds the occurrence; for code that a macro writes, put it next to that code in the macro's own definition, or on the item or statement that holds the macro's call. The audit counts a written `#[expect]` for nothing on a generic parameter, on a parameter's type (on a parameter, it covers only the pattern), on a field of a struct expression or pattern, on a parameter of a function pointer type, on the first statement of a block after `if x` and the like, anywhere inside a macro's input (also one that the macro hands on through `$(#[$meta])*`), on a macro call and on a `macro_rules!` definition, because it cannot tell how far the compiler applies it there: move such an `#[expect]` onto the enclosing item or statement. Never lower a lint in another way. See \"Lint rules and exceptions\" in AGENTS.md.",
);

/// One occurrence, found in one or more units and runs.
#[derive(Debug)]
struct Occurrence {
    /// Where it is, for a person: the file relative to the root, the line and the column, and the macros that wrote the code there, each with the place where it was called.
    shown: String,
    /// The crate root and configuration of each unit that reported it.
    units: BTreeSet<(PathBuf, Config)>,
    /// The runs that reported it.
    runs: BTreeSet<String>,
}

/// An occurrence that no exception written out in full covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uncovered {
    /// The lint.
    pub lint: String,
    /// The file where the compiler reports it, as the file system names it.
    pub file: PathBuf,
    /// A line describing it.
    pub shown: String,
}

/// The audit of the occurrences of guarded lints that one or more runs report.
pub struct Audit<'a> {
    /// The workspace root, as the file system names it; the compiler names the members' files relative to it.
    root: PathBuf,
    /// The workspace members.
    packages: &'a [Package],
    /// What the runs force-warn.
    guarded: &'a Guarded,
    /// Which exceptions written out in full cover which code.
    model: Model,
    /// The occurrences, by lint and by where the compiler reports them: its primary spans, each with the chain of macro calls that wrote the code there, so that two calls of one macro are two occurrences, while the same occurrence reported by several units or runs is one.
    occurrences: BTreeMap<(String, Vec<Place>), Occurrence>,
    /// The force-warned lints that the compiler called unknown.
    unknown: BTreeSet<String>,
    /// How many reports were of lints that the reporting member's lint table sets to `allow`.
    ignored: usize,
}

impl<'a> Audit<'a> {
    /// Prepares the audit of the workspace at `root`, whose members are `packages` and whose Rust files the source check read as `sources`.
    pub fn new(
        root: &Path,
        packages: &'a [Package],
        sources: &BTreeSet<PathBuf>,
        guarded: &'a Guarded,
    ) -> Result<Self, String> {
        let root = std::fs::canonicalize(root)
            .map_err(|error| format!("cannot find {}: {error}", root.display()))?;
        let sources = sources
            .iter()
            .map(|file| {
                std::fs::canonicalize(file)
                    .map_err(|error| format!("cannot find {}: {error}", file.display()))
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(Self {
            root,
            packages,
            guarded,
            model: Model::new(sources),
            occurrences: BTreeMap::new(),
            unknown: BTreeSet::new(),
            ignored: 0,
        })
    }

    /// Adds what one audit run reported, in the configuration `config` (whose `test` it sets for each unit): `non_tests`, what Cargo printed when it compiled the units that are not tests, and `all`, what it printed when it then compiled all targets. Returns one line describing what was found.
    pub fn add_run(
        &mut self,
        run: &str,
        config: &Config,
        non_tests: &str,
        all: &str,
    ) -> Result<String, String> {
        let edition = &self.guarded.edition;
        let first = messages::read(non_tests, self.packages, edition)
            .map_err(|problem| format!("{run}, the units that are not tests: {problem}"))?;
        let second = messages::read(all, self.packages, edition)
            .map_err(|problem| format!("{run}, all targets: {problem}"))?;
        let (replayed, not_replayed) = replayed(&first.units, &second.units);
        let mut in_all: BTreeMap<&Reported, usize> = BTreeMap::new();
        for reported in &second.reported {
            *in_all.entry(reported).or_default() += 1;
        }
        let mut in_non_tests: BTreeMap<&Reported, usize> = BTreeMap::new();
        for reported in &first.reported {
            if reported.always_tests() {
                return Err(format!(
                    "{run}: the units that are not tests reported `{}` for the tests `{}` of {}, so the audit cannot tell which units reported what",
                    reported.lint, reported.target, reported.member
                ));
            }
            *in_non_tests.entry(reported).or_default() += 1;
        }
        for (reported, count) in &in_non_tests {
            if replayed.contains(&reported.target_key())
                && in_all.get(reported).copied().unwrap_or_default() < *count
            {
                return Err(format!(
                    "{run}: the run of all targets did not repeat `{}` at {}:{} of {}, which the units that are not tests reported, so the audit cannot tell which units reported what",
                    reported.lint, reported.spans[0].file, reported.spans[0].line, reported.member
                ));
            }
        }
        let tests = Config {
            test: true,
            ..config.clone()
        };
        let not_tests = Config {
            test: false,
            ..config.clone()
        };
        // A target whose unit that is not a test the second invocation did not replay: its occurrences count both as tests and not.
        for reported in in_non_tests.keys() {
            self.add(reported, &not_tests, run)?;
            if !replayed.contains(&reported.target_key()) {
                self.add(reported, &tests, run)?;
            }
        }
        for (reported, count) in &in_all {
            if reported.always_tests() {
                self.add(reported, &tests, run)?;
            } else if !replayed.contains(&reported.target_key()) {
                self.add(reported, &not_tests, run)?;
                self.add(reported, &tests, run)?;
            } else if *count > in_non_tests.get(reported).copied().unwrap_or_default() {
                self.add(reported, &tests, run)?;
            }
        }
        let mut summary = format!(
            "{run}: the compiler reported {} occurrences of force-warned lints ({} in the units that are not tests)",
            second.reported.len(),
            first.reported.len()
        );
        if not_replayed > 0 {
            summary.push_str(&format!(
                "; the second Cargo invocation did not replay the units that are not tests of {not_replayed} targets from the first, so their occurrences count both as tests and not"
            ));
        }
        Ok(summary)
    }

    /// Adds what one compilation reported whose units all have the configuration `config`, such as a case of the lint canary.
    pub fn add_compilation(
        &mut self,
        run: &str,
        config: &Config,
        text: &str,
    ) -> Result<(), String> {
        let reported = messages::read(text, self.packages, &self.guarded.edition)
            .map_err(|problem| format!("{run}: {problem}"))?
            .reported;
        for occurrence in &reported {
            self.add(occurrence, config, run)?;
        }
        Ok(())
    }

    /// Records one report of the unit configuration `config` in the run `run`.
    fn add(&mut self, reported: &Reported, config: &Config, run: &str) -> Result<(), String> {
        if let Some(lint) = &reported.unknown
            && self.guarded.lints.contains(lint)
        {
            self.unknown.insert(lint.clone());
            return Ok(());
        }
        if self
            .guarded
            .allowed
            .get(&reported.member)
            .is_some_and(|allowed| allowed.contains(&reported.lint))
        {
            self.ignored += 1;
            return Ok(());
        }
        let root = self.root.join(&reported.root);
        let root = std::fs::canonicalize(&root)
            .map_err(|error| format!("cannot find {}: {error}", root.display()))?;
        let places = reported
            .spans
            .iter()
            .map(|span| self.place(span))
            .collect::<Result<Vec<_>, _>>()?;
        let shown = self.show(&reported.spans[0]);
        let occurrence = self
            .occurrences
            .entry((reported.lint.clone(), places))
            .or_insert_with(|| Occurrence {
                shown,
                units: BTreeSet::new(),
                runs: BTreeSet::new(),
            });
        occurrence.units.insert((root, config.clone()));
        occurrence.runs.insert(run.to_owned());
        Ok(())
    }

    /// The place that a span names, with the macro expansions that wrote the code there. Fails if the line that the compiler names is not the line of the byte it names, so that a misreading of positions cannot pass.
    fn place(&mut self, span: &Span) -> Result<Place, String> {
        let file = self.resolve(&span.file);
        if self.model.reads(&file) {
            let line = self.model.line_of(&file, span.byte)?;
            if line != span.line {
                return Err(format!(
                    "the compiler names line {} of {} and the byte at {}, which is on line {line} of the file as it is on disk, so the audit cannot tell where it reports",
                    span.line, span.file, span.byte
                ));
            }
        }
        let expansion = match &span.expansion {
            None => None,
            Some(call) => Some(Box::new(Expansion {
                definition: call
                    .definition
                    .as_ref()
                    .map(|(file, byte)| (self.resolve(file), *byte)),
                call_site: self.place(&call.call_site)?,
            })),
        };
        Ok(Place {
            file,
            byte: span.byte,
            expansion,
        })
    }

    /// The file that the compiler names `name`, as the file system names it if it exists.
    fn resolve(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::canonicalize(&path).unwrap_or(path)
    }

    /// `name` relative to the root if it lies inside.
    fn relative(&self, name: &str) -> String {
        let path = self.resolve(name);
        path.strip_prefix(&self.root)
            .unwrap_or(&path)
            .display()
            .to_string()
    }

    /// A span for a person: its file relative to the root, its line and column, and the macros that wrote the code there.
    fn show(&self, span: &Span) -> String {
        let mut shown = format!(
            "{}:{}:{}",
            self.relative(&span.file),
            span.line,
            span.column
        );
        let mut expansion = span.expansion.as_deref();
        while let Some(call) = expansion {
            shown.push_str(&format!(
                ", written by the macro `{}` called at {}:{}:{}",
                call.name,
                self.relative(&call.call_site.file),
                call.call_site.line,
                call.call_site.column
            ));
            expansion = call.call_site.expansion.as_deref();
        }
        shown
    }

    /// The occurrences that no exception written out in full covers in some unit that reported them, each call of a macro on its own.
    pub fn uncovered(&mut self) -> Result<Vec<Uncovered>, String> {
        let Self {
            occurrences, model, ..
        } = self;
        let mut uncovered = Vec::new();
        for ((lint, places), occurrence) in occurrences.iter() {
            let mut covered = true;
            for (root, config) in &occurrence.units {
                for place in places {
                    covered &= model.covers(lint, place, root, config)?;
                }
            }
            if covered {
                continue;
            }
            let outside = places.iter().any(|place| !model.reads(&place.file));
            let runs: Vec<&str> = occurrence.runs.iter().map(String::as_str).collect();
            uncovered.push(Uncovered {
                lint: lint.clone(),
                file: places[0].file.clone(),
                shown: format!(
                    "  {}: `{}`{} ({})",
                    occurrence.shown,
                    lint,
                    if outside {
                        ", in a file that the source check did not read, so that no exception written in the workspace can cover it"
                    } else {
                        ""
                    },
                    runs.join("; ")
                ),
            });
        }
        Ok(uncovered)
    }

    /// Fails if the compiler called a force-warned lint unknown, or if an occurrence lies where no exception written out in full covers it; otherwise returns one line describing what was checked.
    pub fn finish(&mut self) -> Result<String, String> {
        if !self.unknown.is_empty() {
            let unknown: Vec<&str> = self.unknown.iter().map(String::as_str).collect();
            return Err(format!(
                "the compiler calls the force-warned lints {} unknown, and the skip list of the lint audit (UNGUARDED in xtask/src/lint_audit/guarded.rs) does not hold them. A lint table may name a lint that this toolchain does not know, or the toolchain changed; fix the lint table, or list the lints with the reason in a reviewed pull request.",
                unknown.join(", ")
            ));
        }
        let uncovered = self.uncovered()?;
        if !uncovered.is_empty() {
            let lines: Vec<&str> = uncovered.iter().map(|found| found.shown.as_str()).collect();
            return Err(format!(
                "{}:\n{}\n{}",
                UNCOVERED.0,
                lines.join("\n"),
                UNCOVERED.1
            ));
        }
        let places = self.occurrences.len();
        Ok(format!(
            "lint audit: an `#[expect(…, reason = \"…\")]` written out in full that names its lint covers each of the {places} places where the force-warned runs report a silenced lint, each call of a macro on its own ({} reports of lints that a lint table sets to allow are left out)",
            self.ignored
        ))
    }
}

/// Of the targets whose unit that is not a test the first Cargo invocation of an audit run compiled or found unchanged (`first`), those whose very unit the second (`second`) replayed: the same unit, with the same files, which Cargo found unchanged, so that it repeated the first invocation's messages for it. Without the tests and their dev-dependencies, Cargo can resolve the features of some dependencies differently and so build another unit of the same target in each invocation, and it compiles a unit again when something changed. Also returns how many targets the second invocation did not replay.
fn replayed(first: &[Unit], second: &[Unit]) -> (BTreeSet<TargetKey>, usize) {
    let mut replayed = BTreeSet::new();
    let mut not_replayed = 0;
    for unit in first.iter().filter(|unit| !unit.test) {
        if second.iter().any(|later| {
            !later.test
                && later.fresh
                && later.target == unit.target
                && later.filenames == unit.filenames
        }) {
            replayed.insert(unit.target.clone());
        } else {
            not_replayed += 1;
        }
    }
    (replayed, not_replayed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Target;

    /// A workspace with one member, `a`, whose library is `crates/a/src/lib.rs`, in a folder of its own that is removed again when the test ends.
    struct Workspace {
        root: PathBuf,
        packages: Vec<Package>,
        sources: BTreeSet<PathBuf>,
    }

    impl Workspace {
        fn new(test: &str, library: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("bayan-xtask-audit-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let file = under(&root, LIBRARY);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, library).unwrap();
            let root = std::fs::canonicalize(&root).unwrap();
            Self {
                packages: vec![Package {
                    name: "a".to_owned(),
                    folder: under(&root, "crates/a"),
                    targets: vec![Target {
                        kinds: vec!["lib".to_owned()],
                        name: "a".to_owned(),
                        root_file: under(&root, LIBRARY),
                    }],
                }],
                sources: BTreeSet::from([under(&root, LIBRARY)]),
                root,
            }
        }

        /// A message for an occurrence of `lint` at the first `needle` in the library, written by the macro defined at `macro_rules` and called at `call`, if one is given.
        fn message(&self, lint: &str, needle: &str, call: Option<&str>) -> String {
            let text = std::fs::read_to_string(under(&self.root, LIBRARY)).unwrap();
            // As the compiler names the file: relative to the root, with the platform's separator.
            let file = json_string(&under(Path::new(""), LIBRARY));
            let span = |needle: &str, primary: bool, expansion: String| {
                let byte = text.find(needle).unwrap_or_else(|| panic!("no {needle}"));
                let line = text[..byte].matches('\n').count() + 1;
                let column = byte - text[..byte].rfind('\n').map_or(0, |at| at + 1) + 1;
                format!(
                    r#"{{"byte_end":{},"byte_start":{byte},"column_end":{},"column_start":{column},"expansion":{expansion},"file_name":{file},"is_primary":{primary},"label":null,"line_end":{line},"line_start":{line},"suggested_replacement":null,"suggestion_applicability":null,"text":[]}}"#,
                    byte + needle.len(),
                    column + needle.len()
                )
            };
            let expansion = call.map_or("null".to_owned(), |call| {
                format!(
                    r#"{{"def_site_span":{},"macro_decl_name":"exempt!","span":{}}}"#,
                    span("macro_rules", false, "null".to_owned()),
                    span(call, false, "null".to_owned())
                )
            });
            let manifest = json_string(&under(&self.root, "crates/a/Cargo.toml"));
            let source = json_string(&under(&self.root, LIBRARY));
            format!(
                r#"{{"reason":"compiler-message","package_id":"a","manifest_path":{manifest},"target":{{"kind":["lib"],"crate_types":["lib"],"name":"a","src_path":{source},"edition":"2024","doc":true,"doctest":true,"test":true}},"message":{{"rendered":"","$message_type":"diagnostic","children":[],"code":{{"code":"{lint}","explanation":null}},"level":"warning","message":"a message","spans":[{}]}}}}"#,
                span(needle, true, expansion)
            )
        }
    }

    impl Workspace {
        /// The `compiler-artifact` message of a unit of the library, compiled as tests (`test`) or not, whose file is `file`, and which Cargo found unchanged (`fresh`) or compiled.
        fn artifact(&self, test: bool, file: &str, fresh: bool) -> String {
            let manifest = json_string(&under(&self.root, "crates/a/Cargo.toml"));
            let source = json_string(&under(&self.root, LIBRARY));
            let file = json_string(&under(&self.root, &format!("target/debug/deps/{file}")));
            format!(
                r#"{{"reason":"compiler-artifact","package_id":"a","manifest_path":{manifest},"target":{{"kind":["lib"],"crate_types":["lib"],"name":"a","src_path":{source},"edition":"2024","doc":true,"doctest":true,"test":true}},"profile":{{"opt_level":"0","debuginfo":2,"debug_assertions":true,"overflow_checks":true,"test":{test}}},"features":[],"filenames":[{file}],"executable":null,"fresh":{fresh}}}"#
            )
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The library of the member `a`, relative to the root, as the compiler names it in its messages.
    const LIBRARY: &str = "crates/a/src/lib.rs";

    /// The file `name`, written with `/` between folders, below `folder`. Joined one part at a time, because Windows reads `/` as a separator only in paths without the `\\?\\` prefix that `canonicalize` gives.
    fn under(folder: &Path, name: &str) -> PathBuf {
        name.split('/')
            .fold(folder.to_path_buf(), |path, part| path.join(part))
    }

    /// A path as a JSON string.
    fn json_string(path: &Path) -> String {
        format!(
            "\"{}\"",
            path.display()
                .to_string()
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
        )
    }

    /// The messages of one run: `messages`, then the end of the run.
    fn run(messages: &[&str]) -> String {
        let mut text = String::new();
        for message in messages {
            text.push_str(message);
            text.push('\n');
        }
        text.push_str("{\"reason\":\"build-finished\",\"success\":true}\n");
        text
    }

    fn guarded(lints: &[&str], allowed: &[&str]) -> Guarded {
        let defaults: Vec<String> = lints.iter().map(|lint| (*lint).to_owned()).collect();
        let allowed = BTreeMap::from([(
            "a".to_owned(),
            allowed.iter().map(|lint| (*lint).to_owned()).collect(),
        )]);
        Guarded::new(&[], &defaults, "2024".to_owned(), allowed).unwrap()
    }

    fn host() -> Config {
        Config {
            test: false,
            debug_assertions: true,
            wasm32: false,
            canary_case: None,
        }
    }

    const MAP: &str = "clippy::disallowed_types";

    #[test]
    fn fails_on_an_allow_that_a_macro_assembles_naming_the_lint_the_file_and_the_line() {
        let workspace = Workspace::new(
            "split",
            "macro_rules! exempt { ($hash:tt $decoy:tt, $bracket:tt) => { $hash $bracket /// Entries.\n#[must_use] pub fn entries() -> usize { let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new(); map.len() } }; }\nexempt!(#[doc = \"Entries.\"], [allow(clippy::disallowed_types)]);\n",
        );
        let guarded = guarded(&["clippy::all"], &[]);
        let mut audit = Audit::new(
            &workspace.root,
            &workspace.packages,
            &workspace.sources,
            &guarded,
        )
        .unwrap();
        let message = workspace.message(MAP, "std::collections::HashMap::new", Some("exempt!("));
        audit
            .add_compilation("run 1", &host(), &run(&[&message]))
            .unwrap();
        let error = audit.finish().unwrap_err();
        assert!(error.starts_with(UNCOVERED.0), "{error}");
        // The file as the platform writes paths relative to the root.
        let file = under(Path::new(""), LIBRARY).display().to_string();
        assert!(
            error.contains(&format!("  {file}:2:"))
                && error.contains(&format!(
                    "written by the macro `exempt!` called at {file}:3:1"
                ))
                && error.contains(": `clippy::disallowed_types` (run 1)"),
            "{error}"
        );
    }

    /// One macro, called in two functions: the compiler reports the code it writes at the same place in its definition for both calls.
    const TWO_CALLS: &str = "macro_rules! old { ($x:expr) => { crate::legacy($x) }; }\n#[deprecated]\npub fn legacy(x: u8) -> u8 { x }\n#[expect(deprecated, reason = \"the first call uses the old function on purpose\")]\npub fn first() -> u8 { old!(1) }\npub fn second() -> u8 { old!(2) }\n";

    #[test]
    fn checks_each_call_of_a_macro_on_its_own() {
        let workspace = Workspace::new("two-calls", TWO_CALLS);
        let guarded = guarded(&["deprecated"], &[]);
        let covered = workspace.message("deprecated", "crate::legacy", Some("old!(1)"));
        let uncovered = workspace.message("deprecated", "crate::legacy", Some("old!(2)"));
        let file = under(Path::new(""), LIBRARY).display().to_string();
        // Whichever call the compiler reports first, the call that nothing covers fails, and the error names where it is called.
        for messages in [
            [covered.as_str(), uncovered.as_str()],
            [uncovered.as_str(), covered.as_str()],
        ] {
            let mut audit = Audit::new(
                &workspace.root,
                &workspace.packages,
                &workspace.sources,
                &guarded,
            )
            .unwrap();
            audit
                .add_compilation("run 1", &host(), &run(&messages))
                .unwrap();
            let error = audit.finish().unwrap_err();
            assert!(
                error.contains(&format!("called at {file}:6:"))
                    && !error.contains(&format!("called at {file}:5:"))
                    && error.contains(": `deprecated` (run 1)"),
                "{error}"
            );
        }
        // With both calls covered, both pass, as two places.
        let workspace = Workspace::new(
            "two-calls-covered",
            &TWO_CALLS.replace(
                "pub fn second",
                "#[expect(deprecated, reason = \"so does the second\")]\npub fn second",
            ),
        );
        let covered = workspace.message("deprecated", "crate::legacy", Some("old!(1)"));
        let also_covered = workspace.message("deprecated", "crate::legacy", Some("old!(2)"));
        let mut audit = Audit::new(
            &workspace.root,
            &workspace.packages,
            &workspace.sources,
            &guarded,
        )
        .unwrap();
        audit
            .add_compilation("run 1", &host(), &run(&[&covered, &also_covered]))
            .unwrap();
        let summary = audit.finish().unwrap();
        assert!(summary.contains("each of the 2 places"), "{summary}");
    }

    #[test]
    fn passes_an_occurrence_under_an_expect_written_out_in_full() {
        let workspace = Workspace::new(
            "covered",
            "/// Entries.\n#[expect(clippy::disallowed_types, reason = \"only counted\")]\npub fn entries() -> usize { std::collections::HashMap::<u8, u8>::new().len() }\n",
        );
        let guarded = guarded(&["clippy::all"], &[]);
        let mut audit = Audit::new(
            &workspace.root,
            &workspace.packages,
            &workspace.sources,
            &guarded,
        )
        .unwrap();
        let message = workspace.message(MAP, "std::collections::HashMap", None);
        // Reported twice in the same unit and in two runs: one place.
        audit
            .add_compilation("run 1", &host(), &run(&[&message, &message]))
            .unwrap();
        audit
            .add_compilation("run 2", &host(), &run(&[&message]))
            .unwrap();
        let summary = audit.finish().unwrap();
        assert!(summary.contains("each of the 1 places"), "{summary}");
    }

    /// The library's `cfg_attr(test, expect(…))` exception, which holds only in the unit tests.
    const ONLY_TESTS: &str = "#[cfg_attr(test, expect(dead_code, reason = \"only the build that is not a test uses it\"))]\nfn helper() {}\n";

    #[test]
    fn tells_the_units_that_are_tests_from_the_others() {
        let guarded = guarded(&["dead_code"], &[]);
        let workspace = Workspace::new("tests", ONLY_TESTS);
        let message = workspace.message("dead_code", "helper", None);
        // The library's unit that is not a test, compiled by the first invocation and replayed by the second, and its unit tests.
        let compiled = workspace.artifact(false, "liba-1.rmeta", false);
        let replayed = workspace.artifact(false, "liba-1.rmeta", true);
        let tests = workspace.artifact(true, "liba-2.rmeta", false);
        let new_audit = || {
            Audit::new(
                &workspace.root,
                &workspace.packages,
                &workspace.sources,
                &guarded,
            )
            .unwrap()
        };
        // Only the unit tests report it, where the `cfg_attr` applies.
        let mut audit = new_audit();
        let summary = audit
            .add_run(
                "run 1",
                &host(),
                &run(&[&compiled]),
                &run(&[&replayed, &tests, &message]),
            )
            .unwrap();
        assert!(!summary.contains("did not replay"), "{summary}");
        assert!(audit.finish().is_ok());
        // Both report it, and the `cfg_attr` does not apply in the library that is not a test.
        let mut audit = new_audit();
        audit
            .add_run(
                "run 1",
                &host(),
                &run(&[&compiled, &message]),
                &run(&[&replayed, &message, &tests, &message]),
            )
            .unwrap();
        assert!(audit.finish().unwrap_err().contains("`dead_code`"));
        // The run of all targets must repeat what the units that are not tests reported, where it replayed them.
        let mut audit = new_audit();
        let error = audit
            .add_run(
                "run 1",
                &host(),
                &run(&[&compiled, &message]),
                &run(&[&replayed, &tests]),
            )
            .unwrap_err();
        assert!(error.contains("did not repeat"), "{error}");
    }

    /// Required by the second review of CORE-009: Cargo does not replay a unit that it builds in another variant, or compiles again.
    #[test]
    fn judges_both_ways_a_target_whose_unit_the_second_invocation_did_not_replay() {
        let guarded = guarded(&["dead_code"], &[]);
        let workspace = Workspace::new("not-replayed", ONLY_TESTS);
        let message = workspace.message("dead_code", "helper", None);
        let compiled = workspace.artifact(false, "liba-1.rmeta", false);
        let tests = workspace.artifact(true, "liba-2.rmeta", false);
        // Another unit of the same target, with other files; the same unit compiled again; or none at all.
        let another = workspace.artifact(false, "liba-3.rmeta", true);
        let again = workspace.artifact(false, "liba-1.rmeta", false);
        for second in [vec![&another], vec![&again], vec![]] {
            let mut all: Vec<&str> = second.iter().map(|text| text.as_str()).collect();
            all.extend([tests.as_str(), message.as_str()]);
            let mut audit = Audit::new(
                &workspace.root,
                &workspace.packages,
                &workspace.sources,
                &guarded,
            )
            .unwrap();
            let summary = audit
                .add_run("run 1", &host(), &run(&[&compiled]), &run(&all))
                .unwrap();
            assert!(
                summary.contains("did not replay the units that are not tests of 1 targets"),
                "{summary}"
            );
            // The occurrence counts as not a test too, where the `cfg_attr` does not apply.
            let error = audit.finish().unwrap_err();
            assert!(error.contains("`dead_code`"), "{error}");
            // And a message of the first that the second does not repeat is no error for such a target.
            let mut audit = Audit::new(
                &workspace.root,
                &workspace.packages,
                &workspace.sources,
                &guarded,
            )
            .unwrap();
            let mut only_tests: Vec<&str> = second.iter().map(|text| text.as_str()).collect();
            only_tests.push(tests.as_str());
            audit
                .add_run(
                    "run 1",
                    &host(),
                    &run(&[&compiled, &message]),
                    &run(&only_tests),
                )
                .unwrap();
        }
    }

    #[test]
    fn fails_when_the_compiler_calls_a_force_warned_lint_unknown() {
        let workspace = Workspace::new("unknown", "fn a() {}\n");
        let guarded = guarded(&["clippy::renamed_away"], &[]);
        let mut audit = Audit::new(
            &workspace.root,
            &workspace.packages,
            &workspace.sources,
            &guarded,
        )
        .unwrap();
        let message = workspace
            .message("unknown_lints", "fn a", None)
            .replace("a message", "unknown lint: `clippy::renamed_away`");
        audit
            .add_compilation("run 1", &host(), &run(&[&message]))
            .unwrap();
        let error = audit.finish().unwrap_err();
        assert!(
            error.contains("clippy::renamed_away unknown") && error.contains("UNGUARDED"),
            "{error}"
        );
    }

    #[test]
    fn leaves_out_the_lints_that_a_lint_table_sets_to_allow() {
        let workspace = Workspace::new("allowed", "fn clamp() {}\n");
        let guarded = guarded(&["clippy::all"], &["clippy::manual_clamp"]);
        let mut audit = Audit::new(
            &workspace.root,
            &workspace.packages,
            &workspace.sources,
            &guarded,
        )
        .unwrap();
        let message = workspace.message("clippy::manual_clamp", "clamp", None);
        audit
            .add_compilation("run 1", &host(), &run(&[&message]))
            .unwrap();
        let summary = audit.finish().unwrap();
        assert!(
            summary.contains("each of the 0 places") && summary.contains("1 reports"),
            "{summary}"
        );
    }

    #[test]
    fn fails_when_a_position_does_not_match_the_file() {
        let workspace = Workspace::new("misplaced", "fn a() {}\nfn b() {}\n");
        let guarded = guarded(&["dead_code"], &[]);
        let mut audit = Audit::new(
            &workspace.root,
            &workspace.packages,
            &workspace.sources,
            &guarded,
        )
        .unwrap();
        let message = workspace
            .message("dead_code", "fn b", None)
            .replace("\"line_start\":2", "\"line_start\":1");
        let error = audit
            .add_compilation("run 1", &host(), &run(&[&message]))
            .unwrap_err();
        assert!(error.contains("cannot tell where it reports"), "{error}");
    }
}
