//! A reader of the GitHub Actions workflows in `.github/workflows/`, for the tests of `verify.rs` that check them: the events that start a workflow, its jobs, each job's steps, and the commands of their scripts. It reads YAML in the block style that the workflows use, at any indentation, and refuses every form it cannot read instead of passing over it, because a test that cannot see a job passes a workflow that breaks the test's rule (the review of bayan-core#61): flow style for the events, the jobs or the steps (`on: [push]`, `steps: [...]`), a line that is no key where a key must stand, keys of one mapping at different indentations, anchors, aliases and merge keys, and tabs in the indentation.
//!
//! What the tests still cannot see: a compiler that a program of the repository or of a tool runs, other than `scripts/dev-setup.sh` (`python3 fuzz/ci.py` runs `cargo fuzz`, and `make`, `npm run` or a script file could run anything); a command that an expression `${{ … }}` writes into a script; and the steps of a local action (`uses: ./…`), which count as building for that reason. So review still has to see that no job runs such a program before the supply-chain checks.

/// A line of a workflow that holds something, with the lines indented below it. Blank lines and lines that hold only a comment are left out, also inside scripts, where `#` starts a comment too.
#[derive(Debug)]
pub struct Node<'a> {
    /// The line's number, from 1.
    pub number: usize,
    /// Its indentation, in spaces.
    pub indent: usize,
    /// Its text, without the indentation and the white space at its end.
    pub text: &'a str,
    /// The lines below it that stand further in, up to the next line that does not.
    pub children: Vec<Node<'a>>,
}

/// The outline of a workflow: its lines that hold something, each with the lines indented below it.
///
/// # Errors
///
/// A tab in the indentation, which YAML does not allow.
pub fn outline(workflow: &str) -> Result<Vec<Node<'_>>, String> {
    let mut lines = Vec::new();
    for (index, line) in workflow.lines().enumerate() {
        let content = line.trim_start_matches(' ');
        if content.trim().is_empty() || content.starts_with('#') {
            continue;
        }
        if content.starts_with('\t') {
            return Err(format!(
                "line {}: a tab in the indentation, which YAML does not allow",
                index + 1
            ));
        }
        lines.push((index + 1, line.len() - content.len(), content.trim_end()));
    }
    Ok(tree(&lines))
}

fn tree<'a>(lines: &[(usize, usize, &'a str)]) -> Vec<Node<'a>> {
    let mut nodes = Vec::new();
    let mut rest = lines;
    while let Some((&(number, indent, text), after)) = rest.split_first() {
        let end = after
            .iter()
            .position(|&(_, other, _)| other <= indent)
            .unwrap_or(after.len());
        let (below, next) = after.split_at(end);
        nodes.push(Node {
            number,
            indent,
            text,
            children: tree(below),
        });
        rest = next;
    }
    nodes
}

/// A key of a mapping in block style.
#[derive(Clone, Copy, Debug)]
pub struct Key<'n, 'a> {
    /// Its name, without quotes.
    pub name: &'a str,
    /// What follows its colon on the same line, without a comment after it: empty when its value is the block below.
    pub value: &'a str,
    /// Its line's number.
    pub number: usize,
    /// The lines of its value below it.
    pub below: &'n [Node<'a>],
}

/// Splits `name: value` or `name:` into the name, without quotes, and the value, without a comment after it.
fn split(text: &str) -> Option<(&str, &str)> {
    let (name, rest) = match text.chars().next()? {
        quote @ ('"' | '\'') => {
            let end = text.get(1..)?.find(quote)? + 1;
            (text.get(1..end)?, text.get(end + 1..)?.strip_prefix(':')?)
        }
        _ => {
            let end = text
                .find(": ")
                .or_else(|| text.ends_with(':').then(|| text.len() - 1))?;
            (text.get(..end)?, text.get(end + 1..)?)
        }
    };
    // `- name: value` is an item of a sequence, not a key.
    if name.is_empty() || name == "-" || name.starts_with("- ") {
        return None;
    }
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let value = rest.trim();
    let value = if value.starts_with('#') {
        ""
    } else {
        value.split(" #").next().unwrap_or(value).trim_end()
    };
    Some((name, value))
}

fn key<'n, 'a>(
    context: &str,
    number: usize,
    text: &'a str,
    below: &'n [Node<'a>],
) -> Result<Key<'n, 'a>, String> {
    let (name, value) = split(text).ok_or_else(|| {
        format!(
            "{context}, line {number}: `{text}` is not a key of a mapping; this test cannot read it"
        )
    })?;
    let named = |sigil: char| {
        value
            .strip_prefix(sigil)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_alphanumeric()))
    };
    if name == "<<" || named('*') || named('&') {
        return Err(format!(
            "{context}, line {number}: an anchor, an alias or a merge key, which this test does not follow"
        ));
    }
    Ok(Key {
        name,
        value,
        number,
        below,
    })
}

