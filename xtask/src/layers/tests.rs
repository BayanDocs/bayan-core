use super::*;

const GPL: &str = "GPL-3.0-or-later WITH AdditionRef-BayanDocs-App-Store-Permission";

/// A small table in the shape of the real one.
const TABLE: [Layer; 5] = [
    ("Foundation", &["bayan-units", "bayan-protocol"]),
    ("Model and formats", &["bayan-crdt", "bayan-model"]),
    ("Engine", &["bayan-engine"]),
    ("Fidelity Lab", &["bayan-lab"]),
    ("Spikes", &[]),
];

/// A dependency as `cargo metadata` 1.100 prints it: `kind` is null for a normal dependency, and only a path dependency has a `path`.
fn dependency(name: &str, kind: &str, path: Option<&str>) -> String {
    let kind = match kind {
        "normal" => "null".to_owned(),
        other => format!("\"{other}\""),
    };
    let path = path.map_or_else(String::new, |path| format!(r#","path":"/repo/{path}""#));
    format!(
        r#"{{"name":"{name}","source":null,"req":"*","kind":{kind},"rename":null,"optional":false,"uses_default_features":true,"features":[],"target":null,"registry":null{path}}}"#
    )
}

/// A path dependency on the member `name`, in `crates/<name>`.
fn on(name: &str, kind: &str) -> String {
    dependency(name, kind, Some(&format!("crates/{name}")))
}

/// A member in `crates/<name>`, with its license and dependencies.
fn member(name: &str, license: Option<&str>, dependencies: &[String]) -> String {
    let license = license.map_or_else(|| "null".to_owned(), |license| format!("\"{license}\""));
    format!(
        r#"{{"name":"{name}","version":"0.0.0","id":"path+file:///repo/crates/{name}#0.0.0","license":{license},"dependencies":[{}],"targets":[],"features":{{}},"manifest_path":"/repo/crates/{name}/Cargo.toml"}}"#,
        dependencies.join(",")
    )
}

fn metadata(members: &[String]) -> String {
    format!(
        r#"{{"packages":[{}],"workspace_members":[],"resolve":null,"version":1}}"#,
        members.join(",")
    )
}

/// A workspace that keeps every rule: every kind of dependency that the rules allow, and an Apache-2.0 crate.
fn good() -> Vec<String> {
    vec![
        member(
            "bayan-units",
            Some(GPL),
            &[dependency("serde_core", "normal", None)],
        ),
        member(
            "bayan-protocol",
            Some("Apache-2.0"),
            &[dependency("serde", "normal", None)],
        ),
        member(
            "bayan-crdt",
            Some(GPL),
            &[
                dependency("loro", "normal", None),
                on("bayan-units", "normal"),
            ],
        ),
        member(
            "bayan-model",
            Some(GPL),
            &[
                on("bayan-crdt", "normal"),
                // The first review's L1: a test-only use of Loro, not judged until it is settled.
                dependency("loro", "dev", None),
                // A dev-dependency may point upward: only tests use it.
                on("bayan-engine", "dev"),
            ],
        ),
        member(
            "bayan-engine",
            Some(GPL),
            &[on("bayan-model", "normal"), on("bayan-protocol", "build")],
        ),
        member(
            "bayan-lab",
            Some(GPL),
            &[on("bayan-engine", "normal"), on("bayan-units", "normal")],
        ),
    ]
}

fn refused(members: &[String], table: &[Layer]) -> String {
    check(&metadata(members), table).unwrap_err()
}

#[test]
fn accepts_a_workspace_that_keeps_the_rules() {
    let line = check(&metadata(&good()), &TABLE).unwrap();
    assert_eq!(
        line,
        "layers: the 6 members stand in 4 of the 5 layers of LAYERS, and every normal and build dependency between them points to the member's own layer or below; only bayan-crdt depends on Loro; the one member that is not under the GPL depends on none that is"
    );
}

#[test]
fn refuses_a_table_that_is_not_right() {
    // A member without a layer, an entry that is no member, a member listed twice, a layer listed twice, and a layer without a name.
    let mut members = good();
    members.push(member("bayan-text", Some(GPL), &[]));
    let table: [Layer; 5] = [
        (
            "Foundation",
            &["bayan-units", "bayan-protocol", "bayan-units"],
        ),
        (
            "Model and formats",
            &["bayan-crdt", "bayan-model", "bayan-gone"],
        ),
        ("Engine", &["bayan-engine"]),
        ("Engine", &[]),
        (" ", &["bayan-lab"]),
    ];
    let problems = refused(&members, &table);
    for expected in [
        "  `bayan-units` is listed twice",
        "  `bayan-gone` (layer `Model and formats`) is not a workspace member; update the list",
        "  the layer `Engine` is listed twice",
        "  a layer has no name",
        "  `bayan-text` stands in no layer: add it to its layer in LAYERS, after reading architecture §4",
    ] {
        assert!(problems.contains(expected), "{expected}\n{problems}");
    }
    // The table's own problems are reported before anything else is judged.
    assert!(!problems.contains("higher layer"), "{problems}");
    // Without bayan-crdt, the rule about Loro would name a crate that does not exist.
    let without_crdt: Vec<String> = good()
        .into_iter()
        .filter(|text| !text.starts_with(r#"{"name":"bayan-crdt""#))
        .collect();
    let table: [Layer; 2] = [
        ("Foundation", &["bayan-units", "bayan-protocol"]),
        (
            "Model and formats",
            &["bayan-model", "bayan-engine", "bayan-lab"],
        ),
    ];
    assert!(
        refused(&without_crdt, &table)
            .contains("`bayan-crdt`, the only member that may use Loro, is not a workspace member")
    );
}

#[test]
fn refuses_a_dependency_on_a_higher_layer() {
    let mut members = good();
    // A normal and a build dependency upward fail; the dev-dependency of bayan-model on bayan-engine in good() does not.
    members[0] = member(
        "bayan-units",
        Some(GPL),
        &[on("bayan-engine", "normal"), on("bayan-lab", "build")],
    );
    let problems = refused(&members, &TABLE);
    assert_eq!(
        problems
            .lines()
            .filter(|line| line.starts_with("  "))
            .collect::<Vec<_>>(),
        [
            "  `bayan-units` (Foundation) depends on `bayan-engine` (Engine), a higher layer: a crate may depend only on crates of its own layer or below",
            "  `bayan-units` (Foundation) depends on `bayan-lab` (Fidelity Lab), a higher layer: a crate may depend only on crates of its own layer or below",
        ]
    );
    // A crate of the engine may not depend on the lab, which stands above every layer of the architecture.
    let mut members = good();
    members[4] = member("bayan-engine", Some(GPL), &[on("bayan-lab", "normal")]);
    assert!(
        refused(&members, &TABLE).contains(
            "`bayan-engine` (Engine) depends on `bayan-lab` (Fidelity Lab), a higher layer"
        )
    );
    // Within a layer, dependencies go either way.
    let mut members = good();
    members[2] = member(
        "bayan-crdt",
        Some(GPL),
        &[
            dependency("loro", "normal", None),
            on("bayan-model", "normal"),
        ],
    );
    check(&metadata(&members), &TABLE).unwrap();
}

#[test]
fn refuses_loro_outside_bayan_crdt() {
    let mut members = good();
    members[3] = member(
        "bayan-model",
        Some(GPL),
        &[
            on("bayan-crdt", "normal"),
            dependency("loro", "normal", None),
        ],
    );
    members[4] = member(
        "bayan-engine",
        Some(GPL),
        &[
            dependency("loro-internal", "build", None),
            dependency("loro_fractional_index", "normal", None),
            // Only Loro's own crates count, not every name that starts alike.
            dependency("lorem", "normal", None),
        ],
    );
    let problems = refused(&members, &TABLE);
    for expected in [
        "  `bayan-model` depends on `loro`: only bayan-crdt may use Loro",
        "  `bayan-engine` depends on `loro-internal`: only bayan-crdt may use Loro",
        "  `bayan-engine` depends on `loro_fractional_index`: only bayan-crdt may use Loro",
    ] {
        assert!(problems.contains(expected), "{expected}\n{problems}");
    }
    assert!(!problems.contains("lorem"), "{problems}");
}

#[test]
fn refuses_gpl_code_under_a_crate_that_is_not_under_the_gpl() {
    let apache = |dependencies: &[String]| {
        let mut members = good();
        members[1] = member("bayan-protocol", Some("Apache-2.0"), dependencies);
        members
    };
    // Directly, with every kind of its own dependencies, its tests' included.
    for kind in ["normal", "build", "dev"] {
        let problems = refused(&apache(&[on("bayan-units", kind)]), &TABLE);
        assert!(
            problems.contains("  `bayan-protocol` (Apache-2.0) depends on `bayan-units` (GPL-3.0-or-later WITH AdditionRef-BayanDocs-App-Store-Permission): a crate that is not under the GPL may never depend on code that is"),
            "{kind}: {problems}"
        );
    }
    // Through a member that is not under the GPL either.
    let mut members = apache(&[on("bayan-helper", "normal")]);
    members.push(member(
        "bayan-helper",
        Some("MIT OR Apache-2.0"),
        &[on("bayan-units", "build")],
    ));
    let table: [Layer; 1] = [(
        "Foundation",
        &[
            "bayan-units",
            "bayan-protocol",
            "bayan-helper",
            "bayan-crdt",
            "bayan-model",
            "bayan-engine",
            "bayan-lab",
        ],
    )];
    let problems = refused(&members, &table);
    assert!(
        problems.contains("  `bayan-protocol` (Apache-2.0) depends through `bayan-helper` on `bayan-units` (GPL-3.0-or-later"),
        "{problems}"
    );
    assert!(
        problems.contains("  `bayan-helper` (MIT OR Apache-2.0) depends on `bayan-units`"),
        "{problems}"
    );
    // A member without a license cannot be judged.
    let mut members = good();
    members[0] = member("bayan-units", None, &[]);
    assert!(
        refused(&members, &TABLE).contains("  `bayan-units` states no license in its manifest")
    );
    // Nor can one under the GPL family's other licenses be depended on.
    let mut members = apache(&[on("bayan-units", "normal")]);
    members[0] = member("bayan-units", Some("LGPL-2.1-or-later"), &[]);
    assert!(refused(&members, &TABLE).contains("depends on `bayan-units` (LGPL-2.1-or-later)"));
}

#[test]
fn refuses_a_path_dependency_outside_the_workspace() {
    let mut members = good();
    members[0] = member(
        "bayan-units",
        Some(GPL),
        &[dependency("fuzzed", "normal", Some("fuzz"))],
    );
    assert!(
        refused(&members, &TABLE).contains(
            "  `bayan-units` depends by path on `fuzzed`, which is not a workspace member"
        )
    );
}

#[test]
fn refuses_metadata_it_cannot_follow() {
    let without_kind = metadata(&good()).replacen(r#""kind":null"#, r#""kind":"weird""#, 1);
    assert!(
        check(&without_kind, &TABLE)
            .unwrap_err()
            .contains("has no `kind` that Cargo writes")
    );
    let without_license = metadata(&good()).replacen(r#""license":"Apache-2.0","#, "", 1);
    assert!(
        check(&without_license, &TABLE)
            .unwrap_err()
            .contains("no `license` field for bayan-protocol")
    );
    assert!(check(r#"{"packages":[]}"#, &TABLE).is_err());
    assert!(check("not json", &TABLE).is_err());
}
