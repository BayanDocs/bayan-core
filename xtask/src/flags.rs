//! The preflight check on compiler flags and unstable features.
//!
//! The lint rules only work if the compiler applies them. Some compiler flags lower lint levels for every crate at once: `--cap-lints=allow` even lowers `forbid`, which switches off the float bans, the hash-collection bans and the unsafe policy (ADR-0005, ADR-0006 §2) without a word. Flags reach the compiler from the environment and from `rustflags` in Cargo's configuration files, so the gate reads both before it builds anything and rejects the dangerous ones. Flags that do not touch lint levels, such as `-C target-feature=+simd128` or `--cfg …`, stay allowed.
//!
//! The same goes for programs that stand in for the compiler: a wrapper (`RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`, `[build] rustc-wrapper`) or a replacement compiler (`RUSTC`, `[build] rustc`) sees every compiler call and can add or drop flags for one chosen crate only, which neither this check nor the lint canary would see. The gate therefore rejects them too.
//!
//! The lint canary covers what this check cannot read, such as Cargo configuration files outside the repository: it is compiled with the same flags, so a flag that weakens a rule it exercises makes it compile, and the gate fails (`canary.rs`).

use std::path::Path;

use crate::toml_subset;

/// The Cargo configuration files of the repository that can set compiler flags, relative to its root. Cargo reads `.cargo/config` (without extension) too, and prefers it when both exist.
pub const CONFIG_FILES: [&str; 2] = [".cargo/config.toml", ".cargo/config"];

/// Environment variables that unlock unstable compiler features on the stable toolchain.
const UNSTABLE_VARIABLES: [&str; 1] = ["RUSTC_BOOTSTRAP"];

/// Environment variables that make Cargo run another program in place of, or around, the compiler. The `CARGO_BUILD_…` names are the environment forms of the `[build]` settings in [`COMPILER_SETTINGS`].
const COMPILER_VARIABLES: [&str; 6] = [
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTC",
    "CARGO_BUILD_RUSTC_WRAPPER",
    "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTC",
];

/// The settings of the `[build]` table that do the same as [`COMPILER_VARIABLES`].
const COMPILER_SETTINGS: [&str; 3] = ["rustc-wrapper", "rustc-workspace-wrapper", "rustc"];

/// Why a wrapper or replacement compiler is rejected.
const COMPILER_REASON: &str = "Cargo would run that program in place of, or around, the compiler for every crate. It sees every compiler call and can add or drop flags for one chosen crate only, for example `--cap-lints=allow`, where neither this check nor the lint canary would notice";

/// Why a compiler argument is rejected, or `None` if it is allowed.
///
/// rustc accepts no abbreviated long options and does not combine short flags (checked with Rust 1.99), so these spellings are all there are: `--cap-lints allow` and `--cap-lints=allow`, `-A x`, `-Ax`, `--allow x` and `--allow=x`, and so on.
pub fn rejection(arg: &str) -> Option<&'static str> {
    let long = |name: &str| arg == name || arg.starts_with(&format!("{name}="));
    if long("--cap-lints") {
        Some(
            "caps the level of every lint, `forbid` included, so `--cap-lints=allow` switches off every ban and the unsafe policy",
        )
    } else if arg.starts_with("-A") || long("--allow") {
        Some("lowers a lint to \"allow\", switching off a rule that is not forbidden")
    } else if long("--force-warn") {
        Some("turns a lint into a warning that nothing can raise again, even a forbidden one")
    } else if arg.starts_with("-Z") {
        Some(
            "is an unstable compiler option; with RUSTC_BOOTSTRAP it could, for example, add an exception to every crate (`-Zcrate-attr`)",
        )
    } else if arg.starts_with('@') {
        Some("reads more compiler arguments from a file, which this check cannot see")
    } else {
        None
    }
}