/// The keys of the mapping that `nodes` hold, which must all stand at one indentation.
///
/// # Errors
///
/// A line that is no key, a key at another indentation than the first, an anchor, an alias or a merge key; `context` names the mapping.
pub fn keys<'n, 'a>(context: &str, nodes: &'n [Node<'a>]) -> Result<Vec<Key<'n, 'a>>, String> {
    let indent = nodes.first().map(|node| node.indent);
    nodes
        .iter()
        .map(|node| {
            if Some(node.indent) != indent {
                return Err(format!(
                    "{context}, line {}: a key at another indentation than the first; this test cannot read it",
                    node.number
                ));
            }
            key(context, node.number, node.text, &node.children)
        })
        .collect()
}

/// The items of the sequence of mappings that `nodes` hold, which must all stand at one indentation, each as its keys.
///
/// # Errors
///
/// A line that is no item where an item must stand, or no key where a key must stand, and every error of [`keys`].
pub fn items<'n, 'a>(
    context: &str,
    nodes: &'n [Node<'a>],
) -> Result<Vec<Vec<Key<'n, 'a>>>, String> {
    let indent = nodes.first().map(|node| node.indent);
    let mut items = Vec::new();
    for node in nodes {
        let content = node
            .text
            .strip_prefix('-')
            .filter(|rest| rest.starts_with(' ') && Some(node.indent) == indent)
            .map(str::trim_start)
            .ok_or_else(|| {
                format!(
                    "{context}, line {}: `{}` is not an item of the sequence at its indentation; this test cannot read it",
                    node.number, node.text
                )
            })?;
        // The item's keys stand at the column of its first one; what stands further in below the item's line belongs to the first key.
        let column = node.indent + node.text.len() - content.len();
        let first = node
            .children
            .iter()
            .position(|child| child.indent <= column)
            .unwrap_or(node.children.len());
        let (value, others) = node.children.split_at(first);
        let mut keys = vec![key(context, node.number, content, value)?];
        for child in others {
            if child.indent != column {
                return Err(format!(
                    "{context}, line {}: a key at another indentation than the item's first; this test cannot read it",
                    child.number
                ));
            }
            keys.push(key(context, child.number, child.text, &child.children)?);
        }
        items.push(keys);
    }
    Ok(items)
}

/// The words of a list: a flow sequence on the key's line (`[a, b]`), a single word or words between commas there, or a block sequence below it (`- a`).
fn list<'a>(context: &str, key: &Key<'_, 'a>) -> Result<Vec<&'a str>, String> {
    let unquote = |word: &'a str| word.trim().trim_matches(['"', '\'']);
    if !key.value.is_empty() {
        let inner = key
            .value
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
            .unwrap_or(key.value);
        return Ok(inner
            .split(',')
            .map(unquote)
            .filter(|word| !word.is_empty())
            .collect());
    }
    key.below
        .iter()
        .map(|node| {
            node.text.strip_prefix("- ").map(unquote).ok_or_else(|| {
                format!(
                    "{context}, line {}: `{}` is not an item of a list; this test cannot read it",
                    node.number, node.text
                )
            })
        })
        .collect()
}

/// The key `name` of the workflow's top level, which must open a block, once.
fn root<'n, 'a>(file: &str, outline: &'n [Node<'a>], name: &str) -> Result<Key<'n, 'a>, String> {
    let found: Vec<Key<'n, 'a>> = keys(file, outline)?
        .into_iter()
        .filter(|key| key.name == name)
        .collect();
    match found.as_slice() {
        [key] if key.value.is_empty() && !key.below.is_empty() => Ok(*key),
        [] => Err(format!("{file} has no `{name}:`; this test cannot read it")),
        _ => Err(format!(
            "{file}: `{name}:` must open a block below it, once; this test cannot read it"
        )),
    }
}

/// The events that start the workflow (`on:`), each as its key.
///
/// # Errors
///
/// No `on:` that opens a block of keys.
pub fn events<'n, 'a>(file: &str, outline: &'n [Node<'a>]) -> Result<Vec<Key<'n, 'a>>, String> {
    keys(&format!("{file}, `on:`"), root(file, outline, "on")?.below)
}

