//! The lints that the audit force-warns: every lint and group that a lint table of the workspace sets to `warn` or `deny`, and every lint that the pinned compiler turns on by default for the workspace's edition, except those that force-warning cannot name (the skip list, [`UNGUARDED`]).
//!
//! Both come from where they are decided, never from a list kept by hand: the lint tables from the manifests (`[workspace.lints]` in the root `Cargo.toml` and the copies in the binding crates' manifests), and the compiler's defaults from `rustc --edition <edition> -W help`, run with the pinned toolchain, so that a lint added to a table, or a default lint that a new toolchain brings, is guarded from the same commit. The skip list cannot go stale: [`check_refusals`] fails when the compiler calls a force-warned lint unknown that the list does not hold, and when force-warning a listed lint no longer fails for the reason the list gives.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use crate::policy::{self, Package};
use crate::{json, process, toml_subset};

/// Why force-warning a lint fails on the pinned compiler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The compiler refuses the name with error E0602: the `warnings` group, which stands for every lint that warns rather than for lints of its own.
    NotSupported,
    /// The compiler reports an unknown lint: a lint that only code with an unstable feature switched on can trigger, which the stable toolchain does not allow.
    Unknown,
}

/// A lint that the audit does not force-warn, with the reason why force-warning it fails.
#[derive(Debug, Clone, Copy)]
pub struct Unguarded {
    /// The lint, as `rustc -W help` names it with `_` in place of `-`.
    pub lint: &'static str,
    /// What the compiler does when it is force-warned.
    pub refusal: Refusal,
    /// Why, for a person.
    pub reason: &'static str,
}

/// The lints that the pinned compiler turns on by default but the audit cannot force-warn, each with the reason why force-warning it fails. The audit fails when force-warning a listed lint no longer fails for the reason given, and when the compiler calls a force-warned lint unknown that this list does not hold, so the list changes together with the toolchain, in the monthly dependency session.
pub const UNGUARDED: [Unguarded; 6] = [
    Unguarded {
        lint: "warnings",
        refusal: Refusal::NotSupported,
        reason: "a group that stands for every lint that warns; the compiler refuses to force-warn it (error E0602), and the lints it stands for are force-warned one by one",
    },
    Unguarded {
        lint: "default_overrides_default_fields",
        refusal: Refusal::Unknown,
        reason: "unstable: only code that switches on the feature `default_field_values` can trigger it",
    },
    Unguarded {
        lint: "duplicate_tools",
        refusal: Refusal::Unknown,
        reason: "unstable: only code that switches on the feature `register_tool` can trigger it",
    },
    Unguarded {
        lint: "malformed_diagnostic_filters",
        refusal: Refusal::Unknown,
        reason: "unstable: only code that switches on an unstable diagnostic attribute can trigger it",
    },
    Unguarded {
        lint: "tail_call_track_caller",
        refusal: Refusal::Unknown,
        reason: "unstable: only code that switches on the feature `explicit_tail_calls` can trigger it",
    },
    Unguarded {
        lint: "test_unstable_lint",
        refusal: Refusal::Unknown,
        reason: "unstable: exists only to test the compiler's handling of unstable lints",
    },
];

/// The lint that reports unknown lint names. It is force-warned first, so that the compiler reports every other force-warned name it does not know.
pub const UNKNOWN_LINTS: &str = "unknown_lints";

/// The lint tables of the workspace's manifests: `[workspace.lints.<tool>]` in the root `Cargo.toml`, which every other member inherits (the guardrails step checks that), and `[lints.<tool>]` in the binding crates' manifests.
const LINT_TOOLS: [&str; 3] = ["rust", "clippy", "rustdoc"];