/// Checks the compiler flags from the environment (`env`, every variable as a name and a value) and from the repository's Cargo configuration files (`configs`, each as its path relative to the root and its text). Returns one line describing what was checked, or every problem found.
pub fn check(env: &[(String, String)], configs: &[(String, String)]) -> Result<String, String> {
    let mut problems = Vec::new();
    for (name, value) in env {
        if UNSTABLE_VARIABLES.contains(&name.as_str()) {
            problems.push(format!(
                "the environment variable {name} is set (to `{value}`). It unlocks unstable compiler features on the stable toolchain, such as `-Zcrate-attr`, which adds an attribute to every crate, and `core::intrinsics`, whose platform math clippy.toml does not list. Unset it, or remove it from the [env] table of .cargo/config.toml."
            ));
        }
        if COMPILER_VARIABLES.contains(&name.as_str()) && !value.is_empty() {
            problems.push(format!(
                "the environment variable {name} is set (to `{value}`). {COMPILER_REASON}. Unset it."
            ));
        }
        for arg in env_flags(name, value) {
            if let Some(why) = rejection(&arg) {
                problems.push(format!(
                    "the environment variable {name} passes `{arg}` to the compiler, which {why}"
                ));
            }
        }
    }
    for (path, text) in configs {
        let settings = config_settings(text).map_err(|error| {
            format!("{path}: {error}. The gate reads compiler flags and compiler settings from this file and must understand it; write `rustflags` in a [build] or [target.…] table, as a string or an array of strings, and `rustc`, `rustc-wrapper` and `rustc-workspace-wrapper` in [build], as strings.")
        })?;
        for (key, value) in settings.compilers {
            problems.push(format!(
                "{path}: `{key}` in [build] is set (to `{value}`). {COMPILER_REASON}. Remove the setting."
            ));
        }
        for (location, args) in settings.flags {
            for arg in args {
                if let Some(why) = rejection(&arg) {
                    problems.push(format!(
                        "{path}: `rustflags` in {location} passes `{arg}` to the compiler, which {why}"
                    ));
                }
            }
        }
    }
    if problems.is_empty() {
        return Ok("no compiler flag lowers lint levels or unlocks unstable features, and no wrapper or replacement compiler is set (environment and .cargo/config.toml checked; RUSTC_BOOTSTRAP is not set)".to_owned());
    }
    Err(format!(
        "{}\nThe lint rules of this repository (ADR-0005, ADR-0006 §2) only work if no compiler flag lowers lint levels or unlocks unstable features, so the gate rejects `--cap-lints`, `-A`/`--allow`, `--force-warn`, `-Z` options and `@file` arguments in RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS, CARGO_BUILD_RUSTFLAGS, CARGO_TARGET_<TRIPLE>_RUSTFLAGS and the `rustflags` of .cargo/config.toml, and the RUSTC_BOOTSTRAP variable. For the same reason it rejects compiler wrappers and replacement compilers: RUSTC_WRAPPER, RUSTC_WORKSPACE_WRAPPER, RUSTC (and their CARGO_BUILD_… forms), and `rustc-wrapper`, `rustc-workspace-wrapper` and `rustc` in the [build] table. Remove them and run the gate again. Flags that do not change lint levels, such as `-C target-feature=…`, are fine.",
        problems.join("\n")
    ))
}

/// The compiler arguments an environment variable passes, if it is one Cargo reads compiler flags from: `RUSTFLAGS` (separated by spaces), `CARGO_ENCODED_RUSTFLAGS` (separated by the 0x1F character), and the configuration variables `CARGO_BUILD_RUSTFLAGS` and `CARGO_TARGET_<TRIPLE>_RUSTFLAGS` (and any other `CARGO_…_RUSTFLAGS`).
fn env_flags(name: &str, value: &str) -> Vec<String> {
    if name == "CARGO_ENCODED_RUSTFLAGS" {
        value
            .split('\u{1f}')
            .filter(|arg| !arg.is_empty())
            .map(str::to_owned)
            .collect()
    } else if name == "RUSTFLAGS" || (name.starts_with("CARGO_") && name.ends_with("_RUSTFLAGS")) {
        value.split_whitespace().map(str::to_owned).collect()
    } else {
        Vec::new()
    }
}

/// What a Cargo configuration file sets that this check looks at.
#[derive(Debug, Default, PartialEq, Eq)]
struct Settings {
    /// Every `rustflags` setting, with where it was found and its arguments.
    flags: Vec<(String, Vec<String>)>,
    /// Every non-empty `rustc`, `rustc-wrapper` or `rustc-workspace-wrapper` in `[build]`, as its key and value. (An empty value switches the wrapper off.)
    compilers: Vec<(String, String)>,
}