/// The jobs of the workflow, each as its identifier and its keys.
///
/// # Errors
///
/// No `jobs:` that opens a block, and a job that is not a block of keys.
pub fn jobs<'n, 'a>(
    file: &str,
    outline: &'n [Node<'a>],
) -> Result<Vec<(&'a str, Vec<Key<'n, 'a>>)>, String> {
    keys(
        &format!("{file}, `jobs:`"),
        root(file, outline, "jobs")?.below,
    )?
    .into_iter()
    .map(|job| {
        let context = format!("{file}, job `{}`", job.name);
        if !job.value.is_empty() || job.below.is_empty() {
            return Err(format!(
                "{context}: a job must be a block of keys; this test cannot read it"
            ));
        }
        Ok((job.name, keys(&context, job.below)?))
    })
    .collect()
}

/// Whether the workflow runs on every pull request, without a path filter: `Some` with whether it also runs in the merge queue (`merge_group` with `types: [checks_requested]`), or `None`.
///
/// # Errors
///
/// An `on:` that this test cannot read.
pub fn on_every_pull_request(file: &str, workflow: &str) -> Result<Option<bool>, String> {
    let outline = outline(workflow).map_err(|problem| format!("{file}, {problem}"))?;
    let events = events(file, &outline)?;
    let Some(pull_request) = events.iter().find(|event| event.name == "pull_request") else {
        return Ok(None);
    };
    if !pull_request.value.is_empty() {
        return Err(format!(
            "{file}, line {}: `pull_request:` with a value on its line; this test cannot read it",
            pull_request.number
        ));
    }
    let filters = keys(&format!("{file}, `pull_request:`"), pull_request.below)?;
    if filters
        .iter()
        .any(|filter| matches!(filter.name, "paths" | "paths-ignore"))
    {
        return Ok(None);
    }
    let Some(merge_group) = events.iter().find(|event| event.name == "merge_group") else {
        return Ok(Some(false));
    };
    let types = keys(&format!("{file}, `merge_group:`"), merge_group.below)?;
    Ok(Some(types.iter().any(|key| {
        key.name == "types" && list(file, key).is_ok_and(|types| types == ["checks_requested"])
    })))
}

/// The simple commands of a shell script, each as its words without quotes. The script is cut at line ends and at `;`, `&`, `|`, `(`, `)`, `{`, `}` and backticks outside quotes, which covers `&&`, `||` and `$(…)`, and each piece at white space outside quotes; a backslash at the end of a line continues it, and `#` at the start of a word starts a comment. A quoted string that holds white space, such as the command in `bash -c "cargo build"`, is read as a script of its own too, which also counts the words of a message as commands: that errs on the side of finding a build.
pub fn commands(script: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut nested = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    let mut quoted = String::new();
    let mut characters = script.chars().peekable();
    let end_word = |words: &mut Vec<String>, word: &mut String, started: &mut bool| {
        if *started {
            words.push(std::mem::take(word));
            *started = false;
        }
    };
    while let Some(character) = characters.next() {
        if let Some(open) = quote {
            match character {
                _ if character == open => {
                    if quoted.contains(char::is_whitespace) {
                        nested.push(std::mem::take(&mut quoted));
                    }
                    quoted.clear();
                    quote = None;
                }
                '\\' if open == '"' => {
                    if let Some(next) = characters.next() {
                        word.push(next);
                        quoted.push(next);
                    }
                }
                _ => {
                    word.push(character);
                    quoted.push(character);
                }
            }
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                started = true;
            }
            // A backslash continues the line before a line end, and is kept elsewhere: PowerShell, the default shell on Windows, writes paths with it (`C:\\…\\cargo.exe`).
            '\\' if characters.next_if_eq(&'\n').is_some() => {
                end_word(&mut words, &mut word, &mut started);
            }
            '#' if !started => while characters.next_if(|next| *next != '\n').is_some() {},
            '\n' | ';' | '&' | '|' | '(' | ')' | '{' | '}' | '`' => {
                end_word(&mut words, &mut word, &mut started);
                if !words.is_empty() {
                    commands.push(std::mem::take(&mut words));
                }
            }
            _ if character.is_whitespace() => end_word(&mut words, &mut word, &mut started),
            _ => {
                word.push(character);
                started = true;
            }
        }
    }
    end_word(&mut words, &mut word, &mut started);
    if !words.is_empty() {
        commands.push(words);
    }
    for script in nested {
        commands.extend(self::commands(&script));
    }
    commands
}

