//! Reads what Cargo prints for an audit run with `--message-format=json`: every lint occurrence that the compiler reported for a unit of a workspace member, where it reported it, and which macro wrote the code there, and each unit of a member that Cargo compiled or found unchanged.
//!
//! Every line must be one of Cargo's JSON messages of the kinds it reads (`compiler-message`, `compiler-artifact`, `build-script-executed` and, last, `{"reason":"build-finished","success":true}`), and every diagnostic must be readable: a warning without a lint name, an error, a message for a package that is not a member, a lint occurrence without a primary span, and a member's target that is compiled for another edition than the workspace's, whose default lints the audit force-warns, are errors, never a pass.

use std::path::Path;

use crate::json::{self, Value};
use crate::policy::Package;

use super::guarded;

/// A position that the compiler names in a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Span {
    /// The file as the compiler names it: relative to its working folder, which is the workspace root for members, or absolute.
    pub file: String,
    /// The position of the first byte, in the file as it is on disk.
    pub byte: usize,
    /// The line, counting from 1.
    pub line: usize,
    /// The column, counting from 1.
    pub column: usize,
    /// The expansion of the macro that wrote the code there, if a macro wrote it.
    pub expansion: Option<Box<MacroCall>>,
}

/// One expansion of a macro, as the compiler describes it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacroCall {
    /// The macro's name as the compiler shows it, such as `exempt!` or `#[derive(Debug)]`.
    pub name: String,
    /// Where the macro is defined, if the compiler says: for a `macro_rules!` macro, the position of its `macro_rules` token.
    pub definition: Option<(String, usize)>,
    /// Where it was called.
    pub call_site: Span,
}

/// A target of a workspace member, a library, binary, example, test or benchmark: the member, the target's name, its kinds and its root file as Cargo names it (`src_path`).
pub type TargetKey = (String, String, Vec<String>, String);

/// What Cargo printed for one run.
#[derive(Debug, Default)]
pub struct Run {
    /// The lint occurrences, in the order Cargo printed them.
    pub reported: Vec<Reported>,
    /// The units of the workspace members that Cargo compiled or found unchanged.
    pub units: Vec<Unit>,
}

/// One unit of a workspace member, from the `compiler-artifact` message that Cargo prints when it has compiled the unit or found it unchanged.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unit {
    /// The unit's target.
    pub target: TargetKey,
    /// Whether the unit is its target compiled as tests (`profile.test`).
    pub test: bool,
    /// The files it produced (`filenames`), which tell it from every other unit: each unit of a target has files of its own.
    pub filenames: Vec<String>,
    /// Whether Cargo found the unit unchanged and replayed the messages of its last compilation (`fresh`), instead of compiling it.
    pub fresh: bool,
}

/// One lint occurrence that the compiler reported for a unit of a workspace member.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Reported {
    /// The member.
    pub member: String,
    /// The name of the unit's target.
    pub target: String,
    /// The target's kinds, such as `lib`, `bin` or `test`.
    pub kinds: Vec<String>,
    /// The target's root file, as Cargo names it (`src_path`).
    pub root: String,
    /// The lint, as the compiler names it: `clippy::disallowed_types`, `unused_must_use`.
    pub lint: String,
    /// The lint that the occurrence calls unknown, if it is one of the compiler's `unknown lint: `…`` warnings.
    pub unknown: Option<String>,
    /// The primary spans, at least one.
    pub spans: Vec<Span>,
}

impl Reported {
    /// Whether the unit is always compiled as tests: an integration test or a benchmark. The library and binaries are compiled both ways, and examples without tests.
    pub fn always_tests(&self) -> bool {
        self.kinds
            .iter()
            .any(|kind| kind == "test" || kind == "bench")
    }

    /// The target of the unit that reported it.
    pub fn target_key(&self) -> TargetKey {
        (
            self.member.clone(),
            self.target.clone(),
            self.kinds.clone(),
            self.root.clone(),
        )
    }
}

