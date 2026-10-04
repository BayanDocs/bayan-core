//! Guardrail checks on the configuration itself. They make sure that Clippy reads only the root `clippy.toml`, that the lint rules really apply to every crate, that the binding crates' copies of them have not drifted, that the toolchain and `rust-version` agree, and that `clippy.toml` still forbids everything ADR-0005 requires. The lint canaries (`canary.rs`) then prove that the configuration has the intended effect.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{json, process, toml_subset};

/// The only crates that may contain unsafe code (ADR-0006 §2). Their manifests carry a copy of the workspace lint tables with `unsafe_code = "deny"`, because Cargo cannot override one inherited lint. Adding a crate here needs an ADR amendment.
pub const BINDING_CRATES: [&str; 2] = ["bayan-ffi", "bayan-wasm"];

/// The tool tables of `[workspace.lints]`.
const LINT_TOOLS: [&str; 3] = ["rust", "clippy", "rustdoc"];

/// The floating-point methods that `clippy.toml` must forbid for both `f32` and `f64`: the list in work package CORE-001, plus `sin_cos`, `asinh`, `acosh` and `atanh`, which ADR-0005 §4 forbids too ("platform transcendental methods"). Together they are every stable transcendental method in Rust 1.99.
pub const REQUIRED_FLOAT_METHODS: [&str; 26] = [
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sinh", "cosh", "tanh", "exp", "exp2",
    "exp_m1", "ln", "log", "log2", "log10", "ln_1p", "powf", "powi", "cbrt", "hypot", "sin_cos",
    "asinh", "acosh", "atanh",
];

/// The types that `clippy.toml` must forbid because their iteration order is random (ADR-0005 §5).
pub const REQUIRED_TYPES: [&str; 2] = ["std::collections::HashMap", "std::collections::HashSet"];

/// The file names Clippy reads its configuration from. They are compared without regard to case, because on Windows and macOS `Clippy.toml` is the same file as `clippy.toml`.
const CLIPPY_CONFIG_NAMES: [&str; 2] = ["clippy.toml", ".clippy.toml"];

/// The folders of the repository root that the search for Clippy configuration files skips: build output and Git's own data.
const NOT_SEARCHED: [&str; 2] = ["target", ".git"];

/// A crate of the workspace.
pub struct Member {
    /// The package name from its manifest.
    pub name: String,
    /// The path of its `Cargo.toml`.
    pub manifest: PathBuf,
    /// The text of its `Cargo.toml`.
    pub text: String,
}

/// What `clippy.toml` disallows.
pub struct ClippyConfig {
    /// The paths in `disallowed-methods`, such as `f64::sin`.
    pub methods: Vec<String>,
    /// The paths in `disallowed-types`.
    pub types: Vec<String>,
}

/// The configuration files the checks read.
pub struct Workspace {
    /// The root `Cargo.toml`.
    pub cargo_toml: String,
    /// `rust-toolchain.toml`.
    pub toolchain_toml: String,
    /// What `clippy.toml` disallows.
    pub clippy: ClippyConfig,
    /// Every crate of the workspace, as Cargo sees it.
    pub members: Vec<Member>,
}

impl Workspace {
    /// Reads the configuration files and asks Cargo for the list of workspace members.
    pub fn load(root: &Path) -> Result<Self, String> {
        let clippy_toml = read(&root.join("clippy.toml"))?;
        let clippy = ClippyConfig {
            methods: disallowed(&clippy_toml, "disallowed-methods")?,
            types: disallowed(&clippy_toml, "disallowed-types")?,
        };
        let mut members = Vec::new();
        for manifest in member_manifests(root)? {
            let text = read(&manifest)?;
            let name = toml_subset::value(&text, "package", "name")
                .and_then(|name| name.ok_or_else(|| "no `name` in [package]".to_owned()))
                .and_then(|name| toml_subset::string(&name))
                .map_err(|error| format!("{}: {error}", manifest.display()))?;
            members.push(Member {
                name,
                manifest,
                text,
            });
        }
        Ok(Self {
            cargo_toml: read(&root.join("Cargo.toml"))?,
            toolchain_toml: read(&root.join("rust-toolchain.toml"))?,
            clippy,
            members,
        })
    }
}

