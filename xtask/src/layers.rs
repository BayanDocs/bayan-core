//! The layers of bayan-core's crates, and the rules about their dependencies that only a table can check (the architecture review of 2026-11-20, finding M16).
//!
//! The architecture arranges the engine's crates in layers, from Foundation to Bindings and tools, and a crate may depend only on crates in its own layer or below (architecture §4; AGENTS.md, "Layering"). The Fidelity Lab, the spikes and the build tools stand above all of them: they may use every crate of the engine, and no crate of the engine may depend on them (lab/README.md, spikes/README.md). [`check`] reads the workspace members and their dependencies from `cargo metadata` in the preflight, before anything is built, and fails:
//!
//! - on a member that `LAYERS` in `verify.rs` does not place in a layer, and on an entry of the table that names no member, names a member twice or leaves a layer without a name, so that the table cannot go stale;
//! - on a normal or build dependency of a member on a member of a higher layer. Dev-dependencies, which only a member's tests, benchmarks and examples use, are not judged, as the review asks: they never reach the code that ships;
//! - on a normal or build dependency on Loro (the package `loro`, or one of its own crates, whose names start with `loro-` or `loro_`) of any member but bayan-crdt, the only crate that may talk to the CRDT library, so that it stays replaceable (ADR-0008 decision 1). The dev-dependency of bayan-model's tests on Loro is the first review's finding L1, which is open: once it is settled, dev-dependencies join this rule;
//! - on a member whose license is not under the GPL, such as the Apache-2.0 protocol crate that SRV-104 creates (ADR-0003), that depends, also through other members, on a member whose license is. Here every kind of its own dependencies counts, its tests' included, because the crate's own code may never depend on GPL code at all (AGENTS.md, "Licensing"); from there the normal and build dependencies of the members it reaches count, as Cargo builds them. Dependencies from crates.io cannot be under the GPL: cargo-deny's license allowlist in deny.toml has no license of the GPL family, and the gate's deny step checks every one of them against it. A member without a license fails too, because its license cannot be judged.
//!
//! It judges what the manifests declare, as `cargo metadata --no-deps` reports it, whatever the dependency is renamed to (`package = "…"`), on every platform (`[target.'cfg(…)'.dependencies]`) and whether it is optional or not.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::json;