/// Reads what Cargo printed on standard output for one run with `--message-format=json`, `text`, for the workspace members `members`, whose targets must all be compiled for `edition`.
pub fn read(text: &str, members: &[Package], edition: &str) -> Result<Run, String> {
    let mut run = Run::default();
    let mut finished = None;
    for line in text.lines() {
        let message = json::parse(line).map_err(|error| {
            format!("Cargo printed a line that is not one of its JSON messages ({error}): {line}")
        })?;
        let reason = message
            .get("reason")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Cargo printed a message without a `reason`: {line}"))?;
        if finished.is_some() {
            return Err(format!(
                "Cargo printed a message after `build-finished`: {line}"
            ));
        }
        match reason {
            "build-finished" => finished = Some(message.get("success") == Some(&Value::Bool(true))),
            "compiler-message" => {
                if let Some(occurrence) = occurrence(&message, members, edition)
                    .map_err(|problem| format!("{problem}: {line}"))?
                {
                    run.reported.push(occurrence);
                }
            }
            "compiler-artifact" => {
                if let Some(unit) = unit(&message, members, edition)
                    .map_err(|problem| format!("{problem}: {line}"))?
                {
                    run.units.push(unit);
                }
            }
            // A dependency's build script, which carries no diagnostic: no workspace member has one.
            "build-script-executed" => {}
            other => {
                return Err(format!(
                    "Cargo printed a message of the kind `{other}`, which the lint audit does not read: {line}"
                ));
            }
        }
    }
    match finished {
        Some(true) => Ok(run),
        Some(false) => Err("the run failed (`build-finished` reports no success)".to_owned()),
        None => Err("the run did not finish: Cargo printed no `build-finished` message".to_owned()),
    }
}

/// The member's target that `message`, a message of the kind `what`, names, or `None` if it names a package that is not a workspace member. Fails if the message cannot say which target it names, and if a member's target is compiled for another edition than `edition`, because the audit force-warns the lints that rustc turns on by default for that edition only.
fn member_target(
    message: &Value,
    members: &[Package],
    edition: &str,
    what: &str,
) -> Result<Option<TargetKey>, String> {
    let manifest = message
        .get("manifest_path")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what} without a `manifest_path`"))?;
    let folder = Path::new(manifest).parent();
    let Some(member) = members
        .iter()
        .find(|member| Some(member.folder.as_path()) == folder)
    else {
        return Ok(None);
    };
    let target = message
        .get("target")
        .ok_or_else(|| format!("{what} without a target"))?;
    let name = target
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what} whose target has no name"))?;
    let kinds =
        strings(target.get("kind")).ok_or_else(|| format!("{what} whose target has no kind"))?;
    let root = target
        .get("src_path")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what} whose target has no `src_path`"))?;
    let compiled_for = target
        .get("edition")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{what} whose target has no `edition`"))?;
    if compiled_for != edition {
        return Err(format!(
            "the target `{name}` of {} is compiled for edition {compiled_for}, but the audit force-warns the lints that rustc turns on by default for edition {edition}, the edition of the root Cargo.toml; give the member the workspace's edition, or teach the audit the default lints of each edition",
            member.name
        ));
    }
    Ok(Some((
        member.name.clone(),
        name.to_owned(),
        kinds,
        root.to_owned(),
    )))
}

/// The unit that a `compiler-artifact` message describes, `None` for a package that is not a workspace member, or an error for a member's unit that the message does not describe in full.
fn unit(message: &Value, members: &[Package], edition: &str) -> Result<Option<Unit>, String> {
    let Some(target) = member_target(message, members, edition, "an artifact message")? else {
        return Ok(None);
    };
    let Some(Value::Bool(test)) = message
        .get("profile")
        .and_then(|profile| profile.get("test"))
    else {
        return Err(
            "an artifact message whose profile does not say whether the unit is a test".to_owned(),
        );
    };
    let filenames =
        strings(message.get("filenames")).ok_or("an artifact message without `filenames`")?;
    let Some(Value::Bool(fresh)) = message.get("fresh") else {
        return Err("an artifact message without `fresh`".to_owned());
    };
    Ok(Some(Unit {
        target,
        test: *test,
        filenames,
        fresh: *fresh,
    }))
}

