//! The lint canaries: deliberately broken code in `xtask/lint-canary` that must be rejected (CORE-001, acceptance criterion AC-2).
//!
//! Each case is compiled on its own, in its own build folder, by passing `--cfg bayan_lint_canary="<case>"` to the compiler after `--`. That adds the switch without replacing any other compiler flag, so the cases are compiled with the project's real flags, from `.cargo/config.toml`, the Cargo configuration outside the repository and the environment. A configuration mistake that silently switches a rule off, such as a misspelled path in `clippy.toml` (which Clippy ignores without a word) or a flag such as `--cap-lints=allow`, makes a canary compile cleanly, and the gate fails.

use std::path::Path;

use crate::policy::ClippyConfig;
use crate::{json, process};

/// Which program compiles a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// `cargo clippy … -- -D warnings`: for the Clippy rules.
    Clippy,
    /// `cargo rustc --lib`: for the rules the compiler itself enforces, which make code fail to compile at all.
    Rustc,
}

/// One case of `xtask/lint-canary/src/lib.rs`.
pub struct Case {
    /// The value of `bayan_lint_canary`, which is also the module name.
    pub name: &'static str,
    /// The program that must reject (or accept) it.
    pub tool: Tool,
}

/// Every case, in the order they run.
pub const CASES: [Case; 10] = [
    Case {
        name: "disallowed_methods",
        tool: Tool::Clippy,
    },
    Case {
        name: "explicit_clamp",
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
        name: "expect_disallowed_methods",
        tool: Tool::Clippy,
    },
    Case {
        name: "expect_lint_groups",
        tool: Tool::Clippy,
    },
    Case {
        name: "unsafe_block",
        tool: Tool::Rustc,
    },
    Case {
        name: "unsafe_expect",
        tool: Tool::Rustc,
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
pub fn check_all(root: &Path, clippy_config: &ClippyConfig) -> Result<(), String> {
    // The canaries have their own build directory, so they never disturb the normal build, and each case has its own folder in it, so no case can reuse another's result. It is emptied first: Cargo replays the result of an earlier compilation when it believes nothing changed, and a guardrail must not depend on that belief.
    let target_dir = root.join("target").join(CANARY_DIR);
    match std::fs::remove_dir_all(&target_dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("cannot empty {}: {error}", target_dir.display()));
        }
        _ => {}
    }
    for case in &CASES {
        let outcome = compile(root, &target_dir.join(case.name), case)?;
        match judge(case.name, &outcome, clippy_config) {
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

/// The canaries' build directory, below `target/`.
const CANARY_DIR: &str = "lint-canary";

/// Compiles one case in its own build directory.
///
/// The switch `--cfg bayan_lint_canary="<case>"` goes after `--`, so Cargo hands it to the compiler of the canary crate only, in addition to the flags from `.cargo/config.toml`, Cargo's other configuration files and the environment. (Setting `CARGO_ENCODED_RUSTFLAGS` instead, as CORE-001 did, makes Cargo ignore all of those, so a flag that weakens the lints would go unnoticed.)
fn compile(root: &Path, target_dir: &Path, case: &Case) -> Result<Outcome, String> {
    let mut command = process::cargo(root);
    command.args(compile_args(case));
    command
        .args(["--locked", "--message-format=json", "--target-dir"])
        .arg(target_dir)
        .arg("--")
        .args(case_args(case));
    let output = process::capture(&mut command)?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(Outcome {
        success: output.status.success(),
        text,
    })
}

/// The Cargo command and package that compile a case.
fn compile_args(case: &Case) -> &'static [&'static str] {
    match case.tool {
        Tool::Clippy => &["clippy", "--package", "lint-canary"],
        Tool::Rustc => &["rustc", "--package", "lint-canary", "--lib"],
    }
}

/// The arguments after `--`: the switch that selects the case, and for Clippy, warnings as errors as in the gate's Clippy step.
fn case_args(case: &Case) -> Vec<String> {
    let mut args = vec![
        "--cfg".to_owned(),
        format!("bayan_lint_canary=\"{}\"", case.name),
    ];
    if case.tool == Tool::Clippy {
        args.extend(["-D".to_owned(), "warnings".to_owned()]);
    }
    args
}

/// The exceptions to the float bans in a case, as the lint each one names and the unique `reason` it gives; each must be rejected with error E0453.
fn overruled_exceptions(case: &str) -> &'static [(&'static str, &'static str)] {
    match case {
        "expect_disallowed_methods" => &[
            ("clippy::disallowed_methods", "canary: crate"),
            ("clippy::disallowed_methods", "canary: module"),
            ("clippy::disallowed_methods", "canary: item"),
            ("clippy::disallowed_methods", "canary: cfg_attr"),
        ],
        "expect_lint_groups" => &[
            ("clippy::style", "canary: style group"),
            ("clippy::all", "canary: all group"),
        ],
        _ => &[],
    }
}

/// Decides whether a case behaved as required. Returns a one-line summary, or what went wrong.
pub fn judge(
    case: &str,
    outcome: &Outcome,
    clippy_config: &ClippyConfig,
) -> Result<String, String> {
    match case {
        "disallowed_methods" => {
            rejected(outcome, "clippy::disallowed_methods")?;
            every_path_reported(
                outcome,
                "use of a disallowed method",
                &clippy_config.methods,
            )?;
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
                clippy_config.methods.len()
            ))
        }
        "explicit_clamp" => {
            if outcome.success {
                Ok("Clippy accepted the explicit comparison that replaces the forbidden `clamp` (`manual_clamp` is switched off)".to_owned())
            } else {
                Err("Clippy rejected the explicit comparison that replaces the forbidden `f32::clamp` and `f64::clamp`; is `manual_clamp = \"allow\"` still in the workspace lints?".to_owned())
            }
        }
        "disallowed_types" => {
            rejected(outcome, "clippy::disallowed_types")?;
            every_path_reported(outcome, "use of a disallowed type", &clippy_config.types)?;
            Ok(format!(
                "Clippy rejected all {} disallowed types",
                clippy_config.types.len()
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
        "expect_disallowed_methods" | "expect_lint_groups" => {
            let exceptions = overruled_exceptions(case);
            every_exception_overruled(outcome, exceptions)?;
            Ok(format!(
                "Clippy rejected all {} exceptions to the forbidden float bans ({}) with error E0453",
                exceptions.len(),
                exceptions
                    .iter()
                    .map(|(_, reason)| reason.trim_start_matches("canary: "))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
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

/// Every listed exception produced its own error E0453, `expect(<lint>) incompatible with previous forbid`, whose source line carries the exception's unique reason. Checking each attribute, rather than only that E0453 appears somewhere, makes sure that none of them was accepted.
fn every_exception_overruled(outcome: &Outcome, exceptions: &[(&str, &str)]) -> Result<(), String> {
    rejected(outcome, "E0453")?;
    let errors: Vec<&str> = outcome
        .text
        .lines()
        .filter(|line| line.contains("\"code\":{\"code\":\"E0453\""))
        .collect();
    let missing: Vec<String> = exceptions
        .iter()
        .filter(|(lint, reason)| {
            let message = format!("expect({lint}) incompatible with previous forbid");
            let source = format!("reason = \"{reason}\"");
            !errors.iter().any(|line| {
                json::string_values(line, "message").contains(&message)
                    && json::string_values(line, "text")
                        .iter()
                        .any(|text| text.contains(&source))
            })
        })
        .map(|(lint, reason)| format!("`expect({lint}, reason = \"{reason}\")`"))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "no error E0453 for {}; was `disallowed_methods = \"forbid\"` removed from the workspace lints?",
            missing.join(", ")
        ))
    }
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
    let args: Vec<String> = case_args(case)
        .into_iter()
        .map(|arg| {
            if arg.contains('"') {
                format!("'{arg}'")
            } else {
                arg
            }
        })
        .collect();
    format!(
        "cargo {} --target-dir target/{CANARY_DIR}/{} -- {}",
        compile_args(case).join(" "),
        case.name,
        args.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clippy_config() -> ClippyConfig {
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
            let verdict = judge(case.name, &failed(""), &clippy_config());
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
        assert!(
            judge(
                "disallowed_methods",
                &failed(METHODS_REJECTED),
                &clippy_config()
            )
            .is_ok()
        );
    }

    #[test]
    fn notices_a_disallowed_method_that_did_not_trigger() {
        let text = METHODS_REJECTED.replace("`f32::sin`", "`f32::cos`");
        let problem = judge("disallowed_methods", &failed(&text), &clippy_config()).unwrap_err();
        assert!(problem.contains("no diagnostic for f32::sin"), "{problem}");
    }

    #[test]
    fn notices_a_canary_that_compiled() {
        let outcome = Outcome {
            success: true,
            text: String::new(),
        };
        let problem = judge("unsafe_block", &outcome, &clippy_config()).unwrap_err();
        assert!(problem.contains("it compiled"), "{problem}");
    }

    #[test]
    fn notices_a_canary_that_failed_for_another_reason() {
        let outcome = failed(r#"{"message":{"code":{"code":"E0425","explanation":null}}}"#);
        let problem = judge("unsafe_block", &outcome, &clippy_config()).unwrap_err();
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
                &clippy_config()
            )
            .is_ok()
        );
        assert!(judge("expect_with_reason", &failed(""), &clippy_config()).is_err());
    }

    #[test]
    fn requires_the_explicit_clamp_to_compile() {
        let accepted = Outcome {
            success: true,
            text: String::new(),
        };
        assert!(judge("explicit_clamp", &accepted, &clippy_config()).is_ok());
        let problem = judge("explicit_clamp", &failed(""), &clippy_config()).unwrap_err();
        assert!(problem.contains("manual_clamp"), "{problem}");
    }

    #[test]
    fn shows_how_to_reproduce_a_case() {
        let case = CASES
            .iter()
            .find(|case| case.name == "unsafe_block")
            .unwrap();
        assert_eq!(
            reproduction(case),
            "cargo rustc --package lint-canary --lib --target-dir target/lint-canary/unsafe_block -- --cfg 'bayan_lint_canary=\"unsafe_block\"'"
        );
        let case = CASES
            .iter()
            .find(|case| case.name == "expect_lint_groups")
            .unwrap();
        assert_eq!(
            reproduction(case),
            "cargo clippy --package lint-canary --target-dir target/lint-canary/expect_lint_groups -- --cfg 'bayan_lint_canary=\"expect_lint_groups\"' -D warnings"
        );
    }

    /// Cargo's JSON message for an error E0453, shortened to the fields the verdict reads.
    fn overruled(lint: &str, reason: &str) -> String {
        format!(
            r#"{{"reason":"compiler-message","message":{{"rendered":"error[E0453]: …","children":[{{"message":"`forbid` lint level was set on command line"}}],"message":"expect({lint}) incompatible with previous forbid","spans":[{{"text":[{{"highlight_end":73,"text":"    expect({lint}, reason = \"{reason}\")"}}]}}],"code":{{"code":"E0453","explanation":"…"}}}}}}"#
        )
    }

    #[test]
    fn requires_every_exception_to_be_overruled() {
        let all: Vec<String> = overruled_exceptions("expect_disallowed_methods")
            .iter()
            .map(|(lint, reason)| overruled(lint, reason))
            .collect();
        let verdict = judge(
            "expect_disallowed_methods",
            &failed(&all.join("\n")),
            &clippy_config(),
        );
        assert!(verdict.is_ok(), "{verdict:?}");

        // One exception was accepted: the others' errors are not enough.
        let problem = judge(
            "expect_disallowed_methods",
            &failed(&all[..3].join("\n")),
            &clippy_config(),
        )
        .unwrap_err();
        assert!(
            problem.contains("reason = \"canary: cfg_attr\"") && !problem.contains("canary: item"),
            "{problem}"
        );

        // The right error for the wrong lint does not count.
        let wrong_lint = [
            overruled("clippy::style", "canary: style group"),
            overruled("clippy::style", "canary: all group"),
        ]
        .join("\n");
        let problem =
            judge("expect_lint_groups", &failed(&wrong_lint), &clippy_config()).unwrap_err();
        assert!(problem.contains("expect(clippy::all"), "{problem}");
        assert!(!problem.contains("expect(clippy::style"), "{problem}");
    }

    #[test]
    fn notices_exceptions_that_compiled() {
        let compiled = Outcome {
            success: true,
            text: String::new(),
        };
        let problem = judge("expect_lint_groups", &compiled, &clippy_config()).unwrap_err();
        assert!(problem.contains("it compiled"), "{problem}");
    }

    #[test]
    fn every_overruled_exception_is_in_the_canary_crate() {
        let sources = [
            include_str!("../lint-canary/src/lib.rs"),
            include_str!("../lint-canary/src/expect_disallowed_methods.rs"),
            include_str!("../lint-canary/src/expect_lint_groups.rs"),
        ]
        .concat();
        for case in ["expect_disallowed_methods", "expect_lint_groups"] {
            for (lint, reason) in overruled_exceptions(case) {
                let attribute = format!("expect({lint}, reason = \"{reason}\")");
                assert_eq!(
                    sources.matches(&attribute).count(),
                    1,
                    "the canary crate must contain `{attribute}` exactly once, on one line"
                );
            }
        }
    }
}