/// Runs every configuration check and returns one line per check that passed, or every problem found.
pub fn check(workspace: &Workspace, native_only: &[&str]) -> Result<Vec<String>, String> {
    let results = [
        check_unsafe_forbidden(&workspace.cargo_toml),
        check_lint_inheritance(&workspace.members, native_only),
        check_binding_copies(&workspace.cargo_toml, &workspace.members),
        check_rust_version(&workspace.cargo_toml, &workspace.toolchain_toml),
        check_clippy_requirements(&workspace.clippy),
    ];
    let mut passed = Vec::new();
    let mut problems = Vec::new();
    for result in results {
        match result {
            Ok(line) => passed.push(line),
            Err(problem) => problems.push(problem),
        }
    }
    if problems.is_empty() {
        Ok(passed)
    } else {
        Err(problems.join("\n"))
    }
}

/// The workspace lint table forbids unsafe code (ADR-0006 §2); `deny` would let any crate switch it off again.
fn check_unsafe_forbidden(cargo_toml: &str) -> Result<String, String> {
    match toml_subset::value(cargo_toml, "workspace.lints.rust", "unsafe_code")? {
        Some(level) if level == "\"forbid\"" => {
            Ok("the workspace lints forbid unsafe code (`unsafe_code = \"forbid\"`)".to_owned())
        }
        other => Err(format!(
            "the root Cargo.toml must set `unsafe_code = \"forbid\"` in [workspace.lints.rust] (ADR-0006 §2); found {}",
            other.unwrap_or_else(|| "nothing".to_owned())
        )),
    }
}

/// Every crate except the binding crates inherits the workspace lints, so no crate can quietly opt out of them. The crate lists in this file must name real crates.
fn check_lint_inheritance(members: &[Member], native_only: &[&str]) -> Result<String, String> {
    let mut problems = Vec::new();
    let mut inheriting = 0;
    for member in members {
        if BINDING_CRATES.contains(&member.name.as_str()) {
            continue;
        }
        match toml_subset::value(&member.text, "lints", "workspace") {
            Ok(Some(value)) if value == "true" => inheriting += 1,
            Ok(_) => problems.push(format!(
                "{} must inherit the workspace lints: add `[lints]` with `workspace = true` to {}",
                member.name,
                member.manifest.display()
            )),
            Err(error) => problems.push(format!("{}: {error}", member.manifest.display())),
        }
    }
    for name in BINDING_CRATES.iter().chain(native_only) {
        if !members.iter().any(|member| member.name == *name) {
            problems.push(format!(
                "`{name}` is listed in xtask but is not a workspace member; update the list"
            ));
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "{inheriting} crates inherit the workspace lints; only {} keep their own copy",
            BINDING_CRATES.join(" and ")
        ))
    } else {
        Err(problems.join("\n"))
    }
}

/// The binding crates' lint tables equal the workspace tables, except that `unsafe_code` is "deny" instead of "forbid".
fn check_binding_copies(cargo_toml: &str, members: &[Member]) -> Result<String, String> {
    let mut expected = lint_tables(cargo_toml, "workspace.lints")?;
    expected.insert("rust.unsafe_code".to_owned(), "\"deny\"".to_owned());
    let mut problems = Vec::new();
    for member in members
        .iter()
        .filter(|member| BINDING_CRATES.contains(&member.name.as_str()))
    {
        let actual = lint_tables(&member.text, "lints")
            .map_err(|error| format!("{}: {error}", member.manifest.display()))?;
        for difference in differences(&expected, &actual) {
            problems.push(format!(
                "{}: {difference}. Copy the [workspace.lints.*] tables of the root Cargo.toml into [lints.*], changing only `unsafe_code` to \"deny\"",
                member.manifest.display()
            ));
        }
    }
    if problems.is_empty() {
        Ok(format!(
            "the lint tables of {} match the workspace tables, with `unsafe_code = \"deny\"`",
            BINDING_CRATES.join(" and ")
        ))
    } else {
        Err(problems.join("\n"))
    }
}

