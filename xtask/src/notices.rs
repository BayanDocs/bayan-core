//! Third-party licence notices for the engine's artifacts (CORE-007).
//!
//! The C SDK's libraries and the WebAssembly module contain code from other crates (serde, serde_json, wasm-bindgen and their dependencies) and from the Rust standard library. Their licences, MIT and Apache-2.0 for nearly all of them, require their notices to travel with every copy of the binaries, so each artifact carries `THIRD-PARTY-LICENSES.txt`, written here, and the standard library's own notices from the toolchain (`share/doc/rust/COPYRIGHT-library.html`).
//!
//! The crates are those that `cargo metadata` lists as normal dependencies (not build or development dependencies) of the artifact's crate for its target, leaving out procedural macros, which run while compiling and leave no code in the output, and the workspace's own crates. As bayan-web's `scripts/check-licenses.ts` does for the web bundle, a crate without a licence file, or with an Apache-2.0 NOTICE file (which section 4(d) of that licence requires us to reproduce), stops the build, so that a person decides.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::json::Value;

/// A third-party crate compiled into an artifact.
#[derive(Debug, PartialEq, Eq)]
pub struct Crate {
    /// The package name.
    pub name: String,
    /// The exact version.
    pub version: String,
    /// The licence expression from its manifest, such as `MIT OR Apache-2.0`.
    pub license: String,
    /// The folder of its `Cargo.toml`, where its licence files are.
    pub folder: PathBuf,
}

/// The third-party crates whose code ends up in the library of `root`, a workspace member, from `cargo metadata --format-version 1 --filter-platform <target>` (without `--no-deps`), sorted by name and version.
pub fn bundled_crates(metadata: &Value, root: &str) -> Result<Vec<Crate>, String> {
    let invalid = |what: &str| format!("unexpected `cargo metadata` output: {what}");
    let members: BTreeSet<&str> = metadata
        .get("workspace_members")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("no `workspace_members`"))?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut packages = BTreeMap::new();
    for package in metadata
        .get("packages")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("no `packages`"))?
    {
        let field = |name: &str| package.get(name).and_then(Value::as_str);
        let id = field("id").ok_or_else(|| invalid("a package without an id"))?;
        packages.insert(id, package);
    }
    let root_id = members
        .iter()
        .copied()
        .find(|id| {
            packages
                .get(id)
                .and_then(|package| package.get("name"))
                .and_then(Value::as_str)
                == Some(root)
        })
        .ok_or_else(|| invalid(&format!("{root} is not a workspace member")))?;
    let mut dependencies: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in metadata
        .get("resolve")
        .and_then(|resolve| resolve.get("nodes"))
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("no `resolve.nodes`; run it without --no-deps"))?
    {
        let id = node
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("a node without an id"))?;
        let normal = node
            .get("deps")
            .and_then(Value::as_array)
            .unwrap_or_default()
            .iter()
            .filter(|dependency| is_normal(dependency))
            .filter_map(|dependency| dependency.get("pkg").and_then(Value::as_str))
            .collect();
        dependencies.insert(id, normal);
    }
    // Walk the normal dependencies from the root, but not into procedural macros.
    let mut seen = BTreeSet::from([root_id]);
    let mut queue = vec![root_id];
    while let Some(id) = queue.pop() {
        for &dependency in dependencies.get(id).map_or(&[][..], Vec::as_slice) {
            let is_macro = packages
                .get(dependency)
                .is_some_and(|package| is_proc_macro(package));
            if !is_macro && seen.insert(dependency) {
                queue.push(dependency);
            }
        }
    }
    let mut crates = Vec::new();
    for id in seen {
        if members.contains(id) {
            continue;
        }
        let package = packages
            .get(id)
            .ok_or_else(|| invalid(&format!("no package for {id}")))?;
        let text = |name: &str| {
            package
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| invalid(&format!("no `{name}` for {id}")))
        };
        let manifest = text("manifest_path")?;
        crates.push(Crate {
            name: text("name")?,
            version: text("version")?,
            license: package
                .get("license")
                .and_then(Value::as_str)
                .unwrap_or("(no licence expression)")
                .to_owned(),
            folder: Path::new(&manifest)
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(|| invalid(&format!("a strange manifest path for {id}")))?,
        });
    }
    crates.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    Ok(crates)
}