/// The programs of Rust's toolchain that compile code, which runs the build scripts and procedural macros of the dependencies.
const COMPILERS: [&str; 7] = [
    "cargo",
    "rustc",
    "rustdoc",
    "cargo-clippy",
    "clippy-driver",
    "cargo-fuzz",
    "cargo-miri",
];

/// The arguments with which Cargo or rustc may run before the supply-chain checks, because they build nothing but xtask, which has no dependencies: the checks themselves, the gate, whose first step they are, and the versions.
const BEFORE_THE_CHECKS: [&[&str]; 5] = [
    &["xtask", "check-exact-pins"],
    &["xtask", "check-lockfile-age"],
    &["xtask", "verify"],
    &["--version"],
    &["deny", "--version"],
];

/// The program that a word names: its file name, without a folder or `.exe`.
fn program(word: &str) -> &str {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// Whether a word sets a variable for the command it precedes (`NAME=value`).
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether a simple command only looks a program up by its name, without running it: `command -v` or `-V`, `which` or `type`.
fn looks_up(words: &[String]) -> bool {
    let mut words = words
        .iter()
        .map(String::as_str)
        .skip_while(|word| is_assignment(word));
    match words.next() {
        Some("which" | "type") => true,
        Some("command") => matches!(words.next(), Some("-v" | "-V")),
        _ => false,
    }
}

/// Whether a simple command builds or runs code of the dependencies: it names `dev-setup.sh` anywhere, which can build tools from crates.io, or a compiler of [`COMPILERS`] anywhere, unless it only looks the compiler up ([`looks_up`]) or the compiler is the command itself (after the variables it sets, and with a toolchain such as `+nightly` or none) and its arguments are one of [`BEFORE_THE_CHECKS`].
pub fn builds(words: &[String]) -> bool {
    if words.iter().any(|word| program(word) == "dev-setup.sh") {
        return true;
    }
    if looks_up(words) {
        return false;
    }
    let Some(at) = words
        .iter()
        .position(|word| COMPILERS.contains(&program(word)))
    else {
        return false;
    };
    if words.iter().position(|word| !is_assignment(word)) != Some(at) {
        return true;
    }
    let mut arguments = words.get(at + 1..).unwrap_or_default();
    if let Some((toolchain, rest)) = arguments.split_first()
        && toolchain.starts_with('+')
    {
        arguments = rest;
    }
    !BEFORE_THE_CHECKS.iter().any(|allowed| {
        arguments
            .iter()
            .map(String::as_str)
            .eq(allowed.iter().copied())
    })
}

/// The script of a `run:` key: the lines of a block scalar (`|`, `>`) one by one, or a plain or quoted scalar folded into one line.
fn script(run: &Key<'_, '_>) -> String {
    fn lines<'a>(nodes: &[Node<'a>], out: &mut Vec<&'a str>) {
        for node in nodes {
            out.push(node.text);
            lines(&node.children, out);
        }
    }
    let mut below = Vec::new();
    lines(run.below, &mut below);
    if run.value.starts_with(['|', '>']) {
        return below.join("\n");
    }
    let value = run.value;
    let value = ['"', '\'']
        .iter()
        .find_map(|quote| value.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(value);
    std::iter::once(value)
        .chain(below)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The CodeQL languages of a step's `with:`, read through the job's matrix where the step names one (`${{ matrix.language }}`).
fn languages<'a>(
    context: &str,
    job: &[Key<'_, 'a>],
    with: &[Key<'_, 'a>],
) -> Result<Vec<&'a str>, String> {
    let Some(languages) = with.iter().find(|key| key.name == "languages") else {
        return Ok(Vec::new());
    };
    if !languages.value.contains("${{") {
        return list(context, languages);
    }
    let name = languages
        .value
        .strip_prefix("${{")
        .and_then(|value| value.strip_suffix("}}"))
        .and_then(|value| value.trim().strip_prefix("matrix."))
        .ok_or_else(|| {
            format!(
                "{context}, line {}: languages from an expression other than one value of the job's matrix; this test cannot read it",
                languages.number
            )
        })?;
    let unreadable = || {
        format!(
            "{context}, line {}: the job's `strategy:` has no `matrix:` with `{name}:` as a list, or a matrix with `include:` or `exclude:`; this test cannot read it",
            languages.number
        )
    };
    let strategy = job
        .iter()
        .find(|key| key.name == "strategy")
        .ok_or_else(unreadable)?;
    let matrix = keys(context, strategy.below)?
        .into_iter()
        .find(|key| key.name == "matrix")
        .ok_or_else(unreadable)?;
    let values = keys(context, matrix.below)?;
    if values
        .iter()
        .any(|key| matches!(key.name, "include" | "exclude"))
    {
        return Err(unreadable());
    }
    let values = values
        .iter()
        .find(|key| key.name == name)
        .ok_or_else(unreadable)?;
    list(context, values)
}