/// Reads the settings of a Cargo configuration file that this check looks at: every `rustflags`, in `[build]` and in every `[target.…]` table, and the compiler and wrapper settings of `[build]`, also written with dotted keys such as `build.rustflags = …`. A `rustflags` string is split at whitespace, as Cargo does. Anything that could hide such a setting from this reader, such as an inline table `build = { rustflags = … }`, is an error.
fn config_settings(text: &str) -> Result<Settings, String> {
    let mut found = Settings::default();
    for section in toml_subset::sections(text)? {
        let header = if section.header.is_empty() {
            Vec::new()
        } else {
            toml_subset::key_path(&section.header)
                .map_err(|error| format!("line {}: {error}", section.line))?
        };
        for entry in section.entries {
            let mut path = header.clone();
            path.extend(
                toml_subset::key_path(&entry.key)
                    .map_err(|error| format!("line {}: {error}", entry.line))?,
            );
            let parts: Vec<&str> = path.iter().map(String::as_str).collect();
            let location = match parts.as_slice() {
                ["build", key] if COMPILER_SETTINGS.contains(key) => {
                    let value = toml_subset::any_string(&entry.value)
                        .map_err(|error| format!("line {}: {error}", entry.line))?;
                    if !value.is_empty() {
                        found.compilers.push(((*key).to_owned(), value));
                    }
                    continue;
                }
                ["build", "rustflags"] => "[build]".to_owned(),
                ["target", target, "rustflags"] => format!("[target.'{target}']"),
                ["build"] | ["target"] | ["target", _] => {
                    return Err(format!(
                        "line {}: `{}` is not written as a table",
                        entry.line,
                        path.join(".")
                    ));
                }
                _ => continue,
            };
            let args = if entry.value.starts_with('[') {
                toml_subset::strings(&entry.value)
            } else {
                toml_subset::any_string(&entry.value)
                    .map(|flags| flags.split_whitespace().map(str::to_owned).collect())
            }
            .map_err(|error| format!("line {}: {error}", entry.line))?;
            found.flags.push((location, args));
        }
    }
    Ok(found)
}