/// The lint occurrence that a `compiler-message` describes, `None` for the notes that a failure brings, or an error for anything that the audit cannot judge.
fn occurrence(
    message: &Value,
    members: &[Package],
    edition: &str,
) -> Result<Option<Reported>, String> {
    let (member, name, kinds, root) =
        member_target(message, members, edition, "a compiler message")?
            .ok_or("a compiler message for a package that is not a workspace member")?;
    let diagnostic = message
        .get("message")
        .ok_or("a compiler message without a diagnostic")?;
    match diagnostic.get("$message_type").and_then(Value::as_str) {
        Some("diagnostic") => {}
        Some(other) => {
            return Err(format!(
                "a compiler message of the kind `{other}`, which the audit does not read"
            ));
        }
        None => return Err("a compiler message without `$message_type`".to_owned()),
    }
    let level = diagnostic
        .get("level")
        .and_then(Value::as_str)
        .ok_or("a diagnostic without a level")?;
    match level {
        "warning" => {}
        "note" | "help" | "failure-note" => return Ok(None),
        "error" => return Err("the compiler reported an error".to_owned()),
        other => return Err(format!("a diagnostic of the unknown level `{other}`")),
    }
    let lint = diagnostic
        .get("code")
        .and_then(|code| code.get("code"))
        .and_then(Value::as_str)
        .ok_or("a warning without a lint name, which the audit cannot judge")?;
    let mut spans = Vec::new();
    for span in diagnostic
        .get("spans")
        .and_then(Value::as_array)
        .ok_or("a diagnostic without `spans`")?
    {
        if span.get("is_primary") == Some(&Value::Bool(true)) {
            spans.push(read_span(span, 0)?);
        }
    }
    if spans.is_empty() {
        return Err(format!(
            "an occurrence of `{lint}` without a position, which the audit cannot judge"
        ));
    }
    Ok(Some(Reported {
        member,
        target: name,
        kinds,
        root,
        lint: lint.to_owned(),
        unknown: guarded::unknown_lint(diagnostic)?,
        spans,
    }))
}

/// How many macro expansions deep a span may be described; the compiler stops long before.
const MAX_EXPANSIONS: usize = 64;

/// Reads one span of a diagnostic, with the chain of macro expansions that wrote the code there.
fn read_span(span: &Value, depth: usize) -> Result<Span, String> {
    if depth > MAX_EXPANSIONS {
        return Err("a span inside too many macro expansions".to_owned());
    }
    let file = span
        .get("file_name")
        .and_then(Value::as_str)
        .ok_or("a span without a `file_name`")?;
    let expansion = match span.get("expansion") {
        None | Some(Value::Null) => None,
        Some(expansion) => {
            let name = expansion
                .get("macro_decl_name")
                .and_then(Value::as_str)
                .ok_or("a macro expansion without a `macro_decl_name`")?;
            let call_site = expansion
                .get("span")
                .ok_or("a macro expansion without its call site")?;
            let definition = match expansion.get("def_site_span") {
                None | Some(Value::Null) => None,
                Some(definition) => Some((
                    definition
                        .get("file_name")
                        .and_then(Value::as_str)
                        .ok_or("a macro definition without a `file_name`")?
                        .to_owned(),
                    number(definition, "byte_start")?,
                )),
            };
            Some(Box::new(MacroCall {
                name: name.to_owned(),
                definition,
                call_site: read_span(call_site, depth + 1)?,
            }))
        }
    };
    Ok(Span {
        file: file.to_owned(),
        byte: number(span, "byte_start")?,
        line: number(span, "line_start")?,
        column: number(span, "column_start")?,
        expansion,
    })
}

/// The unsigned number in the field `key` of `value`.
fn number(value: &Value, key: &str) -> Result<usize, String> {
    match value.get(key) {
        Some(Value::Number(digits)) => digits
            .parse()
            .map_err(|_| format!("`{key}` is not an unsigned number: {digits}")),
        _ => Err(format!("a span without `{key}`")),
    }
}