/// One layer of the table, lowest first: its name and its members.
pub type Layer = (&'static str, &'static [&'static str]);

/// The only member that may depend on Loro (ADR-0008 decision 1).
pub const LORO_ADAPTER: &str = "bayan-crdt";

/// What kind of dependency a manifest declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// In `[dependencies]`: the code that ships uses it.
    Normal,
    /// In `[build-dependencies]`: the build script uses it.
    Build,
    /// In `[dev-dependencies]`: only the tests, benchmarks and examples use it.
    Dev,
}

/// A dependency that a member's manifest declares.
#[derive(Debug)]
struct Dependency {
    /// The name of the package, not the name it may be renamed to.
    package: String,
    /// Its kind.
    kind: Kind,
    /// The folder it comes from, for a path dependency.
    path: Option<PathBuf>,
}

/// A workspace member, as the layer check reads it.
#[derive(Debug)]
struct Member {
    /// The package name.
    name: String,
    /// The folder of its `Cargo.toml`.
    folder: PathBuf,
    /// Its license, as its manifest states it.
    license: Option<String>,
    /// Every dependency it declares.
    dependencies: Vec<Dependency>,
}

/// Checks the dependencies of the workspace members, from the JSON of `cargo metadata --no-deps` (`policy::metadata`), against the table `layers` and the rules above. Returns a line for the preflight's report.
pub fn check(metadata: &str, layers: &[Layer]) -> Result<String, String> {
    let members = members(metadata)?;
    let mut problems = check_table(layers, &members);
    if !problems.is_empty() {
        return Err(refusal(&problems));
    }
    let layer_of = |name: &str| layers.iter().position(|(_, listed)| listed.contains(&name));
    let member_in = |folder: &Path| members.iter().find(|member| member.folder == folder);
    for member in &members {
        for dependency in &member.dependencies {
            if dependency.kind == Kind::Dev {
                continue;
            }
            if is_loro(&dependency.package) && member.name != LORO_ADAPTER {
                problems.push(format!(
                    "  `{}` depends on `{}`: only {LORO_ADAPTER} may use Loro (ADR-0008 decision 1); use bayan-crdt's primitives instead",
                    member.name, dependency.package
                ));
            }
            let Some(path) = &dependency.path else {
                continue;
            };
            let Some(on) = member_in(path) else {
                problems.push(format!(
                    "  `{}` depends by path on `{}`, which is not a workspace member",
                    member.name, dependency.package
                ));
                continue;
            };
            if let (Some(from_layer), Some(on_layer)) = (layer_of(&member.name), layer_of(&on.name))
                && on_layer > from_layer
            {
                problems.push(format!(
                    "  `{}` ({}) depends on `{}` ({}), a higher layer: a crate may depend only on crates of its own layer or below",
                    member.name, layers_name(layers, from_layer), on.name, layers_name(layers, on_layer)
                ));
            }
        }
    }
    let mut permissive = 0;
    for member in &members {
        let Some(license) = &member.license else {
            problems.push(format!(
                "  `{}` states no license in its manifest, so whether it may depend on code under the GPL cannot be judged",
                member.name
            ));
            continue;
        };
        if is_gpl(license) {
            continue;
        }
        permissive += 1;
        for (reached, through) in reachable(member, &members) {
            match &reached.license {
                Some(license) if !is_gpl(license) => {}
                _ => problems.push(format!(
                    "  `{}` ({license}) depends{} on `{}` ({}): a crate that is not under the GPL may never depend on code that is (ADR-0003, AGENTS.md \"Licensing\")",
                    member.name,
                    through.map_or_else(String::new, |via| format!(" through `{via}`")),
                    reached.name,
                    reached.license.as_deref().unwrap_or("no license"),
                )),
            }
        }
    }
    if !problems.is_empty() {
        return Err(refusal(&problems));
    }
    let filled = layers
        .iter()
        .filter(|(_, listed)| !listed.is_empty())
        .count();
    Ok(format!(
        "layers: the {} members stand in {filled} of the {} layers of LAYERS, and every normal and build dependency between them points to the member's own layer or below; only {LORO_ADAPTER} depends on Loro; {}",
        members.len(),
        layers.len(),
        match permissive {
            0 => "every member is under the GPL".to_owned(),
            1 => "the one member that is not under the GPL depends on none that is".to_owned(),
            count =>
                format!("the {count} members that are not under the GPL depend on none that is"),
        }
    ))
}

/// The problems of the table itself: a layer without a name or listed twice, a member listed twice, an entry that is not a workspace member, and a member that no layer holds.
fn check_table(layers: &[Layer], members: &[Member]) -> Vec<String> {
    let mut problems = Vec::new();
    let mut names = BTreeSet::new();
    let mut listed = BTreeSet::new();
    for (name, entries) in layers {
        if name.trim().is_empty() {
            problems.push("  a layer has no name".to_owned());
        } else if !names.insert(*name) {
            problems.push(format!("  the layer `{name}` is listed twice"));
        }
        for entry in *entries {
            if !listed.insert(*entry) {
                problems.push(format!("  `{entry}` is listed twice"));
            }
            if !members.iter().any(|member| member.name == *entry) {
                problems.push(format!(
                    "  `{entry}` (layer `{name}`) is not a workspace member; update the list"
                ));
            }
        }
    }
    for member in members {
        if !listed.contains(member.name.as_str()) {
            problems.push(format!(
                "  `{}` stands in no layer: add it to its layer in LAYERS, after reading architecture §4",
                member.name
            ));
        }
    }
    if !members.iter().any(|member| member.name == LORO_ADAPTER) {
        problems.push(format!(
            "  `{LORO_ADAPTER}`, the only member that may use Loro, is not a workspace member; update LORO_ADAPTER in xtask/src/layers.rs"
        ));
    }
    problems
}

/// The members that `member` depends on, directly or through other members, each with the member through which it was first reached (`None` if directly): its own dependencies of every kind, then the normal and build dependencies of the members they lead to.
fn reachable<'a>(member: &Member, members: &'a [Member]) -> Vec<(&'a Member, Option<&'a str>)> {
    let lookup = |dependency: &Dependency| {
        dependency
            .path
            .as_ref()
            .and_then(|path| members.iter().find(|other| &other.folder == path))
    };
    let mut found: Vec<(&Member, Option<&str>)> = Vec::new();
    let mut queue: Vec<(&Member, Option<&str>)> = member
        .dependencies
        .iter()
        .filter_map(lookup)
        .map(|other| (other, None))
        .collect();
    while let Some((next, through)) = queue.pop() {
        if next.name == member.name || found.iter().any(|(seen, _)| seen.name == next.name) {
            continue;
        }
        found.push((next, through));
        let via = through.or(Some(next.name.as_str()));
        queue.extend(
            next.dependencies
                .iter()
                .filter(|dependency| dependency.kind != Kind::Dev)
                .filter_map(lookup)
                .map(|other| (other, via)),
        );
    }
    found
}