/// Reads the repository's Cargo configuration files that exist, as pairs of their path relative to `root` and their text.
pub fn read_configs(root: &Path) -> Result<Vec<(String, String)>, String> {
    let mut configs = Vec::new();
    for relative in CONFIG_FILES {
        let path = root.join(relative);
        match std::fs::read_to_string(&path) {
            Ok(text) => configs.push((relative.to_owned(), text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
        }
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn config(text: &str) -> Vec<(String, String)> {
        vec![(".cargo/config.toml".to_owned(), text.to_owned())]
    }

    #[test]
    fn rejects_every_spelling_of_the_dangerous_flags() {
        for arg in [
            "--cap-lints",
            "--cap-lints=allow",
            "--cap-lints=warn",
            "-A",
            "-Aclippy::unwrap_used",
            "-Awarnings",
            "--allow",
            "--allow=clippy::disallowed_methods",
            "--force-warn",
            "--force-warn=unsafe_code",
            "-Z",
            "-Zcrate-attr=expect(clippy::all)",
            "@flags.txt",
        ] {
            assert!(rejection(arg).is_some(), "{arg} must be rejected");
        }
    }

    #[test]
    fn allows_flags_that_keep_lint_levels() {
        for arg in [
            "-D",
            "warnings",
            "-Dwarnings",
            "-W",
            "clippy::pedantic",
            "-F",
            "--deny=warnings",
            "-C",
            "target-feature=+simd128",
            "-Ctarget-cpu=native",
            "--cfg",
            "getrandom_backend=\"wasm_js\"",
            "clippy::allow_attributes",
            "--cap-lintsx",
            "--allowance",
        ] {
            assert_eq!(rejection(arg), None, "{arg} must be allowed");
        }
    }

    #[test]
    fn accepts_a_clean_environment_and_configuration() {
        let line = check(
            &env(&[
                ("PATH", "/usr/bin"),
                ("CARGO_TARGET_DIR", "/tmp/target"),
                ("RUSTFLAGS", "-D warnings"),
                ("CARGO_ENCODED_RUSTFLAGS", ""),
            ]),
            &config(include_str!("../../.cargo/config.toml")),
        )
        .unwrap_or_else(|problem| panic!("{problem}"));
        assert!(line.starts_with("no compiler flag"), "{line}");
    }

    #[test]
    fn checks_every_environment_variable_cargo_reads_flags_from() {
        let cases = [
            (
                "RUSTFLAGS",
                "-Aclippy::unwrap_used",
                "-Aclippy::unwrap_used",
            ),
            ("RUSTFLAGS", "-D warnings  -A  unsafe_code", "-A"),
            (
                "CARGO_ENCODED_RUSTFLAGS",
                "--cfg\u{1f}x\u{1f}--cap-lints=allow",
                "--cap-lints=allow",
            ),
            (
                "CARGO_BUILD_RUSTFLAGS",
                "--force-warn=unsafe_code",
                "--force-warn=unsafe_code",
            ),
            (
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
                "-Zcrate-attr=x",
                "-Zcrate-attr=x",
            ),
            (
                "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS",
                "@args",
                "@args",
            ),
        ];
        for (name, value, flag) in cases {
            let problem = check(&env(&[(name, value)]), &[]).unwrap_err();
            assert!(
                problem.contains(&format!("{name} passes `{flag}`")),
                "{name}={value:?}: {problem}"
            );
        }
        // The encoded form is not split at spaces: one argument with a space is still one argument.
        assert_eq!(
            env_flags("CARGO_ENCODED_RUSTFLAGS", "-C\u{1f}a b"),
            ["-C", "a b"]
        );
        // Variables that do not carry compiler flags are ignored.
        assert!(
            check(
                &env(&[("RUSTDOCFLAGS_NOTE", "-A x"), ("MY_RUSTFLAGS", "-A x")]),
                &[]
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_compiler_wrappers_and_replacements_in_the_environment() {
        for name in COMPILER_VARIABLES {
            let problem = check(&env(&[(name, "/usr/bin/sccache")]), &[]).unwrap_err();
            assert!(
                problem.contains(&format!(
                    "the environment variable {name} is set (to `/usr/bin/sccache`)"
                )) && problem.contains("in place of, or around, the compiler"),
                "{name}: {problem}"
            );
        }
        // An empty value switches a wrapper off, which is fine.
        assert!(check(&env(&[("RUSTC_WRAPPER", "")]), &[]).is_ok());
    }

    #[test]
    fn rejects_compiler_wrappers_and_replacements_in_the_configuration() {
        let cases = [
            (
                "[build]\nrustc-wrapper = \"scripts/wrap.sh\"\n",
                "rustc-wrapper",
                "scripts/wrap.sh",
            ),
            (
                "[build]\nrustc-workspace-wrapper = 'wrap'\n",
                "rustc-workspace-wrapper",
                "wrap",
            ),
            ("[build]\nrustc = \"/opt/rustc\"\n", "rustc", "/opt/rustc"),
            ("build.rustc-wrapper = \"w\"\n", "rustc-wrapper", "w"),
        ];
        for (text, key, value) in cases {
            let problem = check(&[], &config(text)).unwrap_err();
            assert!(
                problem.contains(&format!(
                    ".cargo/config.toml: `{key}` in [build] is set (to `{value}`)"
                )),
                "{text}: {problem}"
            );
        }
        assert!(check(&[], &config("[build]\nrustc-wrapper = \"\"\n")).is_ok());
        let problem = check(&[], &config("[build]\nrustc-wrapper = [\"w\"]\n")).unwrap_err();
        assert!(problem.contains("must understand it"), "{problem}");
    }

    #[test]
    fn rejects_rustc_bootstrap() {
        for value in ["1", "0", ""] {
            let problem = check(&env(&[("RUSTC_BOOTSTRAP", value)]), &[]).unwrap_err();
            assert!(problem.contains("RUSTC_BOOTSTRAP is set"), "{problem}");
        }
    }

    #[test]
    fn checks_rustflags_in_every_table_of_the_configuration() {
        let cases = [
            (
                "[build]\nrustflags = [\"-A\", \"clippy::unwrap_used\"]\n",
                "[build]",
                "-A",
            ),
            (
                "[build]\nrustflags = \"--cap-lints=allow\"\n",
                "[build]",
                "--cap-lints=allow",
            ),
            (
                "[build]\nrustflags = '-Zcrate-attr=x'\n",
                "[build]",
                "-Zcrate-attr=x",
            ),
            (
                "[build]\nrustflags = [\n  \"-C\", \"opt-level=1\", # fine\n  \"--allow=unsafe_code\",\n]\n",
                "[build]",
                "--allow=unsafe_code",
            ),
            ("build.rustflags = [\"-Ax\"]\n", "[build]", "-Ax"),
            (
                "[target.x86_64-unknown-linux-gnu]\nrustflags = [\"--force-warn\", \"unsafe_code\"]\n",
                "[target.'x86_64-unknown-linux-gnu']",
                "--force-warn",
            ),
            (
                "[target.'cfg(all(windows, target_arch = \"x86_64\"))']\nrustflags = [\"@f\"]\n",
                "[target.'cfg(all(windows, target_arch = \"x86_64\"))']",
                "@f",
            ),
            (
                "[target.\"cfg(unix)\"]\nlinker = \"cc\"\nrustflags = \"-C x --cap-lints allow\"\n",
                "[target.'cfg(unix)']",
                "--cap-lints",
            ),
            (
                "[target]\n'cfg(unix)'.rustflags = [\"-Ay\"]\n",
                "[target.'cfg(unix)']",
                "-Ay",
            ),
            (
                "target.wasm32-unknown-unknown.rustflags = [\"-Az\"]\n",
                "[target.'wasm32-unknown-unknown']",
                "-Az",
            ),
        ];
        for (text, location, flag) in cases {
            let problem = check(&[], &config(text)).unwrap_err();
            assert!(
                problem.contains(&format!("`rustflags` in {location} passes `{flag}`")),
                "{text}: {problem}"
            );
        }
    }

    #[test]
    fn allows_harmless_rustflags_and_other_settings() {
        let text = "[alias]\nxtask = \"run --package xtask --locked --\"\n\n[build]\ntarget-dir = \"target\"\nrustflags = [\"-C\", \"debuginfo=1\"]\n\n[target.wasm32-unknown-unknown]\nrustflags = \"-C target-feature=+simd128 --cfg getrandom_backend=\\\"wasm_js\\\"\"\n\n[env]\nMY_VARIABLE = \"-A x\"\n";
        assert!(check(&[], &config(text)).is_ok());
    }

    #[test]
    fn refuses_configurations_it_cannot_read() {
        for text in [
            "build = { rustflags = [\"-Ax\"] }\n",
            "[target]\nx86_64-unknown-linux-gnu = { rustflags = [\"-Ax\"] }\n",
            "target.'cfg(unix)' = { rustflags = [\"-Ax\"] }\n",
            "[build]\nrustflags = \"\"\"-Ax\"\"\"\n",
            "[build]\nrustflags = [1]\n",
            "[build]\nrustflags = true\n",
            "[target.'unclosed]\nrustflags = []\n",
        ] {
            let problem = check(&[], &config(text)).unwrap_err();
            assert!(
                problem.starts_with(".cargo/config.toml: "),
                "{text}: {problem}"
            );
            assert!(problem.contains("must understand it"), "{text}: {problem}");
        }
    }

    #[test]
    fn reports_every_problem_at_once() {
        let problem = check(
            &env(&[("RUSTFLAGS", "-Ax"), ("RUSTC_BOOTSTRAP", "1")]),
            &[
                (
                    ".cargo/config.toml".to_owned(),
                    "[build]\nrustflags = [\"-Ay\"]\n".to_owned(),
                ),
                (
                    ".cargo/config".to_owned(),
                    "[build]\nrustflags = [\"-Az\"]\n".to_owned(),
                ),
            ],
        )
        .unwrap_err();
        for expected in [
            "`-Ax`",
            "RUSTC_BOOTSTRAP",
            ".cargo/config.toml: `rustflags`",
            ".cargo/config: `rustflags`",
        ] {
            assert!(problem.contains(expected), "{expected}: {problem}");
        }
    }
}