/// The strings of `value`, if it is an array of strings.
fn strings(value: Option<&Value>) -> Option<Vec<String>> {
    value?
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_owned))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Target;
    use std::path::PathBuf;

    /// A member at `/repo/crates/a`.
    fn members() -> Vec<Package> {
        vec![Package {
            name: "a".to_owned(),
            folder: PathBuf::from("/repo/crates/a"),
            targets: vec![Target {
                kinds: vec!["lib".to_owned()],
                name: "a".to_owned(),
                root_file: PathBuf::from("/repo/crates/a/src/lib.rs"),
            }],
        }]
    }

    /// A message as Cargo 1.99 prints it for a force-warned `clippy::disallowed_types` in code that the macro `exempt!` writes, shortened to the fields the audit reads and their neighbours.
    const MAP_IN_MACRO: &str = r#"{"reason":"compiler-message","package_id":"path+file:///repo/crates/a#0.0.0","manifest_path":"/repo/crates/a/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"a","src_path":"/repo/crates/a/src/lib.rs","edition":"2024","doc":true,"doctest":true,"test":true},"message":{"rendered":"warning: use of a disallowed type","$message_type":"diagnostic","children":[{"children":[],"code":null,"level":"note","message":"requested on the command line with `--force-warn clippy::disallowed-types`","rendered":null,"spans":[]}],"code":{"code":"clippy::disallowed_types","explanation":null},"level":"warning","message":"use of a disallowed type `std::collections::HashMap`","spans":[{"byte_end":240,"byte_start":215,"column_end":111,"column_start":86,"expansion":{"def_site_span":{"byte_end":30,"byte_start":11,"column_end":20,"column_start":1,"expansion":null,"file_name":"crates/a/src/lib.rs","is_primary":false,"label":null,"line_end":3,"line_start":3,"suggested_replacement":null,"suggestion_applicability":null,"text":[]},"macro_decl_name":"exempt!","span":{"byte_end":329,"byte_start":266,"column_end":64,"column_start":1,"expansion":null,"file_name":"crates/a/src/lib.rs","is_primary":false,"label":null,"line_end":5,"line_start":5,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}},"file_name":"crates/a/src/lib.rs","is_primary":true,"label":null,"line_end":4,"line_start":4,"suggested_replacement":null,"suggestion_applicability":null,"text":[]},{"byte_end":9,"byte_start":1,"column_end":9,"column_start":1,"expansion":null,"file_name":"crates/a/src/lib.rs","is_primary":false,"label":null,"line_end":1,"line_start":1,"suggested_replacement":null,"suggestion_applicability":null,"text":[]}]}}"#;

    const FINISHED: &str = r#"{"reason":"build-finished","success":true}"#;

    /// The `compiler-artifact` message of the member's library, compiled as itself and found unchanged, as Cargo 1.99 prints it, shortened.
    const LIBRARY: &str = r#"{"reason":"compiler-artifact","package_id":"path+file:///repo/crates/a#0.0.0","manifest_path":"/repo/crates/a/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"a","src_path":"/repo/crates/a/src/lib.rs","edition":"2024","doc":true,"doctest":true,"test":true},"profile":{"opt_level":"0","debuginfo":2,"debug_assertions":true,"overflow_checks":true,"test":false},"features":[],"filenames":["/repo/target/debug/deps/liba-48f2a89a8c4a8ccd.rmeta"],"executable":null,"fresh":true}"#;

    /// A dependency's `compiler-artifact` message, which the audit leaves out.
    const DEPENDENCY: &str = r#"{"reason":"compiler-artifact","package_id":"registry+https://github.com/rust-lang/crates.io-index#memchr@2.7.6","manifest_path":"/cargo/registry/src/memchr-2.7.6/Cargo.toml","target":{"kind":["lib"],"crate_types":["lib"],"name":"memchr","src_path":"/cargo/registry/src/memchr-2.7.6/src/lib.rs","edition":"2021","doc":true,"doctest":true,"test":true},"profile":{"test":false},"features":[],"filenames":["/repo/target/debug/deps/libmemchr-1.rmeta"],"executable":null,"fresh":true}"#;

    #[test]
    fn reads_an_occurrence_and_the_macro_that_wrote_it() {
        let script = r#"{"reason":"build-script-executed","package_id":"registry+https://github.com/rust-lang/crates.io-index#serde@1.0.228","linked_libs":[],"linked_paths":[],"cfgs":[],"env":[],"out_dir":"/repo/target/debug/build/serde-1/out"}"#;
        let text = format!("{DEPENDENCY}\n{script}\n{LIBRARY}\n{MAP_IN_MACRO}\n{FINISHED}\n");
        let run = read(&text, &members(), "2024").unwrap();
        let reported = run.reported;
        assert_eq!(reported.len(), 1);
        let occurrence = &reported[0];
        assert_eq!(
            (
                occurrence.member.as_str(),
                occurrence.target.as_str(),
                occurrence.root.as_str(),
                occurrence.lint.as_str()
            ),
            (
                "a",
                "a",
                "/repo/crates/a/src/lib.rs",
                "clippy::disallowed_types"
            )
        );
        assert_eq!(occurrence.kinds, ["lib"]);
        assert!(!occurrence.always_tests());
        assert_eq!(occurrence.unknown, None);
        // Only the primary span.
        assert_eq!(occurrence.spans.len(), 1);
        let span = &occurrence.spans[0];
        assert_eq!(
            (span.file.as_str(), span.byte, span.line, span.column),
            ("crates/a/src/lib.rs", 215, 4, 86)
        );
        let call = span.expansion.as_deref().unwrap();
        assert_eq!(call.name, "exempt!");
        assert_eq!(
            call.definition,
            Some(("crates/a/src/lib.rs".to_owned(), 11))
        );
        assert_eq!(
            (
                call.call_site.byte,
                call.call_site.line,
                call.call_site.column
            ),
            (266, 5, 1)
        );
        assert!(call.call_site.expansion.is_none());
        // The member's unit, not the dependency's.
        assert_eq!(
            run.units,
            [Unit {
                target: (
                    "a".to_owned(),
                    "a".to_owned(),
                    vec!["lib".to_owned()],
                    "/repo/crates/a/src/lib.rs".to_owned()
                ),
                test: false,
                filenames: vec!["/repo/target/debug/deps/liba-48f2a89a8c4a8ccd.rmeta".to_owned()],
                fresh: true,
            }]
        );
        assert_eq!(occurrence.target_key(), run.units[0].target);
    }

    #[test]
    fn leaves_out_the_notes_of_a_failure() {
        let note = MAP_IN_MACRO.replace(r#""level":"warning""#, r#""level":"failure-note""#);
        assert!(
            read(&format!("{note}\n{FINISHED}"), &members(), "2024")
                .unwrap()
                .reported
                .is_empty()
        );
    }

    #[test]
    fn refuses_what_it_cannot_judge() {
        let replaced = |from: &str, to: &str| {
            assert!(MAP_IN_MACRO.contains(from), "{from}");
            format!("{}\n{FINISHED}", MAP_IN_MACRO.replacen(from, to, 1))
        };
        for (text, problem) in [
            (
                format!("{MAP_IN_MACRO}\nwarning: text\n{FINISHED}"),
                "not one of its JSON messages",
            ),
            ("{}\n".to_owned() + FINISHED, "without a `reason`"),
            (MAP_IN_MACRO.to_owned(), "did not finish"),
            (
                format!(
                    "{MAP_IN_MACRO}\n{}",
                    r#"{"reason":"build-finished","success":false}"#
                ),
                "the run failed",
            ),
            (
                format!("{FINISHED}\n{MAP_IN_MACRO}"),
                "after `build-finished`",
            ),
            (
                replaced(r#""level":"warning""#, r#""level":"error""#),
                "reported an error",
            ),
            (
                replaced(r#""level":"warning""#, r#""level":"strange""#),
                "unknown level",
            ),
            (
                replaced("/repo/crates/a/Cargo.toml", "/elsewhere/b/Cargo.toml"),
                "not a workspace member",
            ),
            (
                replaced(
                    r#""code":{"code":"clippy::disallowed_types","explanation":null}"#,
                    r#""code":null"#,
                ),
                "without a lint name",
            ),
            (
                replaced(r#""is_primary":true"#, r#""is_primary":false"#),
                "without a position",
            ),
            (
                replaced(r#""byte_start":215"#, r#""byte_start":-1"#),
                "not an unsigned number",
            ),
            (
                replaced(r#""macro_decl_name":"exempt!","#, ""),
                "without a `macro_decl_name`",
            ),
            (
                replaced(r#""edition":"2024""#, r#""edition":"2021""#),
                "compiled for edition 2021, but the audit force-warns the lints that rustc turns on by default for edition 2024",
            ),
            (
                format!(
                    "{}\n{FINISHED}",
                    LIBRARY.replace(r#""edition":"2024""#, r#""edition":"2021""#)
                ),
                "compiled for edition 2021",
            ),
            (
                format!("{}\n{FINISHED}", LIBRARY.replace(r#","test":false}"#, "}")),
                "does not say whether the unit is a test",
            ),
            (
                format!("{}\n{FINISHED}", LIBRARY.replace(r#","fresh":true"#, "")),
                "without `fresh`",
            ),
            (
                format!(
                    "{}\n{FINISHED}",
                    r#"{"reason":"timing-info","package_id":"a"}"#
                ),
                "of the kind `timing-info`, which the lint audit does not read",
            ),
            (
                replaced(
                    r#""$message_type":"diagnostic""#,
                    r#""$message_type":"artifact""#,
                ),
                "of the kind `artifact`",
            ),
            (
                replaced(r#""$message_type":"diagnostic","#, ""),
                "without `$message_type`",
            ),
            (
                format!(
                    "{}\n{FINISHED}",
                    LIBRARY.replace(
                        r#""filenames":["/repo/target/debug/deps/liba-48f2a89a8c4a8ccd.rmeta"],"#,
                        ""
                    )
                ),
                "without `filenames`",
            ),
        ] {
            let error = read(&text, &members(), "2024").unwrap_err();
            assert!(error.contains(problem), "{problem}: {error}");
        }
    }
}