/// `rust-version` equals the pinned toolchain, which is also the minimum supported Rust version (ADR-0006 §7).
fn check_rust_version(cargo_toml: &str, toolchain_toml: &str) -> Result<String, String> {
    let channel = toolchain_channel(toolchain_toml)?;
    let rust_version = toml_subset::value(cargo_toml, "workspace.package", "rust-version")?
        .ok_or("the root Cargo.toml has no `rust-version` in [workspace.package]")?;
    let rust_version = toml_subset::string(&rust_version)?;
    if rust_version == channel {
        Ok(format!(
            "`rust-version` equals the pinned toolchain ({channel})"
        ))
    } else {
        Err(format!(
            "`rust-version` in Cargo.toml is {rust_version}, but rust-toolchain.toml pins {channel}; they must be equal (ADR-0006 §7)"
        ))
    }
}

/// `clippy.toml` still forbids every method and type that ADR-0005 requires.
fn check_clippy_requirements(clippy: &ClippyConfig) -> Result<String, String> {
    let mut missing = Vec::new();
    for float in ["f32", "f64"] {
        for method in REQUIRED_FLOAT_METHODS {
            let path = format!("{float}::{method}");
            if !clippy.methods.contains(&path) {
                missing.push(path);
            }
        }
    }
    for path in REQUIRED_TYPES {
        if !clippy.types.iter().any(|configured| configured == path) {
            missing.push(path.to_owned());
        }
    }
    if missing.is_empty() {
        Ok(format!(
            "clippy.toml forbids all {} required float methods and both hash collections",
            REQUIRED_FLOAT_METHODS.len() * 2
        ))
    } else {
        Err(format!(
            "clippy.toml no longer forbids {}, which ADR-0005 requires",
            missing.join(", ")
        ))
    }
}

/// Clippy reads only the root `clippy.toml`. `conf_dir` is the value of the `CLIPPY_CONF_DIR` environment variable, if it is set.
///
/// Clippy does not merge configuration files: for each crate it uses only the nearest `clippy.toml` or `.clippy.toml` it finds walking up from the crate's folder, or the one in `CLIPPY_CONF_DIR`. Any other file, or that variable, therefore replaces the root `clippy.toml` and silently drops its bans on platform floating-point math and hash collections, while the lint canary, which lives in `xtask/`, may keep passing.
pub fn check_clippy_configuration(root: &Path, conf_dir: Option<&OsStr>) -> Result<String, String> {
    if let Some(conf_dir) = conf_dir {
        return Err(format!(
            "the CLIPPY_CONF_DIR environment variable is set (to `{}`). It makes Clippy read its configuration from that folder instead of the root clippy.toml, so the bans on platform floating-point math and hash collections (ADR-0005) would silently stop applying. Unset it, or remove it from the [env] table of .cargo/config.toml, and run the gate again.",
            conf_dir.to_string_lossy()
        ));
    }
    let strays = stray_clippy_configs(root)?;
    if strays.is_empty() {
        return Ok("Clippy reads only the root clippy.toml (no other clippy.toml or .clippy.toml, and CLIPPY_CONF_DIR is not set)".to_owned());
    }
    let list: Vec<String> = strays
        .iter()
        .map(|path| format!("  {}", path.display()))
        .collect();
    Err(format!(
        "Clippy configuration files other than the root clippy.toml were found:\n{}\nClippy does not merge configuration files: for each crate it reads only the nearest clippy.toml or .clippy.toml above the crate's folder. A file below the root therefore replaces the root clippy.toml for every crate beneath it, and silently drops its bans on platform floating-point math and hash collections (ADR-0005). Put any Clippy setting you need into the root clippy.toml and delete these files.",
        list.join("\n")
    ))
}

