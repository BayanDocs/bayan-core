//! The lint canaries: deliberately broken code in `xtask/lint-canary` that must be rejected (CORE-001, acceptance criterion AC-2).
//!
//! Each case is compiled on its own by passing `--cfg bayan_lint_canary="<case>"` to the compiler. A configuration mistake that silently switches a rule off, such as a misspelled path in `clippy.toml` (which Clippy ignores without a word), makes a canary compile cleanly, and the gate fails.

use std::path::Path;

use crate::policy::ClippyConfig;
use crate::{json, process};

/// Which program compiles a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// `cargo clippy … -- -D warnings`: for the Clippy rules.
    Clippy,
    /// `cargo build`: for the rules the compiler itself enforces, which make code fail to compile at all.
    Build,
}

/// One case of `xtask/lint-canary/src/lib.rs`.
pub struct Case {
    /// The value of `bayan_lint_canary`, which is also the module name.
    pub name: &'static str,
    /// The program that must reject (or accept) it.
    pub tool: Tool,
}

/// Every case, in the order they run.
pub const CASES: [Case; 7] = [
    Case {
        name: "disallowed_methods",
        tool: Tool::Clippy,
    },
    Case {
        name: "disallowed_types",
        tool: Tool::Clippy,
    },
    Case {
        name: "expect_with_reason",
        tool: Tool::Clippy,
    },
    Case {
        name: "expect_without_reason",
        tool: Tool::Clippy,
    },
    Case {
        name: "allow_with_reason",
        tool: Tool::Clippy,
    },
    Case {
        name: "unsafe_block",
        tool: Tool::Build,
    },
    Case {
        name: "unsafe_expect",
        tool: Tool::Build,
    },
];

/// What compiling a case produced.
pub struct Outcome {
    /// Whether the program succeeded.
    pub success: bool,
    /// Everything it printed: Cargo's JSON messages and its error output.
    pub text: String,
}

/// Compiles every case and checks the result, printing one line per case.
pub fn check_all(root: &Path, clippy: &ClippyConfig) -> Result<(), String> {
    // The canaries have their own build directory, so they never disturb the normal build. It is emptied first: Cargo replays the result of an earlier compilation when it believes nothing changed, and a guardrail must not depend on that belief.
    let target_dir = root.join("target").join("lint-canary");
    match std::fs::remove_dir_all(&target_dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("cannot empty {}: {error}", target_dir.display()));
        }
        _ => {}
    }
    for case in &CASES {
        let outcome = compile(root, &target_dir, case)?;
        match judge(case.name, &outcome, clippy) {
            Ok(summary) => println!("    ok: canary `{}`: {summary}", case.name),
            Err(problem) => {
                let diagnostics = json::string_values(&outcome.text, "rendered").concat();
                return Err(format!(
                    "lint canary `{}`: {problem}\nReproduce with: {}\nWhat the compiler said:\n{}",
                    case.name,
                    reproduction(case),
                    if diagnostics.is_empty() {
                        &outcome.text
                    } else {
                        &diagnostics
                    }
                ));
            }
        }
    }
    Ok(())
}

/// Compiles one case in the canaries' build directory.
fn compile(root: &Path, target_dir: &Path, case: &Case) -> Result<Outcome, String> {
    let mut command = process::cargo(root);
    command.arg(match case.tool {
        Tool::Clippy => "clippy",
        Tool::Build => "build",
    });
    command
        .args([
            "--package",
            "lint-canary",
            "--locked",
            "--message-format=json",
            "--target-dir",
        ])
        .arg(target_dir);
    if case.tool == Tool::Clippy {
        command.args(["--", "-D", "warnings"]);
    }
    // CARGO_ENCODED_RUSTFLAGS takes precedence over every other way of passing compiler flags, so nothing in the environment can interfere. Its arguments are separated by the 0x1F character.
    command
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            format!("--cfg\u{1f}bayan_lint_canary=\"{}\"", case.name),
        )
        .env_remove("RUSTFLAGS");
    let output = process::capture(&mut command)?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(Outcome {
        success: output.status.success(),
        text,
    })
}

/// Decides whether a case behaved as required. Returns a one-line summary, or what went wrong.
pub fn judge(case: &str, outcome: &Outcome, clippy: &ClippyConfig) -> Result<String, String> {
    match case {
        "disallowed_methods" => {
            rejected(outcome, "clippy::disallowed_methods")?;
            every_path_reported(outcome, "use of a disallowed method", &clippy.methods)?;
            mentions(
                outcome,
                "f64::sin(x)",
                "the path-style call `f64::sin(x)` was not reported",
            )?;
            mentions(
                outcome,
                "bayan-units",
                "the reason does not point to bayan-units",
            )?;
            Ok(format!(
                "Clippy rejected all {} disallowed methods, including a path-style call, pointing to bayan-units",
                clippy.methods.len()
            ))
        }
        "disallowed_types" => {
            rejected(outcome, "clippy::disallowed_types")?;
            every_path_reported(outcome, "use of a disallowed type", &clippy.types)?;
            Ok(format!(
                "Clippy rejected all {} disallowed types",
                clippy.types.len()
            ))
        }
        "expect_with_reason" => {
            if outcome.success {
                Ok("Clippy accepted the sanctioned exception `#[expect(clippy::disallowed_types, reason = \"…\")]`".to_owned())
            } else {
                Err("Clippy rejected the sanctioned exception `#[expect(…, reason = \"…\")]`, which must be accepted".to_owned())
            }
        }
        "expect_without_reason" => {
            rejected(outcome, "clippy::allow_attributes_without_reason")?;
            Ok("Clippy rejected an exception without a reason".to_owned())
        }
        "allow_with_reason" => {
            rejected(outcome, "clippy::allow_attributes")?;
            Ok(
                "Clippy rejected an exception written as `#[allow]` instead of `#[expect]`"
                    .to_owned(),
            )
        }
        "unsafe_block" => {
            rejected(outcome, "unsafe_code")?;
            Ok("the compiler refused to build an `unsafe` block in a non-binding crate".to_owned())
        }
        "unsafe_expect" => {
            rejected(outcome, "E0453")?;
            Ok("the compiler refused to let one function lift the crate-wide `forbid(unsafe_code)`".to_owned())
        }
        other => Err(format!("unknown canary case `{other}`")),
    }
}