/// Whether a dependency edge of the resolve graph is a normal dependency (`kind` is `null`) for at least one target.
fn is_normal(dependency: &Value) -> bool {
    dependency
        .get("dep_kinds")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .any(|kind| kind.get("kind") == Some(&Value::Null))
}

fn is_proc_macro(package: &Value) -> bool {
    package
        .get("targets")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|target| target.get("kind").and_then(Value::as_array))
        .flatten()
        .any(|kind| kind.as_str() == Some("proc-macro"))
}

/// The kind of a licence file, judged by its name.
#[derive(Debug, PartialEq, Eq)]
enum LicenseFile {
    /// A licence text, such as `LICENSE-MIT` or `COPYING`.
    License,
    /// An Apache-2.0 NOTICE file.
    Notice,
}

fn license_file(name: &str) -> Option<LicenseFile> {
    let name = name.to_ascii_lowercase();
    if name.starts_with("notice") {
        Some(LicenseFile::Notice)
    } else if ["license", "licence", "copying", "copyright", "unlicense"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        Some(LicenseFile::License)
    } else {
        None
    }
}

/// The text of `THIRD-PARTY-LICENSES.txt` for `artifact`: each crate's name, version and licence expression, followed by its licence files.
///
/// # Errors
///
/// Fails if a crate has no licence file or has a NOTICE file, or a file cannot be read.
pub fn notices(artifact: &str, crates: &[Crate]) -> Result<String, String> {
    let rule = "-".repeat(100);
    let mut text = format!(
        "Third-party software in {artifact}\n\n{artifact} contains code from the Rust crates below, under the licences that follow each of them, and from the Rust standard library, whose notices are in RUST-STANDARD-LIBRARY-COPYRIGHT.html.\n"
    );
    let mut problems = Vec::new();
    for item in crates {
        let mut files: Vec<(String, PathBuf)> = std::fs::read_dir(&item.folder)
            .map_err(|error| format!("cannot list {}: {error}", item.folder.display()))?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .filter(|(name, _)| license_file(name).is_some())
            .collect();
        files.sort();
        if let Some((name, _)) = files
            .iter()
            .find(|(name, _)| license_file(name) == Some(LicenseFile::Notice))
        {
            problems.push(format!(
                "{} {} has a NOTICE file ({name}); Apache-2.0 §4(d) requires reproducing it, so add it deliberately before allowing it here",
                item.name, item.version
            ));
            continue;
        }
        if files.is_empty() {
            problems.push(format!(
                "{} {} ({}) has no licence file in {}",
                item.name,
                item.version,
                item.license,
                item.folder.display()
            ));
            continue;
        }
        text.push_str(&format!(
            "\n{rule}\n{} {} ({})\n{rule}\n",
            item.name, item.version, item.license
        ));
        for (name, path) in files {
            let content = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            text.push_str(&format!(
                "\n== {name} ==\n\n{}\n",
                content.replace("\r\n", "\n").trim_end()
            ));
        }
    }
    if problems.is_empty() {
        Ok(text)
    } else {
        Err(format!(
            "the licence notices for {artifact} are incomplete:\n  {}",
            problems.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;

    /// A scratch folder for one test, removed again at the end.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(test: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("bayan-xtask-notices-{}-{test}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// A crate folder with the given files.
        fn folder(&self, name: &str, files: &[(&str, &str)]) -> PathBuf {
            let folder = self.0.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            for (file, text) in files {
                std::fs::write(folder.join(file), text).unwrap();
            }
            folder
        }

        /// A crate folder with the given files; returns its manifest path, escaped for JSON.
        fn package(&self, name: &str, files: &[(&str, &str)]) -> String {
            self.folder(name, files)
                .join("Cargo.toml")
                .display()
                .to_string()
                .replace('\\', "\\\\")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn package(id: &str, name: &str, manifest: &str, kind: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"{name}","version":"1.0.0","license":"MIT OR Apache-2.0","manifest_path":"{manifest}","targets":[{{"kind":["{kind}"]}}]}}"#
        )
    }

    fn dependency(id: &str, kind: &str) -> String {
        format!(r#"{{"pkg":"{id}","dep_kinds":[{{"kind":{kind},"target":null}}]}}"#)
    }

    /// A workspace: `root` depends on `normal` (which depends on `deep`), on the procedural macro `macro` (which depends on `macro-helper`), on `built` as a build dependency, on `tested` as a development dependency, and on the workspace member `sibling`.
    fn metadata(scratch: &Scratch) -> Value {
        let license = [
            ("LICENSE-MIT", "MIT text"),
            ("LICENSE-APACHE", "Apache text"),
        ];
        let packages = [
            package("root", "root", &scratch.package("root", &[]), "lib"),
            package(
                "sibling",
                "sibling",
                &scratch.package("sibling", &[]),
                "lib",
            ),
            package(
                "normal",
                "normal",
                &scratch.package("normal", &license),
                "lib",
            ),
            package(
                "deep",
                "deep",
                &scratch.package("deep", &[("COPYING", "deep text")]),
                "lib",
            ),
            package(
                "macro",
                "macro",
                &scratch.package("macro", &license),
                "proc-macro",
            ),
            package(
                "macro-helper",
                "macro-helper",
                &scratch.package("macro-helper", &license),
                "lib",
            ),
            package("built", "built", &scratch.package("built", &license), "lib"),
            package(
                "tested",
                "tested",
                &scratch.package("tested", &license),
                "lib",
            ),
        ];
        let nodes = [
            format!(
                r#"{{"id":"root","deps":[{},{},{},{},{}]}}"#,
                dependency("normal", "null"),
                dependency("macro", "null"),
                dependency("built", r#""build""#),
                dependency("tested", r#""dev""#),
                dependency("sibling", "null"),
            ),
            format!(
                r#"{{"id":"normal","deps":[{}]}}"#,
                dependency("deep", "null")
            ),
            format!(
                r#"{{"id":"macro","deps":[{}]}}"#,
                dependency("macro-helper", "null")
            ),
            r#"{"id":"sibling","deps":[]}"#.to_owned(),
        ];
        json::parse(&format!(
            r#"{{"packages":[{}],"workspace_members":["root","sibling"],"resolve":{{"nodes":[{}]}}}}"#,
            packages.join(","),
            nodes.join(",")
        ))
        .unwrap()
    }

    #[test]
    fn only_normal_dependencies_outside_procedural_macros_and_the_workspace_count() {
        let scratch = Scratch::new("graph");
        let crates = bundled_crates(&metadata(&scratch), "root").unwrap();
        let names: Vec<&str> = crates.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["deep", "normal"]);
        assert!(bundled_crates(&metadata(&scratch), "missing").is_err());
    }

    #[test]
    fn the_notices_hold_every_licence_file() {
        let scratch = Scratch::new("texts");
        let crates = bundled_crates(&metadata(&scratch), "root").unwrap();
        let text = notices("the artifact", &crates).unwrap();
        assert!(text.starts_with("Third-party software in the artifact\n"));
        assert!(text.contains("deep 1.0.0 (MIT OR Apache-2.0)"));
        assert!(text.contains("== COPYING ==\n\ndeep text\n"));
        assert!(
            text.contains("== LICENSE-APACHE ==\n\nApache text\n\n== LICENSE-MIT ==\n\nMIT text\n")
        );
    }

    #[test]
    fn a_missing_licence_or_a_notice_file_stops_the_build() {
        let scratch = Scratch::new("problems");
        let crates = [
            Crate {
                name: "bare".to_owned(),
                version: "1.0.0".to_owned(),
                license: "MIT".to_owned(),
                folder: scratch.folder("bare", &[("README.md", "no licence")]),
            },
            Crate {
                name: "noticed".to_owned(),
                version: "2.0.0".to_owned(),
                license: "Apache-2.0".to_owned(),
                folder: scratch.folder(
                    "noticed",
                    &[("LICENSE", "Apache text"), ("NOTICE", "a notice")],
                ),
            },
        ];
        let problem = notices("the artifact", &crates).unwrap_err();
        assert!(
            problem.contains("bare 1.0.0 (MIT) has no licence file"),
            "{problem}"
        );
        assert!(
            problem.contains("noticed 2.0.0 has a NOTICE file (NOTICE)"),
            "{problem}"
        );
    }

    #[test]
    fn licence_files_are_recognized_by_name() {
        for name in [
            "LICENSE",
            "LICENSE-MIT",
            "license.txt",
            "LICENCE",
            "COPYING",
            "UNLICENSE",
            "COPYRIGHT",
        ] {
            assert_eq!(license_file(name), Some(LicenseFile::License), "{name}");
        }
        assert_eq!(license_file("NOTICE"), Some(LicenseFile::Notice));
        assert_eq!(license_file("README.md"), None);
    }
}