/// Every file or folder named like a Clippy configuration file anywhere in the repository, except the root `clippy.toml`, as paths relative to the root and sorted. Symbolic links are reported by name but not followed.
fn stray_clippy_configs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        let absolute = root.join(&folder);
        let entries = std::fs::read_dir(&absolute)
            .map_err(|error| format!("cannot read {}: {error}", absolute.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot read {}: {error}", absolute.display()))?;
            let name = entry.file_name();
            let path = folder.join(&name);
            let is_config = CLIPPY_CONFIG_NAMES
                .iter()
                .any(|config| name.to_string_lossy().eq_ignore_ascii_case(config));
            if is_config && path != Path::new("clippy.toml") {
                found.push(path.clone());
            }
            let file_type = entry
                .file_type()
                .map_err(|error| format!("cannot read {}: {error}", root.join(&path).display()))?;
            let skipped = folder.as_os_str().is_empty()
                && NOT_SEARCHED.iter().any(|skip| name == OsStr::new(skip));
            if file_type.is_dir() && !skipped {
                folders.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

/// The pinned toolchain version from `rust-toolchain.toml`.
pub fn toolchain_channel(toolchain_toml: &str) -> Result<String, String> {
    let channel = toml_subset::value(toolchain_toml, "toolchain", "channel")?
        .ok_or("rust-toolchain.toml has no `channel` in [toolchain]")?;
    toml_subset::string(&channel)
}

/// All lint levels of the tables `[<prefix>.rust]`, `[<prefix>.clippy]` and `[<prefix>.rustdoc]`, keyed as `tool.lint`.
fn lint_tables(text: &str, prefix: &str) -> Result<BTreeMap<String, String>, String> {
    let mut lints = BTreeMap::new();
    for tool in LINT_TOOLS {
        for entry in toml_subset::table(text, &format!("{prefix}.{tool}"))?.unwrap_or_default() {
            if lints
                .insert(format!("{tool}.{}", entry.key), entry.value)
                .is_some()
            {
                return Err(format!(
                    "line {}: `{}` appears twice",
                    entry.line, entry.key
                ));
            }
        }
    }
    Ok(lints)
}

/// Describes how `actual` differs from `expected`.
fn differences(
    expected: &BTreeMap<String, String>,
    actual: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut found = Vec::new();
    for (lint, level) in expected {
        match actual.get(lint) {
            None => found.push(format!("`{lint}` is missing")),
            Some(other) if other != level => {
                found.push(format!("`{lint}` is {other} instead of {level}"));
            }
            Some(_) => {}
        }
    }
    for lint in actual.keys().filter(|lint| !expected.contains_key(*lint)) {
        found.push(format!("`{lint}` is not in the workspace tables"));
    }
    found
}

/// The `path` of every entry in one of `clippy.toml`'s disallowed lists.
fn disallowed(clippy_toml: &str, key: &str) -> Result<Vec<String>, String> {
    let array = toml_subset::value(clippy_toml, "", key)
        .and_then(|array| array.ok_or_else(|| format!("no `{key}` list")))
        .map_err(|error| format!("clippy.toml: {error}"))?;
    toml_subset::field_of_each(&array, "path")
        .map_err(|error| format!("clippy.toml, `{key}`: {error}"))
}

/// The manifest of every workspace member, from `cargo metadata`.
fn member_manifests(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut command = process::cargo(root);
    command.args(["metadata", "--no-deps", "--format-version", "1", "--locked"]);
    let output = process::capture(&mut command)?;
    if !output.status.success() {
        return Err(format!(
            "`{}` failed:\n{}",
            process::display(&command),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let manifests: Vec<PathBuf> =
        json::string_values(&String::from_utf8_lossy(&output.stdout), "manifest_path")
            .into_iter()
            .map(PathBuf::from)
            .collect();
    if manifests.is_empty() {
        return Err("`cargo metadata` listed no workspace members".to_owned());
    }
    Ok(manifests)
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = r#"
[workspace.package]
rust-version = "1.99.0"

[workspace.lints.rust]
unsafe_code = "forbid"
missing_docs = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
unwrap_used = "warn"
"#;

    const BINDING: &str = r#"
[package]
name = "bayan-ffi"

[lints.rust]
# A different comment and different spacing are fine.
unsafe_code    = "deny"
missing_docs = "warn"

[lints.clippy]
all = {level="warn", priority=-1}
unwrap_used = "warn"
"#;

    fn member(name: &str, text: &str) -> Member {
        Member {
            name: name.to_owned(),
            manifest: PathBuf::from(format!("crates/{name}/Cargo.toml")),
            text: text.to_owned(),
        }
    }

    fn inheriting(name: &str) -> Member {
        member(
            name,
            &format!("[package]\nname = \"{name}\"\n\n[lints]\nworkspace = true\n"),
        )
    }

    fn all_members() -> Vec<Member> {
        vec![
            inheriting("bayan-units"),
            inheriting("bayan-cli"),
            inheriting("xtask"),
            member("bayan-ffi", BINDING),
            member("bayan-wasm", &BINDING.replace("bayan-ffi", "bayan-wasm")),
        ]
    }

    const NATIVE_ONLY: [&str; 3] = ["bayan-cli", "bayan-ffi", "xtask"];

    #[test]
    fn requires_forbid_in_the_workspace() {
        assert!(check_unsafe_forbidden(ROOT).is_ok());
        assert!(check_unsafe_forbidden(&ROOT.replace("\"forbid\"", "\"deny\"")).is_err());
        assert!(check_unsafe_forbidden("[workspace]\n").is_err());
    }

    #[test]
    fn accepts_crates_that_inherit_the_workspace_lints() {
        let line = check_lint_inheritance(&all_members(), &NATIVE_ONLY).unwrap();
        assert!(line.starts_with("3 crates inherit"), "{line}");
    }

    #[test]
    fn rejects_a_crate_that_does_not_inherit_the_workspace_lints() {
        let mut members = all_members();
        members.push(member("bayan-xml", "[package]\nname = \"bayan-xml\"\n"));
        members.push(member(
            "bayan-opc",
            "[package]\nname = \"bayan-opc\"\n[lints]\nworkspace = false\n",
        ));
        let problems = check_lint_inheritance(&members, &NATIVE_ONLY).unwrap_err();
        assert!(problems.contains("bayan-xml must inherit"), "{problems}");
        assert!(problems.contains("bayan-opc must inherit"), "{problems}");
    }

    #[test]
    fn rejects_lists_that_name_crates_that_do_not_exist() {
        let problems = check_lint_inheritance(&all_members(), &["bayan-gone"]).unwrap_err();
        assert!(problems.contains("`bayan-gone` is listed"), "{problems}");
    }

    #[test]
    fn accepts_binding_copies_that_differ_only_in_unsafe_code() {
        assert!(check_binding_copies(ROOT, &all_members()).is_ok());
    }

    #[test]
    fn detects_drift_in_a_binding_copy() {
        let drifted = [
            (
                BINDING.replace("unwrap_used = \"warn\"\n", ""),
                "`clippy.unwrap_used` is missing",
            ),
            (
                BINDING.replace("missing_docs = \"warn\"", "missing_docs = \"allow\""),
                "`rust.missing_docs` is \"allow\" instead of \"warn\"",
            ),
            (
                BINDING.replace("unsafe_code    = \"deny\"", "unsafe_code = \"allow\""),
                "`rust.unsafe_code` is \"allow\" instead of \"deny\"",
            ),
            (
                format!("{BINDING}todo = \"warn\"\n"),
                "`clippy.todo` is not in the workspace tables",
            ),
        ];
        for (text, expected) in drifted {
            let members = vec![member("bayan-ffi", &text)];
            let problems = check_binding_copies(ROOT, &members).unwrap_err();
            assert!(
                problems.contains(expected),
                "expected {expected:?} in {problems}"
            );
        }
    }

    #[test]
    fn compares_rust_version_with_the_toolchain() {
        let toolchain = "[toolchain]\nchannel = \"1.99.0\"\n";
        assert!(check_rust_version(ROOT, toolchain).is_ok());
        let problem =
            check_rust_version(ROOT, &toolchain.replace("1.99.0", "1.100.0")).unwrap_err();
        assert!(problem.contains("1.100.0"), "{problem}");
    }

    #[test]
    fn requires_every_float_method_and_hash_type() {
        let complete = ClippyConfig {
            methods: ["f32", "f64"]
                .iter()
                .flat_map(|float| {
                    REQUIRED_FLOAT_METHODS
                        .iter()
                        .map(move |method| format!("{float}::{method}"))
                })
                .collect(),
            types: REQUIRED_TYPES
                .iter()
                .map(|path| (*path).to_owned())
                .collect(),
        };
        assert!(check_clippy_requirements(&complete).is_ok());

        let weakened = ClippyConfig {
            methods: complete
                .methods
                .iter()
                .filter(|path| *path != "f64::powi")
                .cloned()
                .collect(),
            types: vec!["std::collections::HashMap".to_owned()],
        };
        let problem = check_clippy_requirements(&weakened).unwrap_err();
        assert!(
            problem.contains("f64::powi") && problem.contains("HashSet"),
            "{problem}"
        );
    }

    #[test]
    fn reads_the_disallowed_lists_of_the_real_clippy_toml() {
        // The real file must parse and contain every required entry; it may contain more, which the canaries then prove.
        let text = include_str!("../../clippy.toml");
        let clippy = ClippyConfig {
            methods: disallowed(text, "disallowed-methods").unwrap(),
            types: disallowed(text, "disallowed-types").unwrap(),
        };
        assert!(clippy.methods.contains(&"f64::sin".to_owned()));
        assert!(check_clippy_requirements(&clippy).is_ok());
    }

    #[test]
    fn the_real_binding_manifests_match_the_real_workspace_tables() {
        let root = include_str!("../../Cargo.toml");
        let members = vec![
            member(
                "bayan-ffi",
                include_str!("../../crates/bayan-ffi/Cargo.toml"),
            ),
            member(
                "bayan-wasm",
                include_str!("../../crates/bayan-wasm/Cargo.toml"),
            ),
        ];
        assert!(check_binding_copies(root, &members).is_ok());
    }

    /// A folder tree for one test, removed again when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("bayan-xtask-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn files(self, relative_paths: &[&str]) -> Self {
            for relative in relative_paths {
                let path = self.0.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "").unwrap();
            }
            self
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn accepts_only_the_root_clippy_toml() {
        let scratch = Scratch::new("only-root").files(&[
            "clippy.toml",
            "Cargo.toml",
            "crates/bayan-units/Cargo.toml",
            "crates/bayan-units/src/lib.rs",
            // Build output and Git's data are not searched.
            "target/package/old/clippy.toml",
            ".git/clippy.toml",
        ]);
        let line = check_clippy_configuration(&scratch.0, None)
            .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(
            line.starts_with("Clippy reads only the root clippy.toml"),
            "{line}"
        );
    }

    #[test]
    fn rejects_clippy_configuration_files_below_or_beside_the_root() {
        let scratch = Scratch::new("strays").files(&[
            "clippy.toml",
            ".clippy.toml",
            "crates/clippy.toml",
            "crates/bayan-units/.clippy.toml",
            // On Windows and macOS this is the same file as `clippy.toml`.
            "fuzz/Clippy.TOML",
            // Only the root's `target` folder is skipped.
            "spikes/target/clippy.toml",
        ]);
        let expected: Vec<PathBuf> = [
            ".clippy.toml",
            "crates/bayan-units/.clippy.toml",
            "crates/clippy.toml",
            "fuzz/Clippy.TOML",
            "spikes/target/clippy.toml",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(stray_clippy_configs(&scratch.0).unwrap(), expected);
        let problem = check_clippy_configuration(&scratch.0, None).unwrap_err();
        assert!(
            problem.contains("Clippy does not merge configuration files"),
            "{problem}"
        );
        let listed = format!("  {}", Path::new("crates").join("clippy.toml").display());
        assert!(problem.contains(&listed), "{problem}");
    }

    #[test]
    fn rejects_clippy_conf_dir() {
        let scratch = Scratch::new("conf-dir").files(&["clippy.toml"]);
        let problem = check_clippy_configuration(&scratch.0, Some(OsStr::new("/somewhere/else")))
            .unwrap_err();
        assert!(problem.contains("CLIPPY_CONF_DIR"), "{problem}");
        assert!(problem.contains("/somewhere/else"), "{problem}");
        assert!(
            check_clippy_configuration(&scratch.0, Some(OsStr::new(""))).is_err(),
            "an empty value is still set"
        );
    }
}