/// What a step builds, if it builds: a command of its script that [`builds`], CodeQL's analysis of Rust, whose extractor runs `cargo check`, or a local action, whose steps this test does not read.
fn what_builds(
    context: &str,
    job: &[Key<'_, '_>],
    step: &[Key<'_, '_>],
) -> Result<Option<String>, String> {
    let get = |name: &str| step.iter().find(|key| key.name == name);
    if let Some(uses) = get("uses")
        && uses.value.starts_with("./")
    {
        return Ok(Some(format!("uses: {}", uses.value)));
    }
    if let Some(with) = get("with") {
        let with = keys(context, with.below)?;
        if languages(context, job, &with)?.contains(&"rust") {
            return Ok(Some("CodeQL's analysis of Rust".to_owned()));
        }
    }
    let Some(run) = get("run") else {
        return Ok(None);
    };
    Ok(commands(&script(run))
        .into_iter()
        .find(|words| builds(words))
        .map(|words| words.join(" ")))
}

/// The jobs of a workflow that build, as `<file> <job>`, and the problems of their order: every job of every workflow that builds or runs code of the dependencies runs `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age` as steps of their own, without a condition and not allowed to fail, after a checkout of the full history that the age check needs and before anything that builds; and no step that builds runs when an earlier step failed (`always()`, `failure()` or `cancelled()` in its condition), which could be one of the checks.
///
/// # Errors
///
/// A workflow that this test cannot read.
pub fn supply_chain_order(
    file: &str,
    workflow: &str,
) -> Result<(Vec<String>, Vec<String>), String> {
    let outline = outline(workflow).map_err(|problem| format!("{file}, {problem}"))?;
    let mut building = Vec::new();
    let mut problems = Vec::new();
    for (job, keys_of_job) in jobs(file, &outline)? {
        let context = format!("{file}, job `{job}`");
        let Some(steps) = keys_of_job.iter().find(|key| key.name == "steps") else {
            continue;
        };
        if !steps.value.is_empty() || steps.below.is_empty() {
            return Err(format!(
                "{context}, line {}: `steps:` must open a block of items below it; this test cannot read it",
                steps.number
            ));
        }
        let (mut full_history, mut pins, mut age, mut builds_here) = (false, false, false, false);
        for step in items(&context, steps.below)? {
            let get = |name: &str| step.iter().find(|key| key.name == name);
            let line = step.first().map_or(0, |key| key.number);
            if let Some(with) = get("with")
                && keys(&context, with.below)?
                    .iter()
                    .any(|key| key.name == "fetch-depth" && key.value == "0")
            {
                full_history = true;
            }
            let check = get("run").map(script);
            let check = check.as_deref().map(str::trim);
            if let Some(
                check @ ("cargo xtask check-exact-pins" | "cargo xtask check-lockfile-age"),
            ) = check
            {
                if let Some(condition) = get("if") {
                    problems.push(format!("{context}, line {}: `{check}` runs only `if: {}`, so a job that builds can run without it", condition.number, condition.value));
                }
                if let Some(allowed) = get("continue-on-error")
                    && allowed.value != "false"
                {
                    problems.push(format!("{context}, line {}: `{check}` may fail without stopping the job (`continue-on-error: {}`)", allowed.number, allowed.value));
                }
                if check.ends_with("check-lockfile-age") {
                    if !full_history {
                        problems.push(format!("{context}, line {line}: `{check}` runs before a checkout with `fetch-depth: 0`"));
                    }
                    age = true;
                } else {
                    pins = true;
                }
                continue;
            }
            let Some(what) = what_builds(&context, &keys_of_job, &step)? else {
                continue;
            };
            if !(pins && age) && !builds_here {
                problems.push(format!("{context}, line {line}: `{what}` runs before `cargo xtask check-exact-pins` and `cargo xtask check-lockfile-age`"));
            }
            if let Some(condition) = get("if")
                && ["always()", "failure()", "cancelled()"]
                    .iter()
                    .any(|status| condition.value.contains(status))
            {
                problems.push(format!("{context}, line {}: `{what}` runs `if: {}`, also when an earlier step, such as a supply-chain check, failed", condition.number, condition.value));
            }
            if !builds_here {
                building.push(format!("{file} {job}"));
                builds_here = true;
            }
        }
    }
    Ok((building, problems))
}

#[cfg(test)]
mod tests;