/// What the audit force-warns, and the levels it needs to judge an occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guarded {
    /// The lints and groups to force-warn, [`UNKNOWN_LINTS`] first and then in alphabetical order, as the compiler names them (`clippy::all`, `unused_must_use`).
    pub lints: Vec<String>,
    /// How many of them a lint table sets to `warn` or `deny`.
    pub from_tables: usize,
    /// How many of them the compiler turns on by default and no lint table sets to `warn` or `deny`.
    pub from_compiler: usize,
    /// The workspace's edition, from the root `Cargo.toml`.
    pub edition: String,
    /// For each workspace member, the lints that its lint table sets to `allow`. A group such as `clippy::all` also force-warns these members of it, whose occurrences the audit ignores.
    pub allowed: BTreeMap<String, BTreeSet<String>>,
}

impl Guarded {
    /// Reads the lint tables of the root manifest and of the binding crates among `packages`, and the default levels of the compiler's lints from `rustc --edition <edition> -W help`, run in `root` so that the pinned toolchain answers.
    pub fn load(root: &Path, packages: &[Package]) -> Result<Self, String> {
        let manifest = read(&root.join("Cargo.toml"))?;
        let edition = toml_subset::value(&manifest, "workspace.package", "edition")
            .and_then(|edition| {
                edition.ok_or_else(|| "no `edition` in [workspace.package]".to_owned())
            })
            .and_then(|edition| toml_subset::string(&edition))
            .map_err(|problem| format!("the root Cargo.toml: {problem}"))?;
        let mut tables = vec![(
            "the root Cargo.toml".to_owned(),
            lint_table(&manifest, "workspace.lints")
                .map_err(|problem| format!("the root Cargo.toml: {problem}"))?,
        )];
        let mut allowed = BTreeMap::new();
        for package in packages {
            if policy::BINDING_CRATES.contains(&package.name.as_str()) {
                let path = package.folder.join("Cargo.toml");
                let table = lint_table(&read(&path)?, "lints")
                    .map_err(|problem| format!("{}: {problem}", path.display()))?;
                allowed.insert(package.name.clone(), allowed_in(&table));
                tables.push((format!("the manifest of {}", package.name), table));
            } else {
                allowed.insert(package.name.clone(), allowed_in(&tables[0].1));
            }
        }
        let mut rustc = Command::new("rustc");
        rustc
            .args(["--edition", &edition, "-W", "help"])
            .current_dir(root);
        let output = process::capture(&mut rustc)?;
        if !output.status.success() {
            return Err(format!(
                "`{}` failed:\n{}",
                process::display(&rustc),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let defaults = compiler_defaults(&String::from_utf8_lossy(&output.stdout))
            .map_err(|problem| format!("`{}`: {problem}", process::display(&rustc)))?;
        Self::new(&tables, &defaults, edition, allowed)
    }

    /// The guarded lints from the lint tables `tables` (each with where it comes from) and the lints that the compiler turns on by default, `defaults`. Fails if the list comes out empty.
    pub fn new(
        tables: &[(String, BTreeMap<String, String>)],
        defaults: &[String],
        edition: String,
        allowed: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Self, String> {
        let from_tables: BTreeSet<&str> = tables
            .iter()
            .flat_map(|(_, table)| table.iter())
            .filter(|(_, level)| matches!(level.as_str(), "warn" | "deny"))
            .map(|(lint, _)| lint.as_str())
            .collect();
        let skipped: BTreeSet<&str> = UNGUARDED.iter().map(|unguarded| unguarded.lint).collect();
        let mut guarded: BTreeSet<&str> = from_tables
            .iter()
            .copied()
            .chain(defaults.iter().map(String::as_str))
            .filter(|lint| !skipped.contains(lint))
            .collect();
        if guarded.is_empty() {
            return Err("the list of lints to force-warn came out empty: neither the lint tables nor `rustc -W help` named a lint that warns or denies, so the audit would check nothing".to_owned());
        }
        let mut lints = Vec::new();
        if guarded.remove(UNKNOWN_LINTS) {
            lints.push(UNKNOWN_LINTS.to_owned());
        }
        lints.extend(guarded.iter().map(|lint| (*lint).to_owned()));
        let tables_count = lints
            .iter()
            .filter(|lint| from_tables.contains(lint.as_str()))
            .count();
        Ok(Self {
            from_compiler: lints.len() - tables_count,
            from_tables: tables_count,
            lints,
            edition,
            allowed,
        })
    }

    /// Describes the list for a person, in one line.
    pub fn summary(&self) -> String {
        let skipped: Vec<&str> = UNGUARDED.iter().map(|unguarded| unguarded.lint).collect();
        format!(
            "lint audit: {} lints and groups to force-warn, {} that the lint tables set to warn or deny and {} more that rustc turns on by default for edition {}; not force-warned, because the compiler refuses them: {}",
            self.lints.len(),
            self.from_tables,
            self.from_compiler,
            self.edition,
            skipped.join(", ")
        )
    }

    /// The arguments that force-warn every guarded lint, as they follow `--` in a Clippy command.
    pub fn force_warn_args(&self) -> Vec<String> {
        self.lints
            .iter()
            .flat_map(|lint| ["--force-warn".to_owned(), lint.clone()])
            .collect()
    }
}

/// The lint levels of the tables `[<prefix>.rust]`, `[<prefix>.clippy]` and `[<prefix>.rustdoc]` in a manifest, keyed by the name the compiler gives the lint: `unsafe_code`, `clippy::all`, `rustdoc::broken_intra_doc_links`. A level is written as a string (`"warn"`) or as the `level` of an inline table (`{ level = "warn", priority = -1 }`); anything else is an error.
pub fn lint_table(text: &str, prefix: &str) -> Result<BTreeMap<String, String>, String> {
    let mut levels = BTreeMap::new();
    for tool in LINT_TOOLS {
        for entry in toml_subset::table(text, &format!("{prefix}.{tool}"))?.unwrap_or_default() {
            let level = if entry.value.starts_with('{') {
                toml_subset::field_of_each(&format!("[{}]", entry.value), "level")?
                    .pop()
                    .unwrap_or_default()
            } else {
                toml_subset::string(&entry.value)?
            };
            if !matches!(level.as_str(), "allow" | "warn" | "deny" | "forbid") {
                return Err(format!(
                    "line {}: `{}` has the level `{level}`, which is not allow, warn, deny or forbid",
                    entry.line, entry.key
                ));
            }
            let key = entry.key.replace('-', "_");
            let lint = if tool == "rust" {
                key
            } else {
                format!("{tool}::{key}")
            };
            if levels.insert(lint, level).is_some() {
                return Err(format!(
                    "line {}: `{}` appears twice",
                    entry.line, entry.key
                ));
            }
        }
    }
    Ok(levels)
}

/// The lints that a lint table sets to `allow`.
fn allowed_in(table: &BTreeMap<String, String>) -> BTreeSet<String> {
    table
        .iter()
        .filter(|(_, level)| level.as_str() == "allow")
        .map(|(lint, _)| lint.clone())
        .collect()
}

/// The lints that the compiler turns on by default, `warn` or `deny`, from what `rustc --edition <edition> -W help` prints: the table under "Lint checks provided by rustc:", one lint per line with its name (written with `-` for `_`), its default level and its meaning, between a line of dashes and an empty line. Anything else in the table, or a table without a lint, is an error.
pub fn compiler_defaults(help: &str) -> Result<Vec<String>, String> {
    let mut lines = help.lines();
    lines
        .find(|line| line.trim() == "Lint checks provided by rustc:")
        .ok_or(
            "it printed no table of the compiler's lints (\"Lint checks provided by rustc:\")",
        )?;
    let header = lines
        .find(|line| !line.trim().is_empty())
        .ok_or("the table of lints has no header")?;
    if header.split_whitespace().collect::<Vec<_>>() != ["name", "default", "meaning"] {
        return Err(format!(
            "the table of lints has an unexpected header: `{}`",
            header.trim()
        ));
    }
    let dashes = lines.next().unwrap_or_default();
    if !dashes
        .split_whitespace()
        .all(|word| word.chars().all(|c| c == '-'))
        || dashes.trim().is_empty()
    {
        return Err(format!(
            "the table of lints has no line of dashes under its header, but `{}`",
            dashes.trim()
        ));
    }
    let mut defaults = Vec::new();
    let mut rows = 0_usize;
    for line in lines.take_while(|line| !line.trim().is_empty()) {
        let mut words = line.split_whitespace();
        let (Some(name), Some(level)) = (words.next(), words.next()) else {
            return Err(format!(
                "cannot read the line `{}` of the table of lints",
                line.trim()
            ));
        };
        match level {
            "warn" | "deny" => defaults.push(name.replace('-', "_")),
            "allow" | "forbid" => {}
            other => {
                return Err(format!(
                    "the lint `{name}` has the default level `{other}`, which is not allow, warn, deny or forbid"
                ));
            }
        }
        rows += 1;
    }
    if rows == 0 {
        return Err("the table of the compiler's lints lists no lint".to_owned());
    }
    Ok(defaults)
}

/// Checks the reasons of the skip list against the pinned compiler, by compiling an empty crate with rustc in `root`: force-warning `warnings` must fail with error E0602, and force-warning the other listed lints, together with every guarded lint that is not a tool's (such as Clippy's), must report exactly the listed ones as unknown. Returns one line describing what was checked.
pub fn check_refusals(root: &Path, guarded: &Guarded) -> Result<String, String> {
    let folder = root.join("target").join("lint-audit");
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    let probe = |lints: &[&str]| -> Result<(bool, String), String> {
        let mut rustc = Command::new("rustc");
        rustc
            .args(["--edition", &guarded.edition])
            .args(["--crate-type", "lib", "--crate-name", "lint_audit_probe"])
            .args(["--emit", "metadata", "--error-format", "json", "-o"])
            .arg(folder.join("liblint_audit_probe.rmeta"));
        for lint in lints {
            rustc.args(["--force-warn", lint]);
        }
        // An empty crate, read from standard input, which xtask leaves empty.
        rustc.arg("-").current_dir(root);
        let output = process::capture(&mut rustc)?;
        Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    };
    let unknown_listed: Vec<&str> = UNGUARDED
        .iter()
        .filter(|unguarded| unguarded.refusal == Refusal::Unknown)
        .map(|unguarded| unguarded.lint)
        .collect();
    let mut asked: Vec<&str> = vec![UNKNOWN_LINTS];
    asked.extend(
        guarded
            .lints
            .iter()
            .map(String::as_str)
            .filter(|lint| *lint != UNKNOWN_LINTS && !lint.contains("::")),
    );
    let own = asked.len();
    asked.extend(&unknown_listed);
    let (success, output) = probe(&asked)?;
    if !success {
        return Err(format!(
            "rustc failed to compile an empty crate with the guarded lints force-warned:\n{output}"
        ));
    }
    let reported = unknown_lints(&output)?;
    let mut still = Vec::new();
    for unguarded in &UNGUARDED {
        let refused = match unguarded.refusal {
            Refusal::NotSupported => {
                let (success, output) = probe(&[unguarded.lint])?;
                !success && refuses(&output, unguarded.lint)?
            }
            Refusal::Unknown => reported.contains(unguarded.lint),
        };
        still.push((unguarded, refused));
    }
    let unexpected: BTreeSet<String> = reported
        .into_iter()
        .filter(|lint| !unknown_listed.contains(&lint.as_str()))
        .collect();
    judge_refusals(&still, &unexpected, own)
}

/// Judges the probes of [`check_refusals`]: `still` holds each lint of the skip list and whether the compiler still refuses it for the reason the list gives, `unexpected` the force-warned lints that the compiler calls unknown although the list does not hold them, and `guarded` how many lints of the compiler's own were force-warned.
pub fn judge_refusals(
    still: &[(&Unguarded, bool)],
    unexpected: &BTreeSet<String>,
    guarded: usize,
) -> Result<String, String> {
    let mut problems = Vec::new();
    for (unguarded, refused) in still {
        if !refused {
            problems.push(format!(
                "  force-warning `{}` no longer fails as UNGUARDED says ({}: {}); remove it from the list, so that the audit force-warns it",
                unguarded.lint,
                match unguarded.refusal {
                    Refusal::NotSupported => "error E0602",
                    Refusal::Unknown => "an unknown lint",
                },
                unguarded.reason
            ));
        }
    }
    for lint in unexpected {
        problems.push(format!(
            "  the compiler calls the force-warned lint `{lint}` unknown, and UNGUARDED does not list it"
        ));
    }
    if problems.is_empty() {
        Ok(format!(
            "lint audit: the pinned compiler knows all {guarded} of its own lints that the audit force-warns, and still refuses the {} that UNGUARDED lists, for the reasons it gives",
            still.len()
        ))
    } else {
        Err(format!(
            "the skip list of the lint audit (UNGUARDED in xtask/src/lint_audit/guarded.rs) does not match the pinned compiler:\n{}\nThe audit force-warns every lint that a lint table sets to warn or deny and every lint that the compiler turns on by default, except those that the compiler refuses to force-warn, which UNGUARDED lists with the reason. Update the list to what the compiler says, in a reviewed pull request; never skip a lint that the compiler can force-warn.",
            problems.join("\n")
        ))
    }
}

/// Whether the compiler's JSON diagnostics `output` hold error E0602 for `lint`.
fn refuses(output: &str, lint: &str) -> Result<bool, String> {
    for line in output.lines().filter(|line| line.starts_with('{')) {
        let diagnostic = json::parse(line)?;
        let code = diagnostic
            .get("code")
            .and_then(|code| code.get("code"))
            .and_then(json::Value::as_str);
        let message = diagnostic
            .get("message")
            .and_then(json::Value::as_str)
            .unwrap_or_default();
        if code == Some("E0602") && message.contains(&format!("`{lint}`")) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The lints that the compiler's JSON diagnostics `output` call unknown: the names in its `unknown lint: `…`` warnings.
fn unknown_lints(output: &str) -> Result<BTreeSet<String>, String> {
    let mut found = BTreeSet::new();
    for line in output.lines().filter(|line| line.starts_with('{')) {
        let diagnostic = json::parse(line)?;
        if let Some(name) = unknown_lint(&diagnostic) {
            found.insert(name);
        }
    }
    Ok(found)
}

/// The lint that a diagnostic calls unknown, if it is one of the compiler's `unknown lint: `…`` warnings.
pub fn unknown_lint(diagnostic: &json::Value) -> Option<String> {
    let code = diagnostic.get("code")?.get("code")?.as_str()?;
    let message = diagnostic.get("message")?.as_str()?;
    let name = message.strip_prefix("unknown lint: `")?.strip_suffix('`')?;
    (code == UNKNOWN_LINTS).then(|| name.replace('-', "_"))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `rustc -W help` prints, shortened.
    const HELP: &str = "
Available lint options:
    -W <foo>           Warn about <foo>

Lint checks provided by rustc:

                              name  default  meaning
                              ----  -------  -------
                     missing-docs  allow    detects missing documentation for public members
                  unused-must-use  warn     unused result of a type flagged as `#[must_use]`
                    unknown-lints  warn     unrecognized lint attribute
                         warnings  warn     mass-change the level for lints which produce warnings
           tail-call-track-caller  warn     detects tail calls of functions marked with `#[track_caller]`
                  static-mut-refs  deny     creating a shared reference to mutable static


Lint groups provided by rustc:

                name  sub-lints
                ----  ---------
            warnings  all lints that are set to issue warnings
";

    #[test]
    fn reads_the_default_levels_of_the_compilers_lints() {
        assert_eq!(
            compiler_defaults(HELP).unwrap(),
            [
                "unused_must_use",
                "unknown_lints",
                "warnings",
                "tail_call_track_caller",
                "static_mut_refs"
            ]
        );
    }

    #[test]
    fn refuses_a_help_text_it_cannot_read() {
        for (help, problem) in [
            ("", "printed no table"),
            (
                "Lint checks provided by rustc:\n\n  name  level\n",
                "unexpected header",
            ),
            (
                "Lint checks provided by rustc:\n\n  name  default  meaning\n  ----  -------  -------\n\n",
                "lists no lint",
            ),
            (
                "Lint checks provided by rustc:\n\n  name  default  meaning\n  ----  -------  -------\n  a-lint  sometimes  never\n",
                "`sometimes`",
            ),
        ] {
            let error = compiler_defaults(help).unwrap_err();
            assert!(error.contains(problem), "{help:?}: {error}");
        }
    }

    #[test]
    fn reads_the_lint_levels_of_a_manifest() {
        let manifest = r#"
[workspace.lints.rust]
unsafe_code = "forbid"
missing-docs = "warn"
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(bayan_lint_canary, values(any()))"] }

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
manual_clamp = "allow"
"#;
        let levels = lint_table(manifest, "workspace.lints").unwrap();
        let expected: BTreeMap<String, String> = [
            ("clippy::all", "warn"),
            ("clippy::manual_clamp", "allow"),
            ("missing_docs", "warn"),
            ("unexpected_cfgs", "warn"),
            ("unsafe_code", "forbid"),
        ]
        .into_iter()
        .map(|(lint, level)| (lint.to_owned(), level.to_owned()))
        .collect();
        assert_eq!(levels, expected);
        assert!(lint_table("[lints.rust]\nx = \"sometimes\"\n", "lints").is_err());
        assert!(lint_table("[lints.rust]\nx = { priority = 1 }\n", "lints").is_err());
    }

    fn table(levels: &[(&str, &str)]) -> BTreeMap<String, String> {
        levels
            .iter()
            .map(|(lint, level)| ((*lint).to_owned(), (*level).to_owned()))
            .collect()
    }

    #[test]
    fn force_warns_the_tables_and_the_compilers_defaults_but_not_the_skip_list() {
        let tables = [
            (
                "root".to_owned(),
                table(&[
                    ("unsafe_code", "forbid"),
                    ("clippy::all", "warn"),
                    ("clippy::manual_clamp", "allow"),
                    ("clippy::disallowed_methods", "forbid"),
                    ("missing_docs", "warn"),
                ]),
            ),
            ("binding".to_owned(), table(&[("unsafe_code", "deny")])),
        ];
        let defaults = compiler_defaults(HELP).unwrap();
        let guarded = Guarded::new(&tables, &defaults, "2024".to_owned(), BTreeMap::new()).unwrap();
        assert_eq!(
            guarded.lints,
            [
                "unknown_lints",
                "clippy::all",
                "missing_docs",
                "static_mut_refs",
                "unsafe_code",
                "unused_must_use"
            ]
        );
        assert_eq!((guarded.from_tables, guarded.from_compiler), (3, 3));
        assert_eq!(
            guarded.force_warn_args()[..4],
            [
                "--force-warn",
                "unknown_lints",
                "--force-warn",
                "clippy::all"
            ]
        );
    }

    #[test]
    fn fails_when_nothing_is_left_to_force_warn() {
        let tables = [("root".to_owned(), table(&[("unsafe_code", "forbid")]))];
        let error = Guarded::new(
            &tables,
            &["warnings".to_owned()],
            "2024".to_owned(),
            BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.contains("came out empty"), "{error}");
    }

    #[test]
    fn fails_when_the_skip_list_and_the_compiler_disagree() {
        let warnings = &UNGUARDED[0];
        let unknown = &UNGUARDED[1];
        assert!(judge_refusals(&[(warnings, true), (unknown, true)], &BTreeSet::new(), 3).is_ok());
        // The compiler calls a force-warned lint unknown that the list does not hold.
        let unexpected = BTreeSet::from(["new_lint".to_owned()]);
        let error = judge_refusals(&[(warnings, true)], &unexpected, 3).unwrap_err();
        assert!(error.contains("`new_lint` unknown"), "{error}");
        // A listed lint no longer fails for the reason the list gives.
        for (unguarded, reason) in [(warnings, "error E0602"), (unknown, "an unknown lint")] {
            let error = judge_refusals(&[(unguarded, false)], &BTreeSet::new(), 3).unwrap_err();
            assert!(
                error.contains(&format!("`{}` no longer fails", unguarded.lint))
                    && error.contains(reason),
                "{error}"
            );
        }
    }

    #[test]
    fn recognizes_the_compilers_unknown_lint_warnings() {
        let warning = json::parse(r#"{"$message_type":"diagnostic","message":"unknown lint: `tail_call_track_caller`","code":{"code":"unknown_lints","explanation":null},"level":"warning"}"#).unwrap();
        assert_eq!(
            unknown_lint(&warning).as_deref(),
            Some("tail_call_track_caller")
        );
        let other = json::parse(r#"{"message":"unused variable: `x`","code":{"code":"unused_variables","explanation":null},"level":"warning"}"#).unwrap();
        assert_eq!(unknown_lint(&other), None);
        let output = format!(
            "{}\nnot JSON\n{}\n",
            r#"{"message":"`warnings` lint group is not supported with ´--force-warn´","code":{"code":"E0602","explanation":"…"},"level":"error"}"#,
            r#"{"message":"unknown lint: `duplicate_tools`","code":{"code":"unknown_lints","explanation":null},"level":"warning"}"#
        );
        assert!(refuses(&output, "warnings").unwrap());
        assert!(!refuses(&output, "unused_must_use").unwrap());
        assert_eq!(
            unknown_lints(&output).unwrap(),
            BTreeSet::from(["duplicate_tools".to_owned()])
        );
    }

    /// The real list: every lint that a lint table of the workspace sets to warn or deny, and every lint that the pinned compiler turns on by default for the workspace's edition and the skip list does not hold, is force-warned (acceptance criterion AC-4 of CORE-009).
    #[test]
    fn force_warns_every_lint_that_the_lint_tables_or_the_pinned_compiler_turn_on() {
        let root = crate::workspace_root();
        let metadata = policy::metadata(&root).unwrap();
        let packages = policy::packages(&metadata).unwrap();
        let guarded = Guarded::load(&root, &packages).unwrap();
        let args = guarded.force_warn_args();
        let force_warned: BTreeSet<&str> = args
            .chunks(2)
            .map(|pair| {
                assert_eq!(pair[0], "--force-warn");
                pair[1].as_str()
            })
            .collect();
        let skipped: Vec<&str> = UNGUARDED.iter().map(|unguarded| unguarded.lint).collect();
        let mut expected = Vec::new();
        for (manifest, prefix) in [
            ("Cargo.toml", "workspace.lints"),
            ("crates/bayan-ffi/Cargo.toml", "lints"),
            ("crates/bayan-wasm/Cargo.toml", "lints"),
        ] {
            let text = std::fs::read_to_string(root.join(manifest)).unwrap();
            for (lint, level) in lint_table(&text, prefix).unwrap() {
                if level == "warn" || level == "deny" {
                    expected.push(lint);
                }
            }
        }
        let output = Command::new("rustc")
            .args(["--edition", "2024", "-W", "help"])
            .current_dir(&root)
            .output()
            .unwrap();
        let defaults = compiler_defaults(&String::from_utf8_lossy(&output.stdout)).unwrap();
        assert!(
            defaults.len() > 100,
            "only {} default lints",
            defaults.len()
        );
        expected.extend(
            defaults
                .into_iter()
                .filter(|lint| !skipped.contains(&lint.as_str())),
        );
        for lint in &expected {
            assert!(
                force_warned.contains(lint.as_str()),
                "`{lint}` is not force-warned"
            );
        }
        // The hash-map ban is guarded through `clippy::all`, the group that holds it.
        for lint in [
            "clippy::all",
            "unused_must_use",
            "deprecated",
            "unsafe_code",
        ] {
            assert!(force_warned.contains(lint), "`{lint}` is not force-warned");
        }
        assert!(
            !force_warned.contains("clippy::disallowed_methods"),
            "a forbidden lint needs nothing"
        );
        assert_eq!(guarded.edition, "2024");
        assert!(guarded.allowed["bayan-units"].contains("clippy::manual_clamp"));
    }
}
