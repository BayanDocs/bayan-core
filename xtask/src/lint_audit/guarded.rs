//! The lints that the audit force-warns: every lint and group that a lint table of the workspace sets to `warn` or `deny`, and every lint that the pinned compiler turns on by default for the workspace's edition, except those that force-warning cannot name (the skip list, [`UNGUARDED`]).
//!
//! Both come from where they are decided, never from a list kept by hand: the lint tables from the manifests (`[workspace.lints]` in the root `Cargo.toml` and the copies in the binding crates' manifests), read only in their plain form ([`lint_table`] fails on every other form of TOML, which Cargo reads alike), and the compiler's defaults from `rustc --edition <edition> -W help`, run with the pinned toolchain, as are the lint groups of rustc, Clippy and rustdoc, which decide which `allow` entries switch a lint off ([`allowed_in`]), so that a lint added to a table, or a default lint that a new toolchain brings, is guarded from the same commit. The skip list cannot go stale: [`check_refusals`] fails when the compiler calls a force-warned lint unknown that the list does not hold, and when force-warning a listed lint no longer fails for the reason the list gives.

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

/// The tools whose lint groups `rustc -W help` does not list, with the command that lists them under "Lint groups loaded by this crate:".
const TOOL_HELP: [(&str, &str, &[&str]); 2] = [
    ("clippy", "clippy-driver", &["rustc", "-W", "help"]),
    ("rustdoc", "rustdoc", &["-W", "help"]),
];

/// The keys that an entry of a lint table may set in its inline table: the ones the audit reads, and `check-cfg`, which only Cargo reads (for `unexpected_cfgs`).
const ENTRY_KEYS: [&str; 3] = ["check-cfg", "level", "priority"];

/// One entry of a lint table: the level it sets, and its priority, by which Cargo orders the table's entries when it passes them to the compiler, lowest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// `allow`, `warn`, `deny` or `forbid`.
    pub level: String,
    /// The entry's `priority`, 0 if it has none.
    pub priority: i64,
}

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
    /// For each workspace member, the lints that its lint table switches off ([`allowed_in`]). A group such as `clippy::all` also force-warns these members of it, whose occurrences the audit ignores.
    pub allowed: BTreeMap<String, BTreeSet<String>>,
}