/// The case failed to compile, and the diagnostic code `code` (a lint name or an error number) was among the reasons.
fn rejected(outcome: &Outcome, code: &str) -> Result<(), String> {
    if outcome.success {
        return Err(format!(
            "it compiled, but `{code}` must reject it; is the rule still configured?"
        ));
    }
    mentions(
        outcome,
        &format!("\"code\":{{\"code\":\"{code}\""),
        &format!("it failed, but not because of `{code}`"),
    )
}

/// Every configured path produced its own diagnostic, such as "use of a disallowed method `f64::sin`".
fn every_path_reported(outcome: &Outcome, message: &str, paths: &[String]) -> Result<(), String> {
    let missing: Vec<&str> = paths
        .iter()
        .filter(|path| !outcome.text.contains(&format!("{message} `{path}`")))
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "no diagnostic for {}; the path may be misspelled in clippy.toml, or the canary does not use it yet",
            missing.join(", ")
        ))
    }
}

fn mentions(outcome: &Outcome, text: &str, problem: &str) -> Result<(), String> {
    if outcome.text.contains(text) {
        Ok(())
    } else {
        Err(problem.to_owned())
    }
}

/// A command a person can paste into a shell to see a case's diagnostics.
fn reproduction(case: &Case) -> String {
    let tool = match case.tool {
        Tool::Clippy => "clippy",
        Tool::Build => "build",
    };
    let lints = if case.tool == Tool::Clippy {
        " -- -D warnings"
    } else {
        ""
    };
    format!(
        "RUSTFLAGS='--cfg bayan_lint_canary=\"{}\"' cargo {tool} -p lint-canary --target-dir target/lint-canary{lints}",
        case.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clippy() -> ClippyConfig {
        ClippyConfig {
            methods: vec!["f32::sin".to_owned(), "f64::sin".to_owned()],
            types: vec!["std::collections::HashMap".to_owned()],
        }
    }

    fn failed(text: &str) -> Outcome {
        Outcome {
            success: false,
            text: text.to_owned(),
        }
    }

    const METHODS_REJECTED: &str = r#"{"message":{"code":{"code":"clippy::disallowed_methods","explanation":null},"message":"use of a disallowed method `f32::sin`"}}
{"message":{"code":{"code":"clippy::disallowed_methods","explanation":null},"message":"use of a disallowed method `f64::sin`","children":[{"message":"… use bayan-units …"}],"rendered":"    f64::sin(x)"}}"#;

    #[test]
    fn every_case_exists_in_the_canary_crate() {
        // Git on Windows may check files out with CRLF line endings.
        let source = include_str!("../lint-canary/src/lib.rs").replace("\r\n", "\n");
        for case in &CASES {
            assert!(
                source.contains(&format!(
                    "#[cfg(bayan_lint_canary = \"{}\")]\npub mod {};",
                    case.name, case.name
                )),
                "xtask/lint-canary/src/lib.rs has no module for the case `{}`",
                case.name
            );
        }
    }

    #[test]
    fn every_case_has_a_verdict() {
        for case in &CASES {
            let verdict = judge(case.name, &failed(""), &clippy());
            assert!(
                verdict
                    .as_ref()
                    .err()
                    .is_none_or(|problem| !problem.starts_with("unknown canary case")),
                "no verdict for `{}`",
                case.name
            );
        }
    }

    #[test]
    fn accepts_a_complete_rejection_of_the_disallowed_methods() {
        assert!(judge("disallowed_methods", &failed(METHODS_REJECTED), &clippy()).is_ok());
    }

    #[test]
    fn notices_a_disallowed_method_that_did_not_trigger() {
        let text = METHODS_REJECTED.replace("`f32::sin`", "`f32::cos`");
        let problem = judge("disallowed_methods", &failed(&text), &clippy()).unwrap_err();
        assert!(problem.contains("no diagnostic for f32::sin"), "{problem}");
    }

    #[test]
    fn notices_a_canary_that_compiled() {
        let outcome = Outcome {
            success: true,
            text: String::new(),
        };
        let problem = judge("unsafe_block", &outcome, &clippy()).unwrap_err();
        assert!(problem.contains("it compiled"), "{problem}");
    }

    #[test]
    fn notices_a_canary_that_failed_for_another_reason() {
        let outcome = failed(r#"{"message":{"code":{"code":"E0425","explanation":null}}}"#);
        let problem = judge("unsafe_block", &outcome, &clippy()).unwrap_err();
        assert!(
            problem.contains("not because of `unsafe_code`"),
            "{problem}"
        );
    }

    #[test]
    fn requires_the_sanctioned_exception_to_compile() {
        assert!(
            judge(
                "expect_with_reason",
                &Outcome {
                    success: true,
                    text: String::new()
                },
                &clippy()
            )
            .is_ok()
        );
        assert!(judge("expect_with_reason", &failed(""), &clippy()).is_err());
    }

    #[test]
    fn shows_how_to_reproduce_a_case() {
        assert_eq!(
            reproduction(&CASES[5]),
            "RUSTFLAGS='--cfg bayan_lint_canary=\"unsafe_block\"' cargo build -p lint-canary --target-dir target/lint-canary"
        );
    }
}