/// The name of the layer at `index`.
fn layers_name(layers: &[Layer], index: usize) -> &'static str {
    layers.get(index).map_or("", |(name, _)| name)
}

/// Whether `package` is Loro or one of its own crates.
fn is_loro(package: &str) -> bool {
    package == "loro" || package.starts_with("loro-") || package.starts_with("loro_")
}

/// Whether an SPDX license expression names a license of the GPL family (GPL, LGPL or AGPL).
fn is_gpl(license: &str) -> bool {
    license.contains("GPL")
}

/// The error for `problems`.
fn refusal(problems: &[String]) -> String {
    format!(
        "the workspace's dependencies break the layering, Loro or license rules (xtask/src/layers.rs, LAYERS in xtask/src/verify.rs):\n{}\nA crate may depend only on crates of its own layer or below (architecture §4), only bayan-crdt may use Loro (ADR-0008), and a crate that is not under the GPL may never depend on one that is (ADR-0003). Change the dependency, not the table; a new crate goes into the layer that architecture §4 gives it.",
        problems.join("\n")
    )
}

/// The workspace members with their licenses and the dependencies their manifests declare, from the JSON of `cargo metadata --no-deps`.
fn members(metadata: &str) -> Result<Vec<Member>, String> {
    let invalid = |what: &str| format!("unexpected `cargo metadata` output: {what}");
    let value = json::parse(metadata).map_err(|error| invalid(&error))?;
    let list = value
        .get("packages")
        .and_then(json::Value::as_array)
        .ok_or_else(|| invalid("no `packages` list"))?;
    let mut members = Vec::new();
    for package in list {
        let name = package
            .get("name")
            .and_then(json::Value::as_str)
            .ok_or_else(|| invalid("a package without a name"))?;
        let folder = package
            .get("manifest_path")
            .and_then(json::Value::as_str)
            .and_then(|manifest| Path::new(manifest).parent())
            .ok_or_else(|| invalid(&format!("no `manifest_path` for {name}")))?;
        let license = match package.get("license") {
            Some(json::Value::String(license)) => Some(license.clone()),
            Some(json::Value::Null) => None,
            _ => return Err(invalid(&format!("no `license` field for {name}"))),
        };
        let mut dependencies = Vec::new();
        for dependency in package
            .get("dependencies")
            .and_then(json::Value::as_array)
            .ok_or_else(|| invalid(&format!("no `dependencies` for {name}")))?
        {
            let package = dependency
                .get("name")
                .and_then(json::Value::as_str)
                .ok_or_else(|| invalid(&format!("a dependency of {name} without a name")))?;
            let kind = match dependency.get("kind") {
                Some(json::Value::Null) => Kind::Normal,
                Some(json::Value::String(kind)) if kind == "build" => Kind::Build,
                Some(json::Value::String(kind)) if kind == "dev" => Kind::Dev,
                _ => {
                    return Err(invalid(&format!(
                        "the dependency of {name} on {package} has no `kind` that Cargo writes"
                    )));
                }
            };
            let path = match dependency.get("path") {
                None | Some(json::Value::Null) => None,
                Some(json::Value::String(path)) => Some(PathBuf::from(path)),
                Some(_) => {
                    return Err(invalid(&format!(
                        "the dependency of {name} on {package} has a `path` that is not a string"
                    )));
                }
            };
            dependencies.push(Dependency {
                package: package.to_owned(),
                kind,
                path,
            });
        }
        members.push(Member {
            name: name.to_owned(),
            folder: folder.to_path_buf(),
            license,
            dependencies,
        });
    }
    if members.is_empty() {
        return Err(invalid("no workspace members"));
    }
    Ok(members)
}

#[cfg(test)]
mod tests;