impl Guarded {
    /// Reads the lint tables of the root manifest and of the binding crates among `packages`, the default levels and the groups of the compiler's lints from `rustc --edition <edition> -W help`, and Clippy's and rustdoc's groups from their own `-W help` ([`TOOL_HELP`]), each run in `root` so that the pinned toolchain answers.
    pub fn load(root: &Path, packages: &[Package]) -> Result<Self, String> {
        let manifest = read(&root.join("Cargo.toml"))?;
        let edition = toml_subset::value(&manifest, "workspace.package", "edition")
            .and_then(|edition| {
                edition.ok_or_else(|| "no `edition` in [workspace.package]".to_owned())
            })
            .and_then(|edition| toml_subset::string(&edition))
            .map_err(|problem| format!("the root Cargo.toml: {problem}"))?;
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
        let help = String::from_utf8_lossy(&output.stdout);
        let defaults = compiler_defaults(&help)
            .map_err(|problem| format!("`{}`: {problem}", process::display(&rustc)))?;
        let mut groups: BTreeSet<String> = compiler_groups(&help)
            .map_err(|problem| format!("`{}`: {problem}", process::display(&rustc)))?
            .into_iter()
            .collect();
        for (tool, program, args) in TOOL_HELP {
            let mut command = Command::new(program);
            command.args(args).current_dir(root);
            let output = process::capture(&mut command)?;
            if !output.status.success() {
                return Err(format!(
                    "`{}` failed:\n{}",
                    process::display(&command),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            groups.extend(
                tool_groups(&String::from_utf8_lossy(&output.stdout), tool)
                    .map_err(|problem| format!("`{}`: {problem}", process::display(&command)))?,
            );
        }
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
                allowed.insert(package.name.clone(), allowed_in(&table, &groups));
                tables.push((format!("the manifest of {}", package.name), table));
            } else {
                allowed.insert(package.name.clone(), allowed_in(&tables[0].1, &groups));
            }
        }
        Self::new(&tables, &defaults, edition, allowed)
    }

    /// The guarded lints from the lint tables `tables` (each with where it comes from) and the lints that the compiler turns on by default, `defaults`. Fails if the list comes out empty.
    pub fn new(
        tables: &[(String, BTreeMap<String, Setting>)],
        defaults: &[String],
        edition: String,
        allowed: BTreeMap<String, BTreeSet<String>>,
    ) -> Result<Self, String> {
        let from_tables: BTreeSet<&str> = tables
            .iter()
            .flat_map(|(_, table)| table.iter())
            .filter(|(_, setting)| matches!(setting.level.as_str(), "warn" | "deny"))
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

/// The entries of the tables `[<prefix>.rust]`, `[<prefix>.clippy]` and `[<prefix>.rustdoc]` in a manifest, keyed by the name the compiler gives the lint: `unsafe_code`, `clippy::all`, `rustdoc::broken_intra_doc_links`. A level is written as a string (`"warn"`), at priority 0, or as the `level` of an inline table, with its `priority` (`{ level = "warn", priority = -1 }`); anything else is an error.
pub fn lint_table(text: &str, prefix: &str) -> Result<BTreeMap<String, Setting>, String> {
    check_forms(text, prefix)?;
    let mut levels = BTreeMap::new();
    for tool in LINT_TOOLS {
        for entry in toml_subset::table(text, &format!("{prefix}.{tool}"))?.unwrap_or_default() {
            let (level, priority) = if entry.value.starts_with('{') {
                let mut level = String::new();
                let mut priority = 0;
                for (key, value) in toml_subset::inline_table(&entry.value)? {
                    match key.as_str() {
                        "level" => level = toml_subset::string(&value)?,
                        "priority" => {
                            priority = value.parse().map_err(|_| {
                                format!(
                                    "line {}: `{}` has the priority `{value}`, which is not a whole number",
                                    entry.line, entry.key
                                )
                            })?;
                        }
                        "check-cfg" => {}
                        other => {
                            return Err(format!(
                                "line {}: `{}` sets `{other}`, which the lint audit does not read; an entry's inline table may set only {}, written without quotes. {}",
                                entry.line,
                                entry.key,
                                ENTRY_KEYS.map(|key| format!("`{key}`")).join(", "),
                                plain_form(prefix)
                            ));
                        }
                    }
                }
                (level, priority)
            } else {
                (toml_subset::string(&entry.value)?, 0)
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
            if levels.insert(lint, Setting { level, priority }).is_some() {
                return Err(format!(
                    "line {}: `{}` appears twice",
                    entry.line, entry.key
                ));
            }
        }
    }
    Ok(levels)
}

/// How to write a lint table's entries so that the lint audit reads them, for the tables `[<prefix>.<tool>]`.
fn plain_form(prefix: &str) -> String {
    format!(
        "Write each entry as `name = \"level\"` or `name = {{ level = \"…\", priority = … }}` directly in `[{prefix}.rust]`, `[{prefix}.clippy]` or `[{prefix}.rustdoc]`, the one form that the lint audit reads (the tables are `[workspace.lints.<tool>]` in the root Cargo.toml and `[lints.<tool>]` in a binding crate's manifest); Cargo reads the other forms of TOML too, so the audit would not see what they set."
    )
}

/// Fails on every way of writing an entry of the lint tables `[<prefix>.<tool>]` that [`lint_table`] does not read, although Cargo reads them all alike: a table below a tool's table (`[<prefix>.clippy.all]`), an entry of `[<prefix>]` itself, a dotted key that reaches the lint tables from another table (`clippy.all = …` under `[<prefix>]`, or `lints.clippy.all = …` under `[workspace]`), a quoted or dotted key, a table written with quotes or as an array of tables, and the table of a tool other than rust, clippy and rustdoc. The message says how to write the entry in the one form that the audit reads.
fn check_forms(text: &str, prefix: &str) -> Result<(), String> {
    let lints = toml_subset::key_path(prefix)?;
    for section in toml_subset::sections(text)? {
        let header = if section.header.is_empty() {
            Vec::new()
        } else {
            toml_subset::key_path(&section.header)
                .map_err(|problem| format!("line {}: {problem}", section.line))?
        };
        let below = header.strip_prefix(lints.as_slice());
        let plain = match below {
            Some([tool]) => {
                LINT_TOOLS.contains(&tool.as_str())
                    && !section.array
                    && section.header == format!("{prefix}.{tool}")
            }
            _ => false,
        };
        if let Some(rest) = below
            && !plain
            && (!rest.is_empty() || section.array || !section.entries.is_empty())
        {
            let form = match rest {
                [] => "holds entries of its own".to_owned(),
                [tool] if !LINT_TOOLS.contains(&tool.as_str()) => format!(
                    "is the lint table of `{tool}`, which is not one of {}",
                    LINT_TOOLS.join(", ")
                ),
                [_] => "is written with quotes or as an array of tables".to_owned(),
                _ => "writes a lint table entry as a table of its own".to_owned(),
            };
            return Err(format!(
                "line {}: `[{}]` {form}, which the lint audit does not read. {}",
                section.line,
                section.header,
                plain_form(prefix)
            ));
        }
        for entry in &section.entries {
            let key = toml_subset::key_path(&entry.key)
                .map_err(|problem| format!("line {}: {problem}", entry.line))?;
            let full: Vec<String> = header.iter().chain(&key).cloned().collect();
            if !full.starts_with(&lints) {
                continue;
            }
            let bare = key.len() == 1 && entry.key == key[0];
            if !(plain && bare) {
                let place = if section.header.is_empty() {
                    "before the first table".to_owned()
                } else {
                    format!("in `[{}]`", section.header)
                };
                let form = if plain {
                    "a quoted or dotted key"
                } else {
                    "a key that reaches the lint tables from another table"
                };
                return Err(format!(
                    "line {}: `{}` {place} is {form}, which the lint audit does not read. {}",
                    entry.line,
                    entry.key,
                    plain_form(prefix)
                ));
            }
        }
    }
    Ok(())
}

/// The lints that a lint table switches off: those it sets to `allow` at a higher priority than every group of `groups` that it sets to `warn`, `deny` or `forbid`, of any tool. Cargo passes a table's entries to the compiler in the order of their priority, and of the flags that name a lint or a group that holds it, the last one wins; among entries of the same priority, the order is Cargo's own. So an `allow` at the same or a lower priority than such a group can leave the lint on, and the audit then judges its occurrences like those of any other guarded lint. Every group counts, whichever lints it holds, because the audit does not model that (`warnings` holds them all).
fn allowed_in(table: &BTreeMap<String, Setting>, groups: &BTreeSet<String>) -> BTreeSet<String> {
    let highest_group = table
        .iter()
        .filter(|(name, setting)| groups.contains(*name) && setting.level != "allow")
        .map(|(_, setting)| setting.priority)
        .max();
    table
        .iter()
        .filter(|(_, setting)| {
            setting.level == "allow"
                && highest_group.is_none_or(|priority| setting.priority > priority)
        })
        .map(|(lint, _)| lint.clone())
        .collect()
}

/// The rows of the table under the line `title` in what `rustc -W help` prints, which has the columns `columns`: the lines between the line of dashes under the header and the next empty line. A missing table, header or line of dashes is an error; `what` names the table.
fn help_table<'a>(
    help: &'a str,
    title: &str,
    columns: &[&str],
    what: &str,
) -> Result<Vec<&'a str>, String> {
    let titles = help.lines().filter(|line| line.trim() == title).count();
    if titles > 1 {
        return Err(format!(
            "it printed the table of {what} (\"{title}\") {titles} times"
        ));
    }
    let mut lines = help.lines();
    lines
        .find(|line| line.trim() == title)
        .ok_or_else(|| format!("it printed no table of {what} (\"{title}\")"))?;
    let header = lines
        .find(|line| !line.trim().is_empty())
        .ok_or_else(|| format!("the table of {what} has no header"))?;
    if header.split_whitespace().collect::<Vec<_>>() != columns {
        return Err(format!(
            "the table of {what} has an unexpected header: `{}`",
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
            "the table of {what} has no line of dashes under its header, but `{}`",
            dashes.trim()
        ));
    }
    Ok(lines.take_while(|line| !line.trim().is_empty()).collect())
}

/// The lints that the compiler turns on by default, `warn` or `deny`, from what `rustc --edition <edition> -W help` prints: the table under "Lint checks provided by rustc:", one lint per line with its name (written with `-` for `_`), its default level and its meaning, between a line of dashes and an empty line. Anything else in the table, or a table without a lint, is an error.
pub fn compiler_defaults(help: &str) -> Result<Vec<String>, String> {
    let lines = help_table(
        help,
        "Lint checks provided by rustc:",
        &["name", "default", "meaning"],
        "lints",
    )?;
    let mut defaults = Vec::new();
    let mut rows = 0_usize;
    for line in lines {
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

/// The names in a table of lint groups of `-W help`, under `title`: one group per line, with its name (written with `-` for `_`) and the lints it holds. A line without both, or a table without a group, is an error.
fn group_names(help: &str, title: &str) -> Result<Vec<String>, String> {
    let mut groups = Vec::new();
    for line in help_table(help, title, &["name", "sub-lints"], "lint groups")? {
        let mut words = line.split_whitespace();
        let (Some(name), Some(_)) = (words.next(), words.next()) else {
            return Err(format!(
                "cannot read the line `{}` of the table of lint groups",
                line.trim()
            ));
        };
        groups.push(name.replace('-', "_"));
    }
    if groups.is_empty() {
        return Err(format!(
            "the table of lint groups (\"{title}\") lists no group"
        ));
    }
    Ok(groups)
}

/// The compiler's lint groups, from what `rustc -W help` prints: the table under "Lint groups provided by rustc:". A name of a tool's group (with `::`) is an error.
pub fn compiler_groups(help: &str) -> Result<Vec<String>, String> {
    let groups = group_names(help, "Lint groups provided by rustc:")?;
    if let Some(group) = groups.iter().find(|group| group.contains("::")) {
        return Err(format!(
            "the table of the compiler's lint groups lists `{group}`, a tool's group"
        ));
    }
    Ok(groups)
}

/// The lint groups of the tool `tool`, `clippy` or `rustdoc`, from what its `-W help` prints: the table under "Lint groups loaded by this crate:". A name that does not start with `<tool>::` is an error.
pub fn tool_groups(help: &str, tool: &str) -> Result<Vec<String>, String> {
    let groups = group_names(help, "Lint groups loaded by this crate:")?;
    let own = format!("{tool}::");
    if let Some(group) = groups.iter().find(|group| !group.starts_with(&own)) {
        return Err(format!(
            "the table of {tool}'s lint groups lists `{group}`, which is not one of {tool}'s"
        ));
    }
    Ok(groups)
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

/// The compiler's JSON diagnostics in `output`, what it printed with `--error-format json`: every line that is not empty must be one.
fn diagnostics(output: &str) -> Result<Vec<json::Value>, String> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            json::parse(line).map_err(|error| {
                format!("the compiler printed a line that is not one of its JSON diagnostics ({error}): {line}")
            })
        })
        .collect()
}

/// Whether the compiler's JSON diagnostics `output` hold error E0602 for `lint`.
fn refuses(output: &str, lint: &str) -> Result<bool, String> {
    for diagnostic in diagnostics(output)? {
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
    for diagnostic in diagnostics(output)? {
        if let Some(name) = unknown_lint(&diagnostic)? {
            found.insert(name);
        }
    }
    Ok(found)
}

/// The lint that a diagnostic calls unknown, if it is one of the compiler's warnings of the lint `unknown_lints`, `unknown lint: `…``; such a warning in any other words is an error, because the audit could not tell which lint the compiler does not know.
pub fn unknown_lint(diagnostic: &json::Value) -> Result<Option<String>, String> {
    let code = diagnostic
        .get("code")
        .and_then(|code| code.get("code"))
        .and_then(json::Value::as_str);
    if code != Some(UNKNOWN_LINTS) {
        return Ok(None);
    }
    let message = diagnostic
        .get("message")
        .and_then(json::Value::as_str)
        .unwrap_or_default();
    message
        .strip_prefix("unknown lint: `")
        .and_then(|rest| rest.strip_suffix('`'))
        .map(|name| Some(name.replace('-', "_")))
        .ok_or_else(|| {
            format!("a warning of `{UNKNOWN_LINTS}` that the audit cannot read: {message}")
        })
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
   nonstandard-style  non-camel-case-types, non-snake-case, non-upper-case-globals
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
            (&format!("{HELP}{HELP}"), "2 times"),
        ] {
            let error = compiler_defaults(help).unwrap_err();
            assert!(error.contains(problem), "{help:?}: {error}");
        }
        let groups = "Lint groups provided by rustc:\n\n  name  sub-lints\n  ----  ---------\n";
        for (help, problem) in [
            (
                format!("{groups}  unused\n"),
                "cannot read the line `unused`",
            ),
            (
                format!("{groups}  clippy::all  clippy::x\n"),
                "a tool's group",
            ),
            (format!("{groups}\n"), "lists no group"),
        ] {
            let error = compiler_groups(&help).unwrap_err();
            assert!(error.contains(problem), "{help:?}: {error}");
        }
    }

    /// What `clippy-driver rustc -W help` prints under its own title, shortened.
    const CLIPPY_HELP: &str = "
Lint groups loaded by this crate:

                    name  sub-lints
                    ----  ---------
             clippy::all  clippy::almost-complete-range, clippy::approx-constant
        clippy::pedantic  clippy::bool-to-int-with-if
";

    #[test]
    fn reads_the_lint_groups_of_clippy_and_rustdoc() {
        assert_eq!(
            tool_groups(CLIPPY_HELP, "clippy").unwrap(),
            ["clippy::all", "clippy::pedantic"]
        );
        assert!(
            tool_groups(CLIPPY_HELP, "rustdoc")
                .unwrap_err()
                .contains("not one of rustdoc's")
        );
        assert!(
            tool_groups(HELP, "clippy")
                .unwrap_err()
                .contains("no table of lint groups")
        );
        // The pinned toolchain's own: Clippy's ten groups, and rustdoc's one.
        let root = crate::workspace_root();
        for (tool, program, args) in TOOL_HELP {
            let output = Command::new(program)
                .args(args)
                .current_dir(&root)
                .output()
                .unwrap();
            let groups = tool_groups(&String::from_utf8_lossy(&output.stdout), tool).unwrap();
            assert!(groups.contains(&format!("{tool}::all")), "{groups:?}");
            if tool == "clippy" {
                assert_eq!(groups.len(), 10, "{groups:?}");
            }
        }
    }

    /// Required by the third review of CORE-009: every way of writing a lint table entry that Cargo reads but `lint_table` does not fails, and says how to write it.
    #[test]
    fn fails_on_every_form_of_a_lint_table_entry_that_it_does_not_read() {
        for (manifest, prefix, form) in [
            (
                "[workspace.lints.clippy.all]\nlevel = \"warn\"\npriority = -1\n",
                "workspace.lints",
                "line 1: `[workspace.lints.clippy.all]` writes a lint table entry as a table of its own",
            ),
            (
                "[workspace.lints]\nclippy.all = { level = \"warn\", priority = -1 }\n",
                "workspace.lints",
                "line 1: `[workspace.lints]` holds entries of its own",
            ),
            (
                "[workspace]\nmembers = []\nlints.clippy.all = { level = \"warn\" }\n",
                "workspace.lints",
                "line 3: `lints.clippy.all` in `[workspace]` is a key that reaches the lint tables from another table",
            ),
            (
                "workspace.lints.clippy.all = \"warn\"\n",
                "workspace.lints",
                "line 1: `workspace.lints.clippy.all` before the first table is a key that reaches",
            ),
            (
                "[lints]\nclippy = { all = \"warn\" }\n",
                "lints",
                "line 1: `[lints]` holds entries of its own",
            ),
            (
                "[workspace.lints.clippy]\n\"all\" = \"warn\"\n",
                "workspace.lints",
                "line 2: `\"all\"` in `[workspace.lints.clippy]` is a quoted or dotted key",
            ),
            (
                "[workspace.lints.clippy]\nall.level = \"warn\"\n",
                "workspace.lints",
                "line 2: `all.level` in `[workspace.lints.clippy]` is a quoted or dotted key",
            ),
            (
                "[workspace.lints.\"clippy\"]\nall = \"warn\"\n",
                "workspace.lints",
                "is written with quotes or as an array of tables",
            ),
            (
                "[[workspace.lints.clippy]]\nall = \"warn\"\n",
                "workspace.lints",
                "is written with quotes or as an array of tables",
            ),
            (
                "[workspace.lints.clipy]\nall = \"warn\"\n",
                "workspace.lints",
                "is the lint table of `clipy`, which is not one of rust, clippy, rustdoc",
            ),
            (
                "[workspace.lints.clippy]\nall = { level = \"warn\", \"priority\" = -1 }\n",
                "workspace.lints",
                "line 2: `all` sets `\"priority\"`, which the lint audit does not read",
            ),
            (
                "[lints.clippy]\nall = { level = \"warn\", priorty = -1 }\n",
                "lints",
                "line 2: `all` sets `priorty`",
            ),
        ] {
            let error = lint_table(manifest, prefix).unwrap_err();
            assert!(
                error.contains(form)
                    && error.contains(&format!(
                        "directly in `[{prefix}.rust]`, `[{prefix}.clippy]`"
                    )),
                "{manifest:?}: {error}"
            );
        }
        // Tables that do not reach the lint tables, and an empty `[workspace.lints]`, are left alone.
        let fine = "[workspace]\nmembers = []\n\n[workspace.lints]\n\n[workspace.lints.clippy]\nall = { level = \"warn\", priority = -1 }\n\n[package]\nlints = 1\n";
        assert_eq!(lint_table(fine, "workspace.lints").unwrap().len(), 1);
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
        let expected: BTreeMap<String, Setting> = [
            ("clippy::all", "warn", -1),
            ("clippy::manual_clamp", "allow", 0),
            ("missing_docs", "warn", 0),
            ("unexpected_cfgs", "warn", 0),
            ("unsafe_code", "forbid", 0),
        ]
        .into_iter()
        .map(|(lint, level, priority)| {
            (
                lint.to_owned(),
                Setting {
                    level: level.to_owned(),
                    priority,
                },
            )
        })
        .collect();
        assert_eq!(levels, expected);
        assert!(lint_table("[lints.rust]\nx = \"sometimes\"\n", "lints").is_err());
        assert!(lint_table("[lints.rust]\nx = { priority = 1 }\n", "lints").is_err());
        assert!(
            lint_table(
                "[lints.rust]\nx = { level = \"warn\", priority = \"high\" }\n",
                "lints"
            )
            .is_err()
        );
    }

    /// Required by the second review of CORE-009: Cargo passes `-A clippy::disallowed_types` before `-W clippy::all` when the `allow` has the lower priority, so the ban stays on and the audit must judge it.
    #[test]
    fn an_allow_switches_a_lint_off_only_above_every_group_that_turns_it_on() {
        let groups: BTreeSet<String> = compiler_groups(HELP)
            .unwrap()
            .into_iter()
            .chain(["unused".to_owned(), "clippy::all".to_owned()])
            .collect();
        // The review's table: below the group, and at its priority, where the order is Cargo's own, the lint stays on; above it, it is off.
        let review = r#"
[lints.clippy]
all = { level = "warn", priority = -1 }
disallowed_types = { level = "allow", priority = -2 }
unwrap_used = { level = "allow", priority = -1 }
manual_clamp = "allow"
"#;
        let allowed = allowed_in(&lint_table(review, "lints").unwrap(), &groups);
        assert_eq!(allowed, BTreeSet::from(["clippy::manual_clamp".to_owned()]));
        // A group of another tool counts too, because the audit does not model which lints a group holds.
        let across = r#"
[lints.rust]
unused = { level = "warn", priority = 1 }
dead_code = { level = "allow", priority = 1 }
unreachable_code = { level = "allow", priority = 2 }

[lints.clippy]
manual_clamp = "allow"
"#;
        let allowed = allowed_in(&lint_table(across, "lints").unwrap(), &groups);
        assert_eq!(allowed, BTreeSet::from(["unreachable_code".to_owned()]));
        // The workspace's own tables: `manual_clamp` at priority 0 sits above `clippy::all` at -1.
        let root = std::fs::read_to_string(crate::workspace_root().join("Cargo.toml")).unwrap();
        let workspace = lint_table(&root, "workspace.lints").unwrap();
        assert!(allowed_in(&workspace, &groups).contains("clippy::manual_clamp"));
    }

    #[test]
    fn reads_the_compilers_lint_groups() {
        assert_eq!(
            compiler_groups(HELP).unwrap(),
            ["warnings", "nonstandard_style"]
        );
        let without = HELP.replace("Lint groups provided by rustc:", "Lint groups:");
        assert!(
            compiler_groups(&without)
                .unwrap_err()
                .contains("no table of lint groups")
        );
        let empty = &HELP[..HELP.find("            warnings  all").unwrap()];
        assert!(
            compiler_groups(empty)
                .unwrap_err()
                .contains("lists no group")
        );
    }

    fn table(levels: &[(&str, &str)]) -> BTreeMap<String, Setting> {
        levels
            .iter()
            .map(|(lint, level)| {
                (
                    (*lint).to_owned(),
                    Setting {
                        level: (*level).to_owned(),
                        priority: 0,
                    },
                )
            })
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
            unknown_lint(&warning).unwrap().as_deref(),
            Some("tail_call_track_caller")
        );
        let other = json::parse(r#"{"message":"unused variable: `x`","code":{"code":"unused_variables","explanation":null},"level":"warning"}"#).unwrap();
        assert_eq!(unknown_lint(&other).unwrap(), None);
        let reworded = json::parse(r#"{"message":"lint `x` is unknown","code":{"code":"unknown_lints","explanation":null},"level":"warning"}"#).unwrap();
        assert!(unknown_lint(&reworded).unwrap_err().contains("cannot read"));
        assert!(
            unknown_lints("{}\nnot JSON\n")
                .unwrap_err()
                .contains("not one of its JSON diagnostics")
        );
        assert!(refuses("not JSON\n", "warnings").is_err());
        let output = format!(
            "{}\n\n{}\n",
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
            for (lint, setting) in lint_table(&text, prefix).unwrap() {
                if setting.level == "warn" || setting.level == "deny" {
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
