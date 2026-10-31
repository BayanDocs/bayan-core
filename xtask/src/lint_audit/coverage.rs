//! The coverage model: which code the lint exceptions written out in full in the source cover.
//!
//! An occurrence that the audit runs report passes only if an `#[expect(<lint>, reason = "…")]` or `#![expect(<lint>, reason = "…")]` written out in full in the source covers the position where the compiler reports it, naming exactly that lint (not a group that contains it), also inside a `cfg_attr` whose condition holds in the unit that reported it. The model reads the source with the tokens of the source check (`sources.rs`), and it may be stricter than the compiler, never looser:
//!
//! - An outer attribute covers what follows it and its other outer attributes. In item or statement position (after `;`, `{`, `}`, an inner attribute, or at the start of a file), that is an item (`fn`, `struct`, `impl`, `mod`, `use`, `static`, `const`, …) or a `let` statement, up to its first `;`, the end of its first `{…}` group or the end of the group around it, whichever comes first. Before another statement, a field or a variant in the definition of a struct, union or enum, or a match arm, it is also no further than its first `,` or a `>` that is not part of `->` or `=>`. Before a parameter of a function or a closure, it is only the parameter's pattern, which also ends before the `:` of its type and at a `|` or `=`: the compiler applies the attribute neither to the type, which belongs to the signature, nor to a closure's body or its other parameters.
//! - Where the model cannot tell how far the compiler applies an outer attribute, it covers nothing, so that it errs on the side of failing: before a generic parameter (whose bounds and default the compiler does not cover, nor its name for the lints of its later passes), a field of a struct expression or pattern (which look alike, and on a pattern's field the compiler applies the attribute to almost nothing), a parameter of a function pointer type, or the first statement of a block whose `{` the model cannot tell from that of a struct expression, such as after `if x`; inside a macro's input, because the macro decides where the attribute and the tokens after it go; and anywhere else that the model does not recognise.
//! - An attribute on a `macro_rules!` definition or on a macro call covers nothing: the compiler does not apply it where the macro expands (on a macro call in item position it reports the attribute as unused).
//! - An outer attribute on `mod name;` covers the declaration, and with it the module's file and the files of its submodules: a position in a module's file is covered if every declaration that leads to the file from the unit's crate root is.
//! - An inner attribute covers the rest of the `{…}` block or module it stands in, or, at the top of a file, the whole file, and through the declarations the module's submodules (in a crate's root file, the whole crate). Inside a macro's input it covers nothing.
//! - Code that a `macro_rules!` macro writes gets its lint levels where the macro is called, not where it is defined. A position that a macro wrote is therefore covered by an attribute inside the same transcriber (the code that a rule of the macro writes), or else where the macro was called, and an attribute outside the macro's definition never covers its code. Inside a transcriber, a region also ends at a metavariable (`$name`) that could expand to something that ends the item, which is any but `$crate` and those bound as `ident`, `lifetime`, `literal`, `vis`, `ty`, `path`, `expr`, `pat` or `meta`.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::sources::{self, Kind, Token};
use crate::verify::CANARY_SWITCH;

/// The values of the approved conditions in one unit that the compiler compiled, and the lint canary's switch.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Config {
    /// Whether the unit is compiled as tests (`cfg(test)`).
    pub test: bool,
    /// Whether debug assertions are on (`cfg(debug_assertions)`).
    pub debug_assertions: bool,
    /// Whether the unit is compiled for WebAssembly (`cfg(target_arch = "wasm32")`).
    pub wasm32: bool,
    /// The lint canary's case (`cfg(bayan_lint_canary = "<case>")`), which only the guardrails step sets, for the canary's own crate.
    pub canary_case: Option<String>,
}

/// Where the compiler reports a token: a file, the position of the token's first byte in it, and the expansion of the macro that wrote the token, if a macro did.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Place {
    /// The file, as the file system names it.
    pub file: PathBuf,
    /// The position of the token's first byte in the file.
    pub byte: usize,
    /// The expansion of the macro that wrote the token, if a macro wrote it.
    pub expansion: Option<Box<Expansion>>,
}

/// One macro expansion.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Expansion {
    /// Where the macro is defined, if the compiler says: the file and a position in the definition's first line.
    pub definition: Option<(PathBuf, usize)>,
    /// Where the macro was called.
    pub call_site: Place,
}

/// How many declarations and macro calls deep the model follows a position; a real workspace stays far below.
const MAX_DEPTH: usize = 64;

/// The fragments of a macro's metavariable that cannot expand to anything that ends an item early: single tokens and the syntax nodes that the compiler keeps whole. A `block`, `item` or `stmt` can end an item, and a `tt` or a repetition can be anything.
const WHOLE_FRAGMENTS: [&str; 11] = [
    "ident",
    "lifetime",
    "literal",
    "vis",
    "ty",
    "path",
    "expr",
    "expr_2021",
    "pat",
    "pat_param",
    "meta",
];

/// The built-in attributes that the compiler checks as part of the item they stand on, under its lint levels, such as `#[unsafe(no_mangle)]`, which `unsafe_code` reports. A derive or an attribute macro is not among them: the code it writes is a separate item, to which the attributes of this one do not apply.
const BUILT_IN_ATTRIBUTES: [&str; 17] = [
    "cfg",
    "cold",
    "deprecated",
    "doc",
    "expect",
    "export_name",
    "ignore",
    "inline",
    "link_section",
    "must_use",
    "no_mangle",
    "non_exhaustive",
    "repr",
    "should_panic",
    "test",
    "track_caller",
    "unsafe",
];

/// The keywords that may stand before a `!` that is not a macro call, as in `if !(…)`.
const NOT_MACROS: [&str; 8] = [
    "if", "while", "match", "return", "in", "break", "yield", "else",
];

/// Rust's keywords, strict, reserved and weak.
const KEYWORDS: [&str; 56] = [
    "abstract",
    "as",
    "async",
    "await",
    "become",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "macro",
    "macro_rules",
    "match",
    "mod",
    "move",
    "mut",
    "override",
    "priv",
    "pub",
    "raw",
    "ref",
    "return",
    "safe",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "union",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
];

/// The keywords that may stand in the header of a definition between its keyword and its parameters or body: in generic parameters, bounds and a `where` clause.
const HEADER_KEYWORDS: [&str; 10] = [
    "as", "const", "crate", "dyn", "for", "mut", "self", "Self", "super", "where",
];

/// The keywords that may stand in the expression after `match`.
const SCRUTINEE_KEYWORDS: [&str; 11] = [
    "as", "await", "crate", "dyn", "false", "mut", "self", "Self", "super", "true", "unsafe",
];

/// The keywords after which a `{` opens a block.
const BLOCK_KEYWORDS: [&str; 7] = ["async", "const", "else", "loop", "move", "try", "unsafe"];

/// The coverage model of the workspace's files.
#[derive(Debug)]
pub struct Model {
    /// The files that the source check read, as the file system names them; the model reads no other file.
    sources: BTreeSet<PathBuf>,
    /// The model of each file read so far.
    files: BTreeMap<PathBuf, File>,
    /// For each crate root met so far, the declarations that lead to each of its module files: the declaring file and the position of the declaration's `mod` token.
    trees: BTreeMap<PathBuf, BTreeMap<PathBuf, Vec<(PathBuf, usize)>>>,
}

impl Model {
    /// A model of the files `sources`, as the file system names them.
    pub fn new(sources: BTreeSet<PathBuf>) -> Self {
        Self {
            sources,
            files: BTreeMap::new(),
            trees: BTreeMap::new(),
        }
    }

    /// Whether the source check read `file`, as the file system names it.
    pub fn reads(&self, file: &Path) -> bool {
        self.sources.contains(file)
    }

    /// The line, counting from 1, of the byte at `byte` in `file`, which the source check read.
    pub fn line_of(&mut self, file: &Path, byte: usize) -> Result<usize, String> {
        let model = self.file(file)?;
        Ok(model.line_starts.partition_point(|start| *start <= byte))
    }

    /// Whether an exception written out in full for `lint` covers the code at `place`, in a unit whose crate root is `root` and whose configuration is `config`.
    pub fn covers(
        &mut self,
        lint: &str,
        place: &Place,
        root: &Path,
        config: &Config,
    ) -> Result<bool, String> {
        self.covers_at(lint, place, root, config, 0)
    }

    fn covers_at(
        &mut self,
        lint: &str,
        place: &Place,
        root: &Path,
        config: &Config,
        depth: usize,
    ) -> Result<bool, String> {
        if depth > MAX_DEPTH {
            return Err(format!(
                "{}: the declarations and macro calls that lead to it are nested more than {MAX_DEPTH} deep",
                place.file.display()
            ));
        }
        if !self.sources.contains(&place.file) {
            return Ok(false);
        }
        let file = self.file(&place.file)?;
        if let Some(expansion) = &place.expansion {
            // Written by a macro: covered by an exception next to it in the macro's own code, or where the macro was called.
            let own = expansion
                .definition
                .as_ref()
                .filter(|(definition, _)| *definition == place.file)
                .and_then(|(_, at)| file.transcriber_of(*at, place.byte));
            if own.is_some_and(|transcriber| {
                file.covered(Some(transcriber), lint, place.byte, config)
            }) {
                return Ok(true);
            }
            let call_site = expansion.call_site.clone();
            return self.covers_at(lint, &call_site, root, config, depth + 1);
        }
        let transcriber = file.innermost_transcriber(place.byte);
        if file.covered(transcriber, lint, place.byte, config) {
            return Ok(true);
        }
        if transcriber.is_some() {
            return Ok(false);
        }
        // The module that the file holds is covered where it is declared, by every declaration that leads to it from the crate root.
        let declarations = self.declarations(root, &place.file)?;
        if declarations.is_empty() {
            return Ok(false);
        }
        for (declaring, byte) in declarations {
            let declaration = Place {
                file: declaring,
                byte,
                expansion: None,
            };
            if !self.covers_at(lint, &declaration, root, config, depth + 1)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The model of `file`, read on first use.
    fn file(&mut self, file: &Path) -> Result<&File, String> {
        if !self.files.contains_key(file) {
            let text = std::fs::read_to_string(file)
                .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
            let model =
                File::read(&text).map_err(|problem| format!("{}: {problem}", file.display()))?;
            self.files.insert(file.to_path_buf(), model);
        }
        self.files
            .get(file)
            .ok_or_else(|| format!("{} was not read", file.display()))
    }

    /// The declarations that lead to the module file `file` from the crate root `root`: the declaring files and the positions of their `mod` tokens. Empty for the root itself, and for a file that no declaration that the model can follow leads to.
    fn declarations(&mut self, root: &Path, file: &Path) -> Result<Vec<(PathBuf, usize)>, String> {
        if !self.trees.contains_key(root) {
            let tree = self.tree(root)?;
            self.trees.insert(root.to_path_buf(), tree);
        }
        Ok(self
            .trees
            .get(root)
            .and_then(|tree| tree.get(file))
            .cloned()
            .unwrap_or_default())
    }

    /// Follows the declarations `mod name;` from the crate root `root` to every module file, as the compiler finds them: `name.rs` or `name/mod.rs` in the folder of a crate root or a `mod.rs` file, or in the folder named after any other module file, below the folders of the inline modules around the declaration. The source check rejects `#[path]`, so these are the only places.
    fn tree(&mut self, root: &Path) -> Result<BTreeMap<PathBuf, Vec<(PathBuf, usize)>>, String> {
        let mut tree: BTreeMap<PathBuf, Vec<(PathBuf, usize)>> = BTreeMap::new();
        let mut queue = VecDeque::from([root.to_path_buf()]);
        let mut seen = BTreeSet::from([root.to_path_buf()]);
        while let Some(file) = queue.pop_front() {
            if !self.sources.contains(&file) {
                continue;
            }
            let declarations = self.file(&file)?.declarations.clone();
            let folder = file.parent().unwrap_or_else(|| Path::new(""));
            let base = if file == root || file.file_name() == Some(OsStr::new("mod.rs")) {
                folder.to_path_buf()
            } else {
                folder.join(file.file_stem().unwrap_or_default())
            };
            for declaration in declarations {
                let mut folder = base.clone();
                folder.extend(&declaration.path);
                for candidate in [
                    folder.join(format!("{}.rs", declaration.name)),
                    folder.join(&declaration.name).join("mod.rs"),
                ] {
                    let Ok(module) = std::fs::canonicalize(&candidate) else {
                        continue;
                    };
                    if !self.sources.contains(&module) {
                        continue;
                    }
                    tree.entry(module.clone())
                        .or_default()
                        .push((file.clone(), declaration.byte));
                    if seen.insert(module.clone()) {
                        queue.push_back(module);
                    }
                }
            }
        }
        Ok(tree)
    }
}

/// The model of one file.
#[derive(Debug)]
struct File {
    /// The exceptions written out in full.
    expects: Vec<Expect>,
    /// The transcribers of its `macro_rules!` definitions.
    transcribers: Vec<Transcriber>,
    /// Its declarations `mod name;` that the model can follow.
    declarations: Vec<Declaration>,
    /// The position of the first byte of each line.
    line_starts: Vec<usize>,
}

/// An exception written out in full: `expect(<lints>, reason = "…")`, alone or inside `cfg_attr`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Expect {
    /// The lints it names.
    lints: Vec<String>,
    /// When it applies.
    condition: Condition,
    /// The bytes it covers.
    region: Vec<Range<usize>>,
    /// The innermost transcriber that holds it, as a position in [`File::transcribers`].
    transcriber: Option<usize>,
}

/// The code that one rule of a `macro_rules!` definition writes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Transcriber {
    /// The bytes from the definition's first outer attribute, or its `macro_rules` token, through that token: where the compiler may say that the macro is defined.
    definition: Range<usize>,
    /// The position of its opening bracket among the tokens.
    open: usize,
    /// The positions of its tokens, brackets excluded.
    tokens: Range<usize>,
    /// Its bytes, brackets excluded.
    bytes: Range<usize>,
    /// The fragment that the rule's matcher binds each metavariable to, such as `ident` for `$name:ident`.
    bindings: BTreeMap<String, String>,
}

/// A declaration `mod name;`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Declaration {
    /// The module's name.
    name: String,
    /// The position of its `mod` token.
    byte: usize,
    /// The inline modules around it, outermost first.
    path: Vec<String>,
}

/// The condition of a `cfg_attr`, as far as the model can evaluate it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Condition {
    /// `true` or `false`.
    Bool(bool),
    /// `test`.
    Test,
    /// `debug_assertions`.
    DebugAssertions,
    /// `target_arch = "wasm32"`.
    Wasm32,
    /// The lint canary's switch, with its value.
    Canary(String),
    /// `all(…)`.
    All(Vec<Condition>),
    /// `any(…)`.
    Any(Vec<Condition>),
    /// `not(…)`.
    Not(Box<Condition>),
    /// Anything else, which the preflight rejects.
    Unknown,
}

impl Condition {
    /// Whether the condition holds in `config`, or `None` if the model cannot tell.
    fn holds(&self, config: &Config) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            Self::Test => Some(config.test),
            Self::DebugAssertions => Some(config.debug_assertions),
            Self::Wasm32 => Some(config.wasm32),
            Self::Canary(case) => Some(config.canary_case.as_deref() == Some(case.as_str())),
            Self::All(conditions) => {
                let mut all = Some(true);
                for condition in conditions {
                    match condition.holds(config) {
                        Some(false) => return Some(false),
                        None => all = None,
                        Some(true) => {}
                    }
                }
                all
            }
            Self::Any(conditions) => {
                let mut any = Some(false);
                for condition in conditions {
                    match condition.holds(config) {
                        Some(true) => return Some(true),
                        None => any = None,
                        Some(false) => {}
                    }
                }
                any
            }
            Self::Not(condition) => condition.holds(config).map(|value| !value),
            Self::Unknown => None,
        }
    }
}

/// Where an attribute stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// In the code itself.
    Code,
    /// In the input of a macro call, which the macro may rearrange.
    MacroInput,
    /// In the pattern that a macro's input must match, which is never code.
    Matcher,
    /// In a transcriber, as a position in [`File::transcribers`].
    Transcriber(usize),
}

/// What an outer attribute stands before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// An item or a `let` statement.
    Item,
    /// A `macro_rules!` definition.
    MacroRules,
    /// A macro call.
    MacroCall,
    /// Anything else: a field, a match arm, a parameter, an expression.
    Other,
}

impl File {
    /// Reads the model of a file whose text is `text`.
    fn read(text: &str) -> Result<Self, String> {
        let tokens = sources::tokenize(text)?;
        let closing = sources::closing_brackets(&tokens)?;
        let rules = sources::macro_rules(&tokens, &closing)?;
        let mut reader = Reader {
            text,
            tokens: &tokens,
            closing: &closing,
            opening: vec![None; tokens.len()],
            parent: vec![None; tokens.len()],
            matchers: rules.iter().map(|rule| rule.matcher.clone()).collect(),
            transcribers: Vec::new(),
        };
        let mut stack = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            reader.parent[index] = stack.last().copied();
            match token.kind {
                Kind::Punct(b'(' | b'[' | b'{') => stack.push(index),
                Kind::Punct(b')' | b']' | b'}') => {
                    if let Some(open) = stack.pop() {
                        reader.opening[index] = Some(open);
                        reader.parent[index] = stack.last().copied();
                    }
                }
                _ => {}
            }
        }
        for rule in &rules {
            let open = rule.transcriber.start - 1;
            let close = rule.transcriber.end;
            let start = reader.first_attribute(rule.definition);
            reader.transcribers.push(Transcriber {
                definition: tokens[start].start..tokens[rule.definition].end,
                open,
                tokens: rule.transcriber.clone(),
                bytes: tokens[open].end..tokens[close].start,
                bindings: reader.bindings(&rule.matcher),
            });
        }
        let mut expects = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            if token.kind != Kind::Punct(b'#') {
                continue;
            }
            let inner = reader.kind(index + 1) == Some(Kind::Punct(b'!'));
            let bracket = if inner { index + 2 } else { index + 1 };
            if reader.kind(bracket) != Some(Kind::Punct(b'[')) {
                continue;
            }
            let Some(close) = closing[bracket] else {
                continue;
            };
            let mut found = Vec::new();
            reader.exceptions(bracket + 1..close, &Condition::Bool(true), &mut found);
            if found.is_empty() {
                continue;
            }
            let context = reader.context(index);
            let region = if inner {
                reader
                    .inner_region(index, close, context)
                    .map(|region| vec![region])
            } else {
                reader.outer_region(index, close, context)
            };
            let Some(region) = region else {
                continue;
            };
            let transcriber = reader.transcriber_around(index);
            for (lints, condition) in found {
                expects.push(Expect {
                    lints,
                    condition,
                    region: region.clone(),
                    transcriber,
                });
            }
        }
        let declarations = reader.declarations();
        let line_starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(at, _)| at + 1))
            .collect();
        Ok(Self {
            expects,
            transcribers: reader.transcribers,
            declarations,
            line_starts,
        })
    }

    /// The innermost transcriber whose code holds the byte at `byte`, as a position in [`Self::transcribers`].
    fn innermost_transcriber(&self, byte: usize) -> Option<usize> {
        self.transcribers
            .iter()
            .enumerate()
            .filter(|(_, transcriber)| transcriber.bytes.contains(&byte))
            .min_by_key(|(_, transcriber)| transcriber.bytes.len())
            .map(|(index, _)| index)
    }

    /// The innermost transcriber that holds the byte at `byte`, if it belongs to the `macro_rules!` definition whose first line holds the byte at `definition`.
    fn transcriber_of(&self, definition: usize, byte: usize) -> Option<usize> {
        self.innermost_transcriber(byte)
            .filter(|at| self.transcribers[*at].definition.contains(&definition))
    }

    /// Whether an exception for `lint` that `transcriber` holds (or that no transcriber holds, for `None`) and that applies in `config` covers the byte at `byte`.
    fn covered(
        &self,
        transcriber: Option<usize>,
        lint: &str,
        byte: usize,
        config: &Config,
    ) -> bool {
        self.expects.iter().any(|expect| {
            expect.transcriber == transcriber
                && expect.region.iter().any(|part| part.contains(&byte))
                && expect.lints.iter().any(|named| named == lint)
                && expect.condition.holds(config) == Some(true)
        })
    }
}

/// Which region of the code an outer attribute's region extends over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Extent {
    /// An item or a `let` statement in item or statement position: up to its first `;`, the end of its first `{…}` group or the end of the group around it.
    Item,
    /// Another statement, a field or a variant in the definition of a type, or a match arm: also no further than its first `,` or a `>` that is not part of `->` or `=>`.
    Node,
    /// A parameter of a function or a closure: its pattern, which also ends before the `:` of its type and at a `|` or `=`.
    Pattern,
}

/// What a bracket group holds, as far as the model can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    /// The parameters of a function: `fn name<…>(…)`.
    Parameters,
    /// The fields of a struct, union or variant in its definition.
    Fields,
    /// The variants of an enum.
    Variants,
    /// The arms of a `match`.
    Arms,
    /// A block, which holds statements.
    Block,
    /// Anything else, such as the fields of a struct expression or pattern, which look alike, the parameters of a function pointer type or the arguments of a call.
    Other,
}

/// What opens the list that an element after a comma belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opener {
    /// The `|` of a closure's parameters.
    Pipe,
    /// The `<` of generic parameters.
    Angle,
    /// The bracket group around the element, which opens at this position.
    Group(usize),
}

/// Reads the model of one file from its tokens.
struct Reader<'a> {
    /// The file's text.
    text: &'a str,
    /// Its tokens.
    tokens: &'a [Token<'a>],
    /// For each opening bracket, the position of the bracket that closes it.
    closing: &'a [Option<usize>],
    /// For each closing bracket, the position of the bracket it closes.
    opening: Vec<Option<usize>>,
    /// For each token, the position of the opening bracket of the innermost group around it.
    parent: Vec<Option<usize>>,
    /// The positions of the tokens of every matcher.
    matchers: Vec<Range<usize>>,
    /// The transcribers.
    transcribers: Vec<Transcriber>,
}

impl Reader<'_> {
    fn kind(&self, at: usize) -> Option<Kind<'_>> {
        self.tokens.get(at).map(|token| token.kind)
    }

    /// The identifier at `at`, unless it is written as a raw identifier (`r#name`), which is never a keyword.
    fn word(&self, at: usize) -> Option<&str> {
        match self.tokens.get(at)?.kind {
            Kind::Ident(word) if !self.text[self.tokens[at].start..].starts_with("r#") => {
                Some(word)
            }
            _ => None,
        }
    }

    /// The fragment that each metavariable of the matcher at `matcher` is bound to.
    fn bindings(&self, matcher: &Range<usize>) -> BTreeMap<String, String> {
        let mut bindings = BTreeMap::new();
        for at in matcher.clone() {
            if let (
                Some(Kind::Punct(b'$')),
                Some(Kind::Ident(name)),
                Some(Kind::Punct(b':')),
                Some(Kind::Ident(fragment)),
            ) = (
                self.kind(at),
                self.kind(at + 1),
                self.kind(at + 2),
                self.kind(at + 3),
            ) {
                bindings.insert(name.to_owned(), fragment.to_owned());
            }
        }
        bindings
    }

    /// The position of the `#` of the first outer attribute in the run of outer attributes that ends with the one at `at` (or with the token at `at` itself, such as a `macro_rules` token).
    fn first_attribute(&self, at: usize) -> usize {
        let mut first = at;
        while let Some(before) = first.checked_sub(1) {
            match (self.kind(before), self.opening[before]) {
                (Some(Kind::Punct(b']')), Some(open))
                    if open >= 1 && self.kind(open - 1) == Some(Kind::Punct(b'#')) =>
                {
                    first = open - 1;
                }
                _ => break,
            }
        }
        first
    }

    /// The innermost transcriber that holds the token at `at`.
    fn transcriber_around(&self, at: usize) -> Option<usize> {
        self.transcribers
            .iter()
            .enumerate()
            .filter(|(_, transcriber)| transcriber.tokens.contains(&at))
            .min_by_key(|(_, transcriber)| transcriber.tokens.len())
            .map(|(index, _)| index)
    }

    /// Where the token at `at` stands: in a matcher, in a macro call's input inside the innermost transcriber around it (or in the code, if none), or else in that transcriber or the code.
    fn context(&self, at: usize) -> Context {
        if self.matchers.iter().any(|matcher| matcher.contains(&at)) {
            return Context::Matcher;
        }
        let transcriber = self.transcriber_around(at);
        let boundary = transcriber.map(|index| self.transcribers[index].open);
        let mut inner = at;
        while let Some(group) = self.parent[inner] {
            if Some(group) == boundary {
                break;
            }
            if self.is_macro_input(group) {
                return Context::MacroInput;
            }
            inner = group;
        }
        transcriber.map_or(Context::Code, Context::Transcriber)
    }

    /// Whether the group that opens at `open` is the input of a macro call, `name!(…)`.
    fn is_macro_input(&self, open: usize) -> bool {
        open >= 2
            && self.kind(open - 1) == Some(Kind::Punct(b'!'))
            && matches!(self.kind(open - 2), Some(Kind::Ident(name)) if !NOT_MACROS.contains(&name))
    }

    /// The lint exceptions in the attribute content at `range`, each with the lints it names and the condition under which it applies, given that the attribute applies under `condition`.
    fn exceptions(
        &self,
        range: Range<usize>,
        condition: &Condition,
        found: &mut Vec<(Vec<String>, Condition)>,
    ) {
        if range.len() < 3 || self.kind(range.start + 1) != Some(Kind::Punct(b'(')) {
            return;
        }
        if self.closing[range.start + 1] != Some(range.end - 1) {
            return;
        }
        let arguments = range.start + 2..range.end - 1;
        match self.word(range.start) {
            Some("expect") => {
                if let Some(lints) = self.expect_arguments(arguments) {
                    found.push((lints, condition.clone()));
                }
            }
            Some("cfg_attr") => {
                let parts = self.split(arguments);
                if let Some((predicate, attributes)) = parts.split_first() {
                    let both =
                        Condition::All(vec![condition.clone(), self.condition(predicate.clone())]);
                    for attribute in attributes {
                        self.exceptions(attribute.clone(), &both, found);
                    }
                }
            }
            _ => {}
        }
    }

    /// The lints that the arguments of an `expect(…)` at `range` name, if they are written out in full: lint names, then `reason = "…"`.
    fn expect_arguments(&self, range: Range<usize>) -> Option<Vec<String>> {
        let parts = self.split(range);
        let (reason, lints) = parts.split_last()?;
        match self.tokens[reason.clone()]
            .iter()
            .map(|token| token.kind)
            .collect::<Vec<_>>()[..]
        {
            [Kind::Ident("reason"), Kind::Punct(b'='), Kind::Str(text)]
                if text.starts_with('"') || text.starts_with("r\"") || text.starts_with("r#") => {}
            _ => return None,
        }
        let mut names = Vec::new();
        for lint in lints {
            match self.tokens[lint.clone()]
                .iter()
                .map(|token| token.kind)
                .collect::<Vec<_>>()[..]
            {
                [Kind::Ident(name)] => names.push(name.to_owned()),
                [Kind::Ident(tool), Kind::PathSeparator, Kind::Ident(name)] => {
                    names.push(format!("{tool}::{name}"));
                }
                _ => return None,
            }
        }
        (!names.is_empty()).then_some(names)
    }

    /// The condition at `range`.
    fn condition(&self, range: Range<usize>) -> Condition {
        let kinds: Vec<Kind<'_>> = self.tokens[range.clone()]
            .iter()
            .map(|token| token.kind)
            .collect();
        match kinds[..] {
            [Kind::Ident("test")] => Condition::Test,
            [Kind::Ident("debug_assertions")] => Condition::DebugAssertions,
            [Kind::Ident("true")] => Condition::Bool(true),
            [Kind::Ident("false")] => Condition::Bool(false),
            [
                Kind::Ident("target_arch"),
                Kind::Punct(b'='),
                Kind::Str("\"wasm32\""),
            ] => Condition::Wasm32,
            [Kind::Ident(switch), Kind::Punct(b'='), Kind::Str(value)]
                if switch == CANARY_SWITCH =>
            {
                value
                    .strip_prefix('"')
                    .and_then(|rest| rest.strip_suffix('"'))
                    .map_or(Condition::Unknown, |case| {
                        Condition::Canary(case.to_owned())
                    })
            }
            [
                Kind::Ident(operator @ ("all" | "any" | "not")),
                Kind::Punct(b'('),
                ..,
            ] if self.closing[range.start + 1] == Some(range.end - 1) => {
                let mut inner: Vec<Condition> = self
                    .split(range.start + 2..range.end - 1)
                    .into_iter()
                    .map(|part| self.condition(part))
                    .collect();
                match operator {
                    "all" => Condition::All(inner),
                    "any" => Condition::Any(inner),
                    _ if inner.len() == 1 => {
                        Condition::Not(Box::new(inner.pop().unwrap_or(Condition::Unknown)))
                    }
                    _ => Condition::Unknown,
                }
            }
            _ => Condition::Unknown,
        }
    }

    /// The parts of `range` between its commas outside brackets; a comma at the end ends the last part.
    fn split(&self, range: Range<usize>) -> Vec<Range<usize>> {
        let mut parts = Vec::new();
        let mut start = range.start;
        let mut at = range.start;
        while at < range.end {
            match self.kind(at) {
                Some(Kind::Punct(b'(' | b'[' | b'{')) => {
                    at = self.closing[at].map_or(range.end, |close| close + 1);
                    continue;
                }
                Some(Kind::Punct(b',')) => {
                    parts.push(start..at);
                    start = at + 1;
                }
                _ => {}
            }
            at += 1;
        }
        if start < range.end {
            parts.push(start..range.end);
        }
        parts
    }

    /// The bytes that the outer attribute whose `#` is at `hash` and whose `]` is at `close` covers: the item, statement or other code it stands before, and, unless that is a parameter, the built-in attributes of its run, to which the compiler applies the same lint levels.
    fn outer_region(
        &self,
        hash: usize,
        close: usize,
        context: Context,
    ) -> Option<Vec<Range<usize>>> {
        // What it stands before: the first token after it and the outer attributes that follow it.
        let mut start = close + 1;
        while self.kind(start) == Some(Kind::Punct(b'#'))
            && self.kind(start + 1) == Some(Kind::Punct(b'['))
        {
            start = self.closing[start + 1]? + 1;
        }
        self.tokens.get(start)?;
        let extent = self.extent(hash, start, context)?;
        let last = self.scan(start, extent, context)?;
        let item = self.tokens[start].start..self.tokens[last].end;
        let mut region = vec![item];
        if extent == Extent::Pattern {
            return Some(region);
        }
        let mut at = self.first_attribute(hash);
        while at < start {
            let close = self.closing[at + 1]?;
            if at != hash
                && self
                    .word(at + 2)
                    .is_some_and(|name| BUILT_IN_ATTRIBUTES.contains(&name))
            {
                region.push(self.tokens[at].start..self.tokens[close].end);
            }
            at = close + 1;
        }
        Some(region)
    }

    /// Whether the run of outer attributes that holds the one at `hash` stands where an item or a statement may: at the start of the file or of a transcriber, or after `;`, `{`, `}` or an inner attribute.
    fn in_item_position(&self, hash: usize, context: Context) -> bool {
        let Some(before) = self.first_attribute(hash).checked_sub(1) else {
            return true;
        };
        match self.kind(before) {
            Some(Kind::Punct(b';' | b'{' | b'}')) => true,
            Some(Kind::Punct(b']')) => self.ends_inner_attribute(before),
            _ => {
                matches!(context, Context::Transcriber(index) if self.transcribers[index].open == before)
            }
        }
    }

    /// Whether the `]` at `at` ends an inner attribute, `#![…]`.
    fn ends_inner_attribute(&self, at: usize) -> bool {
        self.opening[at].is_some_and(|open| {
            open >= 2
                && self.kind(open - 1) == Some(Kind::Punct(b'!'))
                && self.kind(open - 2) == Some(Kind::Punct(b'#'))
        })
    }

    /// The punctuation before the `>` at `at`, if it makes it part of `->` or `=>`.
    fn arrow(&self, at: usize) -> Option<u8> {
        match at.checked_sub(1).and_then(|before| self.kind(before)) {
            Some(Kind::Punct(byte @ (b'-' | b'='))) => Some(byte),
            _ => None,
        }
    }

    /// How far the outer attribute whose `#` is at `hash` extends over the code from the token at `start` on, which it stands before in `context`, or `None` where the model cannot tell how far the compiler applies it, so that it covers nothing.
    fn extent(&self, hash: usize, start: usize, context: Context) -> Option<Extent> {
        if matches!(context, Context::Matcher | Context::MacroInput) {
            return None;
        }
        let target = self.target(start);
        if matches!(target, Target::MacroRules | Target::MacroCall) {
            return None;
        }
        if target == Target::Item && self.in_item_position(hash, context) {
            return Some(Extent::Item);
        }
        let run = self.first_attribute(hash);
        let before = run.checked_sub(1)?;
        if matches!(context, Context::Transcriber(index) if self.transcribers[index].open == before)
        {
            // What a macro writes first is a statement there, or the compiler rejects the attribute.
            return Some(Extent::Node);
        }
        match self.kind(before)? {
            Kind::Punct(b';' | b'}') => Some(Extent::Node),
            Kind::Punct(b']') if self.ends_inner_attribute(before) => Some(Extent::Node),
            Kind::Punct(b'|') => Some(Extent::Pattern),
            Kind::Punct(b'{' | b'(') => match self.group(before) {
                Group::Parameters => Some(Extent::Pattern),
                Group::Fields | Group::Variants | Group::Arms | Group::Block => Some(Extent::Node),
                Group::Other => None,
            },
            Kind::Punct(b',') => match self.list_opener(run)? {
                Opener::Pipe => Some(Extent::Pattern),
                Opener::Angle => None,
                Opener::Group(group) => match self.group(group) {
                    Group::Parameters => Some(Extent::Pattern),
                    Group::Fields | Group::Variants | Group::Arms => Some(Extent::Node),
                    Group::Block | Group::Other => None,
                },
            },
            _ => None,
        }
    }

    /// What opens the list whose element starts at `run`, after a comma: the `|` of a closure's parameters, the `<` of generic parameters, or else the bracket group around it; `None` at the top level of a file.
    fn list_opener(&self, run: usize) -> Option<Opener> {
        let mut at = run;
        let mut angles = 0usize;
        while let Some(before) = at.checked_sub(1) {
            match self.kind(before)? {
                Kind::Punct(b')' | b']' | b'}') => {
                    at = self.opening[before]?;
                    continue;
                }
                Kind::Punct(b'(' | b'[' | b'{') => return Some(Opener::Group(before)),
                Kind::Punct(b'|') => return Some(Opener::Pipe),
                // A statement or a match arm ends here, so the list is the group's own.
                Kind::Punct(b';') => return self.parent[run].map(Opener::Group),
                Kind::Punct(b'>') => match self.arrow(before) {
                    Some(b'=') => return self.parent[run].map(Opener::Group),
                    Some(_) => {}
                    None => angles += 1,
                },
                Kind::Punct(b'<') => match angles.checked_sub(1) {
                    Some(fewer) => angles = fewer,
                    None => return Some(Opener::Angle),
                },
                _ => {}
            }
            at = before;
        }
        None
    }

    /// What the bracket group that opens at `open` holds, as far as the model can tell.
    fn group(&self, open: usize) -> Group {
        match self.kind(open) {
            Some(Kind::Punct(b'(')) => match self.definition(open) {
                Some("fn") => Group::Parameters,
                Some("struct") => Group::Fields,
                _ if self.is_variant(open) => Group::Fields,
                _ => Group::Other,
            },
            Some(Kind::Punct(b'{')) => {
                if self.is_match_body(open) {
                    return Group::Arms;
                }
                match self.definition(open) {
                    Some("struct" | "union") => Group::Fields,
                    Some("enum") => Group::Variants,
                    _ if self.is_variant(open) => Group::Fields,
                    _ if self.is_block(open) => Group::Block,
                    _ => Group::Other,
                }
            }
            _ => Group::Other,
        }
    }

    /// The keyword of the definition whose parameters or body the group at `open` holds, if the tokens before it are that definition's header: `fn`, `struct`, `union` or `enum`, a name, generic parameters and, before a body, a `where` clause.
    fn definition(&self, open: usize) -> Option<&str> {
        let mut at = open;
        let mut angles = 0usize;
        loop {
            let before = at.checked_sub(1)?;
            match self.kind(before)? {
                Kind::Punct(b')' | b']') => {
                    at = self.opening[before]?;
                    continue;
                }
                Kind::Punct(b'>') => {
                    if self.arrow(before).is_none() {
                        angles += 1;
                    }
                }
                Kind::Punct(b'<') => angles = angles.checked_sub(1)?,
                Kind::Punct(b'=') if angles > 0 => {}
                Kind::Punct(b':' | b',' | b'+' | b'?' | b'&' | b'*' | b'-' | b'!')
                | Kind::PathSeparator
                | Kind::Other => {}
                Kind::Ident(_) => match self.word(before) {
                    Some(keyword @ ("fn" | "struct" | "union" | "enum")) => {
                        let after = before + 2;
                        let shaped = matches!(self.kind(before + 1), Some(Kind::Ident(_)))
                            && (after == open
                                || self.kind(after) == Some(Kind::Punct(b'<'))
                                || self.word(after) == Some("where"));
                        return (angles == 0 && shaped).then_some(keyword);
                    }
                    Some(word) if KEYWORDS.contains(&word) && !HEADER_KEYWORDS.contains(&word) => {
                        return None;
                    }
                    _ => {}
                },
                _ => return None,
            }
            at = before;
        }
    }

    /// Whether the group at `open` holds the fields of a variant in the definition of an enum: whether it follows the variant's name, which stands first in the enum's body or after a comma, after the variant's outer attributes.
    fn is_variant(&self, open: usize) -> bool {
        let (Some(name), Some(body)) = (open.checked_sub(1), self.parent[open]) else {
            return false;
        };
        matches!(self.kind(name), Some(Kind::Ident(_)))
            && self
                .first_attribute(name)
                .checked_sub(1)
                .is_some_and(|before| {
                    before == body || self.kind(before) == Some(Kind::Punct(b','))
                })
            && self.kind(body) == Some(Kind::Punct(b'{'))
            && self.definition(body) == Some("enum")
    }

    /// Whether the `{` at `open` holds the arms of a `match`: whether the tokens before it are an expression after the keyword `match`.
    fn is_match_body(&self, open: usize) -> bool {
        let mut at = open;
        while let Some(before) = at.checked_sub(1) {
            match self.kind(before) {
                Some(Kind::Punct(b')' | b']' | b'}')) => {
                    let Some(group) = self.opening[before] else {
                        return false;
                    };
                    at = group;
                    continue;
                }
                Some(
                    Kind::Punct(
                        b'.' | b'&' | b'*' | b'-' | b'!' | b'+' | b'/' | b'%' | b'^' | b'|' | b'<'
                        | b'>' | b'?',
                    )
                    | Kind::PathSeparator
                    | Kind::Str(_)
                    | Kind::Other,
                ) => {}
                Some(Kind::Ident(_)) => match self.word(before) {
                    Some("match") => return true,
                    Some(word)
                        if KEYWORDS.contains(&word) && !SCRUTINEE_KEYWORDS.contains(&word) =>
                    {
                        return false;
                    }
                    _ => {}
                },
                _ => return false,
            }
            at = before;
        }
        false
    }

    /// Whether the `{` at `open` opens a block, which holds statements: whether the token before it is one after which a block can start but the fields of a struct expression or pattern cannot, because they follow a path.
    fn is_block(&self, open: usize) -> bool {
        let Some(before) = open.checked_sub(1) else {
            return false;
        };
        match self.kind(before) {
            Some(
                Kind::Punct(
                    b')' | b']' | b';' | b'{' | b'}' | b'=' | b'(' | b'[' | b',' | b'|' | b':'
                    | b'?',
                )
                | Kind::Str(_)
                | Kind::Other,
            ) => true,
            Some(Kind::Punct(b'>')) => self.arrow(before) == Some(b'='),
            Some(Kind::Ident(_)) => self
                .word(before)
                .is_some_and(|word| BLOCK_KEYWORDS.contains(&word)),
            _ => false,
        }
    }

    /// What the tokens from `start` on are.
    fn target(&self, start: usize) -> Target {
        // A macro call: `$crate::`, `::` or neither, a path, `!` and a bracket.
        let mut at = start;
        if self.kind(at) == Some(Kind::Punct(b'$')) {
            at += 1;
        }
        if self.kind(at) == Some(Kind::PathSeparator) {
            at += 1;
        }
        if matches!(self.kind(at), Some(Kind::Ident(_))) {
            at += 1;
            while self.kind(at) == Some(Kind::PathSeparator)
                && matches!(self.kind(at + 1), Some(Kind::Ident(_)))
            {
                at += 2;
            }
            if self.kind(at) == Some(Kind::Punct(b'!'))
                && matches!(self.kind(at + 1), Some(Kind::Punct(b'(' | b'[' | b'{')))
            {
                return if self.word(start) == Some("macro_rules") {
                    Target::MacroRules
                } else {
                    Target::MacroCall
                };
            }
        }
        if self.word(start) == Some("macro_rules")
            && self.kind(start + 1) == Some(Kind::Punct(b'!'))
        {
            return Target::MacroRules;
        }
        // An item: visibility and qualifiers, then its keyword.
        let mut at = start;
        if self.word(at) == Some("pub") {
            at += 1;
            if self.kind(at) == Some(Kind::Punct(b'(')) {
                let Some(close) = self.closing[at] else {
                    return Target::Other;
                };
                at = close + 1;
            }
        }
        let mut extern_abi = false;
        loop {
            match self.word(at) {
                Some("unsafe" | "async" | "safe") => at += 1,
                Some("default")
                    if matches!(
                        self.word(at + 1),
                        Some("fn" | "impl" | "type" | "const" | "unsafe" | "async" | "extern")
                    ) =>
                {
                    at += 1;
                }
                Some("auto") if self.word(at + 1) == Some("trait") => at += 1,
                Some("const")
                    if matches!(
                        self.word(at + 1),
                        Some("fn" | "unsafe" | "async" | "extern")
                    ) =>
                {
                    at += 1;
                }
                Some("extern") if matches!(self.kind(at + 1), Some(Kind::Str(_))) => {
                    at += 2;
                    extern_abi = true;
                }
                _ => break,
            }
        }
        match self.word(at) {
            Some(
                "fn" | "struct" | "enum" | "trait" | "impl" | "type" | "mod" | "use" | "static"
                | "let",
            ) => Target::Item,
            Some("const") if matches!(self.kind(at + 1), Some(Kind::Ident(_))) => Target::Item,
            Some("union") if self.word(at + 1).is_some() => Target::Item,
            Some("extern")
                if self.word(at + 1) == Some("crate")
                    || self.kind(at + 1) == Some(Kind::Punct(b'{')) =>
            {
                Target::Item
            }
            None if extern_abi && self.kind(at) == Some(Kind::Punct(b'{')) => Target::Item,
            _ => Target::Other,
        }
    }

    /// The position of the last token that a region starting at `start` holds.
    fn scan(&self, start: usize, extent: Extent, context: Context) -> Option<usize> {
        let mut last = None;
        let mut at = start;
        while let Some(token) = self.tokens.get(at) {
            match token.kind {
                Kind::Punct(open @ (b'(' | b'[' | b'{')) => {
                    let close = self.closing[at]?;
                    last = Some(close);
                    if open == b'{' {
                        return last;
                    }
                    at = close + 1;
                    continue;
                }
                Kind::Punct(b')' | b']' | b'}') => return last,
                Kind::Punct(b';') => return Some(at),
                Kind::Punct(b',') if extent != Extent::Item => return last,
                Kind::Punct(b'>') if extent != Extent::Item && self.arrow(at).is_none() => {
                    return last;
                }
                Kind::Punct(b':' | b'|' | b'=') if extent == Extent::Pattern => return last,
                Kind::Punct(b'$') => {
                    if !self.whole_substitution(at, context) {
                        return last;
                    }
                    last = Some(at + 1);
                    at += 2;
                    continue;
                }
                _ => {}
            }
            last = Some(at);
            at += 1;
        }
        last
    }

    /// Whether the metavariable at `at` (its `$`) cannot expand to anything that ends an item: `$crate`, or one that the transcriber's matcher binds to a fragment that the compiler keeps whole.
    fn whole_substitution(&self, at: usize, context: Context) -> bool {
        match (self.kind(at + 1), context) {
            (Some(Kind::Ident("crate")), _) => true,
            (Some(Kind::Ident(name)), Context::Transcriber(index)) => self.transcribers[index]
                .bindings
                .get(name)
                .is_some_and(|fragment| WHOLE_FRAGMENTS.contains(&fragment.as_str())),
            _ => false,
        }
    }

    /// The bytes that the inner attribute whose `#` is at `hash` and whose `]` is at `close` covers: the rest of the `{…}` group it stands in, or the whole file at its top.
    fn inner_region(&self, hash: usize, close: usize, context: Context) -> Option<Range<usize>> {
        if matches!(context, Context::Matcher | Context::MacroInput) {
            return None;
        }
        let Some(group) = self.parent[hash] else {
            return (context == Context::Code).then_some(0..self.text.len());
        };
        if self.kind(group) != Some(Kind::Punct(b'{')) {
            return None;
        }
        if let Context::Transcriber(index) = context
            && self.transcribers[index].open == group
        {
            return None;
        }
        Some(self.tokens[close].end..self.tokens[self.closing[group]?].start)
    }

    /// The declarations `mod name;` in the code itself, outside macros, inside nothing but inline modules.
    fn declarations(&self) -> Vec<Declaration> {
        let mut declarations = Vec::new();
        for (index, token) in self.tokens.iter().enumerate() {
            if self.word(index) != Some("mod") {
                continue;
            }
            let (Some(Kind::Ident(name)), Some(Kind::Punct(b';'))) =
                (self.kind(index + 1), self.kind(index + 2))
            else {
                continue;
            };
            if self.context(index) != Context::Code {
                continue;
            }
            let mut path = Vec::new();
            let mut inner = index;
            let mut inline_modules_only = true;
            while let Some(group) = self.parent[inner] {
                let module = group.checked_sub(2).and_then(|at| {
                    match (self.word(at), self.kind(at + 1), self.kind(group)) {
                        (Some("mod"), Some(Kind::Ident(module)), Some(Kind::Punct(b'{'))) => {
                            Some(module)
                        }
                        _ => None,
                    }
                });
                let Some(module) = module else {
                    inline_modules_only = false;
                    break;
                };
                path.push(module.to_owned());
                inner = group;
            }
            if inline_modules_only {
                path.reverse();
                declarations.push(Declaration {
                    name: name.to_owned(),
                    byte: token.start,
                    path,
                });
            }
        }
        declarations
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lint that most tests ask about.
    const MAP: &str = "clippy::disallowed_types";

    /// Source files for one test in a folder of their own, removed again when the test ends, with the model of all of them.
    struct Sources {
        folder: PathBuf,
        model: Model,
    }

    impl Sources {
        fn new(test: &str, files: &[(&str, &str)]) -> Self {
            let folder = std::env::temp_dir().join(format!(
                "bayan-xtask-coverage-{}-{test}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&folder);
            let mut sources = BTreeSet::new();
            for (name, text) in files {
                let path = under(&folder, name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, text).unwrap();
                sources.insert(std::fs::canonicalize(&path).unwrap());
            }
            let folder = std::fs::canonicalize(&folder).unwrap();
            Self {
                folder,
                model: Model::new(sources),
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            under(&self.folder, name)
        }

        /// The place of the `occurrence`-th time (counting from 0) that `needle` occurs in `file`, as code that no macro wrote.
        fn place(&self, file: &str, needle: &str, occurrence: usize) -> Place {
            let text = std::fs::read_to_string(self.path(file)).unwrap();
            let byte = text
                .match_indices(needle)
                .nth(occurrence)
                .unwrap_or_else(|| panic!("{needle} does not occur {occurrence} times in {file}"))
                .0;
            Place {
                file: self.path(file),
                byte,
                expansion: None,
            }
        }

        /// The place of `needle` in `file`, written by the macro defined at `definition` and called at `call`, both in `file`.
        fn in_macro(&self, file: &str, needle: &str, definition: &str, call: &str) -> Place {
            let definition = self.place(file, definition, 0);
            Place {
                expansion: Some(Box::new(Expansion {
                    definition: Some((definition.file, definition.byte)),
                    call_site: self.place(file, call, 0),
                })),
                ..self.place(file, needle, 0)
            }
        }

        fn covers(&mut self, lint: &str, place: &Place, root: &str, config: &Config) -> bool {
            let root = self.path(root);
            self.model.covers(lint, place, &root, config).unwrap()
        }

        /// Whether an exception for `lint` covers the place in `lib.rs`, the crate root, in a unit that is not compiled as tests.
        fn covered(&mut self, lint: &str, place: &Place) -> bool {
            self.covers(lint, place, "lib.rs", &host(false))
        }
    }

    impl Drop for Sources {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }

    /// The file `name`, written with `/` between folders, below `folder`. Joined one part at a time, because Windows reads `/` as a separator only in paths without the `\\?\\` prefix that `canonicalize` gives.
    fn under(folder: &Path, name: &str) -> PathBuf {
        name.split('/')
            .fold(folder.to_path_buf(), |path, part| path.join(part))
    }

    /// The configuration of a unit on the host in the dev profile, compiled as tests or not.
    fn host(test: bool) -> Config {
        Config {
            test,
            debug_assertions: true,
            wasm32: false,
            canary_case: None,
        }
    }

    #[test]
    fn an_expect_on_a_function_covers_the_whole_function_and_nothing_after_it() {
        let mut sources = Sources::new(
            "function",
            &[(
                "lib.rs",
                r#"
#[expect(clippy::disallowed_types, reason = "only counted")]
#[must_use]
pub fn counted<K, V>() -> Result<usize, String> where K: Ord, V: Ord {
    let map: HashMap<K, V> = HashMap::new();
    Ok(map.len())
}

pub fn after() -> usize { HashMap::<u8, u8>::new().len() }
"#,
            )],
        );
        let inside = sources.place("lib.rs", "HashMap::new()", 0);
        assert!(sources.covered(MAP, &inside));
        assert!(!sources.covered("clippy::unwrap_used", &inside));
        let after = sources.place("lib.rs", "HashMap::<u8", 0);
        assert!(!sources.covered(MAP, &after));
    }

    #[test]
    fn an_expect_on_a_statement_covers_that_statement_only() {
        let mut sources = Sources::new(
            "statement",
            &[(
                "lib.rs",
                r#"
pub fn two() -> usize {
    #[expect(clippy::disallowed_types, reason = "only counted")]
    let first: HashMap<u8, u8> = HashMap::new();
    let second = HashMap::<u8, u8>::new();
    first.len() + second.len()
}
"#,
            )],
        );
        let first = sources.place("lib.rs", "HashMap::new()", 0);
        assert!(sources.covered(MAP, &first));
        let second = sources.place("lib.rs", "HashMap::<u8", 0);
        assert!(!sources.covered(MAP, &second));
    }

    #[test]
    fn an_expect_on_a_statement_that_is_not_an_item_covers_that_statement_only() {
        let mut sources = Sources::new(
            "expression-statement",
            &[(
                "lib.rs",
                r#"
pub fn statements() {
    #[expect(clippy::disallowed_types, reason = "r")]
    drop(HashMap::<u8, u8>::new());
    let set = HashSet::<u8>::new();
    #[expect(clippy::disallowed_types, reason = "r")]
    drop(HashMap::<u8, u8>::new());
    drop(set);
    drop(HashMap::<u8, u8>::new());
}
"#,
            )],
        );
        for (needle, occurrence, covered) in [
            ("HashMap", 0, true),
            ("HashSet", 0, false),
            ("HashMap", 1, true),
            ("HashMap", 2, false),
        ] {
            let place = sources.place("lib.rs", needle, occurrence);
            assert_eq!(
                sources.covered(MAP, &place),
                covered,
                "{needle} {occurrence}"
            );
        }
    }

    #[test]
    fn an_expect_on_a_field_covers_that_field_only() {
        let mut sources = Sources::new(
            "field",
            &[(
                "lib.rs",
                r#"
pub struct Maps {
    #[expect(clippy::disallowed_types, reason = "only counted")]
    pub first: HashMap<u8, u8>,
    pub second: HashMap<u8, u8>,
}
"#,
            )],
        );
        let first = sources.place("lib.rs", "HashMap", 0);
        assert!(sources.covered(MAP, &first));
        let second = sources.place("lib.rs", "HashMap", 1);
        assert!(!sources.covered(MAP, &second));
    }

    #[test]
    fn an_expect_on_a_match_arm_covers_that_arm_only() {
        let mut sources = Sources::new(
            "arm",
            &[(
                "lib.rs",
                r#"
pub fn pick(x: u8) -> usize {
    match x {
        #[expect(clippy::disallowed_types, reason = "only counted")]
        1 => HashMap::<u8, u8>::new().len(),
        _ => HashSet::<u8>::new().len(),
    }
}
"#,
            )],
        );
        let first = sources.place("lib.rs", "HashMap", 0);
        assert!(sources.covered(MAP, &first));
        let second = sources.place("lib.rs", "HashSet", 0);
        assert!(!sources.covered(MAP, &second));
    }

    #[test]
    fn an_expect_on_a_module_declaration_covers_its_file_and_its_submodules() {
        let mut sources = Sources::new(
            "module",
            &[
                (
                    "lib.rs",
                    "#[cfg(test)]\n#[expect(unsafe_code, reason = \"the tests call the C functions\")]\nmod tests;\nmod other;\n",
                ),
                ("tests.rs", "mod deeper;\nfn a() { unsafe {} }\n"),
                ("tests/deeper.rs", "fn b() { unsafe {} }\n"),
                ("other.rs", "fn c() { unsafe {} }\n"),
            ],
        );
        for file in ["tests.rs", "tests/deeper.rs"] {
            let place = sources.place(file, "unsafe", 0);
            assert!(sources.covered("unsafe_code", &place), "{file}");
        }
        let other = sources.place("other.rs", "unsafe", 0);
        assert!(!sources.covered("unsafe_code", &other));
    }

    #[test]
    fn a_crate_level_expect_covers_the_whole_crate_and_an_inner_one_its_module() {
        let mut sources = Sources::new(
            "crate-level",
            &[
                (
                    "main.rs",
                    "#![expect(clippy::print_stdout, reason = \"a tool reports to the person running it\")]\nmod report;\nfn main() { println!(\"a\"); }\n",
                ),
                ("report.rs", "mod inline {\n    mod nested;\n}\n"),
                ("report/inline/nested.rs", "fn b() { println!(\"b\"); }\n"),
                (
                    "lib.rs",
                    "mod quiet;\nmod loud;\nfn d() { println!(\"d\"); }\n",
                ),
                (
                    "quiet.rs",
                    "#![expect(clippy::print_stdout, reason = \"r\")]\nmod deeper;\n",
                ),
                ("quiet/deeper.rs", "fn e() { println!(\"e\"); }\n"),
                ("loud.rs", "fn f() { println!(\"f\"); }\n"),
            ],
        );
        let print = "clippy::print_stdout";
        for file in ["main.rs", "report/inline/nested.rs"] {
            let place = sources.place(file, "println", 0);
            assert!(
                sources.covers(print, &place, "main.rs", &host(false)),
                "{file}"
            );
        }
        for (file, covered) in [
            ("lib.rs", false),
            ("quiet/deeper.rs", true),
            ("loud.rs", false),
        ] {
            let place = sources.place(file, "println", 0);
            assert_eq!(sources.covered(print, &place), covered, "{file}");
        }
    }

    #[test]
    fn an_expect_inside_cfg_attr_covers_only_where_its_condition_holds() {
        let mut sources = Sources::new(
            "cfg-attr",
            &[(
                "lib.rs",
                r#"
#[cfg_attr(test, expect(dead_code, reason = "only the build that is not a test uses it"))]
fn helper() -> u8 { 1 }

#[cfg_attr(all(not(debug_assertions), target_arch = "wasm32"), expect(dead_code, reason = "r"))]
fn other() -> u8 { 2 }
"#,
            )],
        );
        let helper = sources.place("lib.rs", "helper", 0);
        assert!(sources.covers("dead_code", &helper, "lib.rs", &host(true)));
        assert!(!sources.covers("dead_code", &helper, "lib.rs", &host(false)));
        let other = sources.place("lib.rs", "other", 0);
        let release_wasm = Config {
            test: false,
            debug_assertions: false,
            wasm32: true,
            canary_case: None,
        };
        assert!(sources.covers("dead_code", &other, "lib.rs", &release_wasm));
        assert!(!sources.covered("dead_code", &other));
    }

    #[test]
    fn an_expect_in_a_macros_own_code_covers_what_the_macro_writes_next_to_it() {
        let mut sources = Sources::new(
            "macro-own-code",
            &[(
                "lib.rs",
                r#"
macro_rules! counted {
    ($name:ident, $value:ty) => {
        /// A count.
        #[expect(clippy::disallowed_types, reason = "only counted")]
        pub fn $name() -> usize {
            let map: HashMap<u8, $value> = HashMap::new();
            map.len()
        }
        pub fn plain() -> usize { HashSet::<u8>::new().len() }
    };
}
counted!(entries, u8);
"#,
            )],
        );
        let inside = sources.in_macro("lib.rs", "HashMap::new()", "macro_rules", "counted!(");
        assert!(sources.covered(MAP, &inside));
        let next = sources.in_macro("lib.rs", "HashSet", "macro_rules", "counted!(");
        assert!(!sources.covered(MAP, &next));
    }

    #[test]
    fn an_expect_in_a_macros_own_code_covers_what_the_macro_writes_at_every_call() {
        let mut sources = Sources::new(
            "macro-own-code-twice",
            &[(
                "lib.rs",
                r#"
macro_rules! counted {
    ($name:ident) => {
        #[expect(clippy::disallowed_types, reason = "only counted")]
        pub fn $name() -> usize { HashMap::<u8, u8>::new().len() }
    };
}
counted!(first);
pub fn caller() -> usize {
    counted!(second);
    second()
}
"#,
            )],
        );
        for call in ["counted!(first)", "counted!(second)"] {
            let place = sources.in_macro("lib.rs", "HashMap", "macro_rules", call);
            assert!(sources.covered(MAP, &place), "{call}");
        }
    }

    #[test]
    fn an_expect_on_a_macro_definition_covers_nothing_in_its_body() {
        let mut sources = Sources::new(
            "macro-definition",
            &[(
                "lib.rs",
                r#"
#[expect(clippy::disallowed_types, reason = "does not apply where the macro expands")]
macro_rules! counted {
    () => {
        pub fn entries() -> usize { HashMap::<u8, u8>::new().len() }
    };
}
counted!();
"#,
            )],
        );
        let written = sources.in_macro("lib.rs", "HashMap", "macro_rules", "counted!(");
        assert!(!sources.covered(MAP, &written));
        let plain = sources.place("lib.rs", "HashMap", 0);
        assert!(!sources.covered(MAP, &plain));
    }

    #[test]
    fn an_expect_around_a_macro_call_covers_nothing_that_the_macro_writes() {
        let mut sources = Sources::new(
            "macro-call",
            &[(
                "lib.rs",
                r#"
macro_rules! counted {
    () => {
        pub fn entries() -> usize { HashMap::<u8, u8>::new().len() }
    };
}
#[expect(clippy::disallowed_types, reason = "the compiler reports it as an unused attribute")]
counted!();

/// Where the expansion of a macro inherits the levels of the code around its call.
#[expect(clippy::disallowed_types, reason = "the call is inside the function")]
pub fn around() -> usize {
    counted!();
    entries()
}
"#,
            )],
        );
        let first = sources.in_macro("lib.rs", "HashMap", "macro_rules", "counted!();");
        assert!(!sources.covered(MAP, &first));
        // The second call lies inside the function, so the code it writes is covered there.
        let second = sources.in_macro(
            "lib.rs",
            "HashMap",
            "macro_rules",
            "counted!();\n    entries",
        );
        assert!(sources.covered(MAP, &second));
    }

    /// Forms 1 and 2 of the work package: a macro builds `#[allow(…)]` or `#[expect(…)]` from the `#` of a doc attribute and a bracket group handed in separately.
    #[test]
    fn an_exception_that_a_macro_assembles_covers_nothing() {
        let mut sources = Sources::new(
            "split",
            &[(
                "lib.rs",
                r#"
macro_rules! exempt { ($hash:tt $decoy:tt, $bracket:tt) => { $hash $bracket /// The number of entries of an empty map.
#[must_use] pub fn entries() -> usize { let map: std::collections::HashMap<u8, u8> = std::collections::HashMap::new(); map.len() } }; }
exempt!(#[doc = "Entries."], [allow(clippy::disallowed_types)]);
exempt!(#[doc = "Entries."], [expect(clippy::disallowed_types, reason = "an exception that a macro assembles")]);
"#,
            )],
        );
        for call in [
            "exempt!(#[doc = \"Entries.\"], [allow",
            "exempt!(#[doc = \"Entries.\"], [expect",
        ] {
            let place = sources.in_macro(
                "lib.rs",
                "std::collections::HashMap::new",
                "macro_rules",
                call,
            );
            assert!(!sources.covered(MAP, &place), "{call}");
        }
    }

    /// Form 3 of the work package: an `#[allow]` written in full inside the input of another crate's macro, which puts it out.
    #[test]
    fn an_allow_inside_another_crates_macro_covers_nothing() {
        let mut sources = Sources::new(
            "foreign-macro",
            &[(
                "lib.rs",
                r#"
std::thread_local! {
    /// A map under an `#[allow]` written in full.
    #[allow(clippy::disallowed_types)]
    static MAP: core::cell::RefCell<std::collections::HashMap<u8, u8>> = core::cell::RefCell::new(std::collections::HashMap::new());
}
"#,
            )],
        );
        for occurrence in 0..2 {
            let place = sources.place("lib.rs", "std::collections::HashMap", occurrence);
            assert!(!sources.covered(MAP, &place));
        }
    }

    #[test]
    fn only_an_expect_written_out_in_full_that_names_the_lint_counts() {
        let mut sources = Sources::new(
            "written-in-full",
            &[(
                "lib.rs",
                r#"
#[allow(clippy::disallowed_types, reason = "an allow")]
pub fn allowed() -> usize { HashMap::<u8, u8>::new().len() }
#[expect(clippy::disallowed_types)]
pub fn without_reason() -> usize { HashMap::<u8, u8>::new().len() }
#[expect(clippy::all, reason = "a group")]
pub fn group() -> usize { HashMap::<u8, u8>::new().len() }
#[expect(clippy::disallowed_types, clippy::unwrap_used, reason = "two lints")]
pub fn two() -> usize { HashMap::<u8, u8>::new().len() }
"#,
            )],
        );
        for (occurrence, covered) in [(0, false), (1, false), (2, false), (3, true)] {
            let place = sources.place("lib.rs", "HashMap", occurrence);
            assert_eq!(
                sources.covered(MAP, &place),
                covered,
                "occurrence {occurrence}"
            );
        }
    }

    /// The macro decides where the attribute and the tokens after it go, which the model cannot know, also where the macro passes them on unchanged, as `thread_local!` does.
    #[test]
    fn an_expect_inside_a_macros_input_covers_nothing() {
        let mut sources = Sources::new(
            "macro-input",
            &[(
                "lib.rs",
                r#"
sneaky! {
    #[expect(clippy::disallowed_types, reason = "decoy")] fn decoy() -> HashMap<u8, u8>,
    fn real() -> usize { HashSet::<u8>::new().len() }
}
std::thread_local! {
    #[expect(clippy::disallowed_types, reason = "passed on unchanged")]
    static MAP: core::cell::RefCell<HashMap<u8, u8>> = core::cell::RefCell::new(HashMap::new());
}
"#,
            )],
        );
        for occurrence in 0..3 {
            let place = sources.place("lib.rs", "HashMap", occurrence);
            assert!(!sources.covered(MAP, &place), "occurrence {occurrence}");
        }
        let real = sources.place("lib.rs", "HashSet", 0);
        assert!(!sources.covered(MAP, &real));
    }

    #[test]
    fn in_a_transcriber_an_expect_ends_at_a_metavariable_that_could_end_the_item() {
        let mut sources = Sources::new(
            "metavariable",
            &[(
                "lib.rs",
                r#"
macro_rules! two {
    ($($rest:tt)*) => {
        #[expect(clippy::disallowed_types, reason = "r")]
        fn first() -> u8 $($rest)* fn second() -> usize { HashMap::<u8, u8>::new().len() }
    };
}
two!({ 1 });
"#,
            )],
        );
        let second = sources.in_macro("lib.rs", "HashMap", "macro_rules", "two!(");
        assert!(!sources.covered(MAP, &second));
    }

    /// The compiler covers neither a generic parameter's bounds nor its default, nor its name for the lints of its later passes, and on a pattern's field it applies the attribute to almost nothing; a struct expression's fields look like a pattern's.
    #[test]
    fn an_expect_on_a_generic_parameter_or_a_field_of_a_struct_expression_or_pattern_covers_nothing()
     {
        let mut sources = Sources::new(
            "nothing",
            &[(
                "lib.rs",
                r#"
pub struct Generic<'a, #[expect(deprecated, reason = "r")] T: OldTrait + 'a = Old>(&'a T);
pub fn generic<#[expect(non_upper_case_globals, reason = "r")] const n: usize>() -> usize { n + HashMap::<u8, u8>::new().len() }
pub fn literal() -> Fields {
    Fields { #[expect(clippy::disallowed_types, reason = "r")] r#fn: HashSet::new(), rest: HashMap::new() }
}
pub fn destructure(fields: Fields) {
    let Fields { #[expect(non_snake_case, reason = "r")] r#fn: Set, .. } = fields;
}
"#,
            )],
        );
        for (lint, needle) in [
            ("deprecated", "T: OldTrait"),
            ("deprecated", "OldTrait"),
            ("deprecated", "Old>"),
            ("non_upper_case_globals", "n: usize"),
            ("non_upper_case_globals", "n + HashMap"),
            (MAP, "HashMap::<u8, u8>::new().len() }"),
            (MAP, "HashSet::new()"),
            (MAP, "HashMap::new()"),
            ("non_snake_case", "Set, .."),
        ] {
            let place = sources.place("lib.rs", needle, 0);
            assert!(!sources.covered(lint, &place), "{needle}");
        }
    }

    /// Required by the review of CORE-009: the compiler applies an attribute on a parameter to the parameter's pattern, never to its type, which belongs to the signature, nor to a closure's body or its other parameters.
    #[test]
    fn an_expect_on_a_parameter_covers_its_pattern_only() {
        let mut sources = Sources::new(
            "parameter",
            &[(
                "lib.rs",
                r#"
pub fn typed(#[expect(deprecated, reason = "r")] Old: Old, next: Old) {}
pub fn closures() -> u8 {
    let typed = |#[expect(non_snake_case, deprecated, reason = "r")] Param: Old| { let Body = Param; Body };
    let plain = |#[expect(non_snake_case, reason = "r")] First, Second| First + Second + Tail;
    typed(1) + plain(1, 2)
}
pub fn pattern(#[expect(non_snake_case, deprecated, reason = "r")] Point { x: X, .. }: Point) -> u8 { X }
"#,
            )],
        );
        for (lint, needle, occurrence, covered) in [
            // A function's parameter: its pattern, not its type, nor the next parameter.
            ("deprecated", "Old", 0, true),
            ("deprecated", "Old", 1, false),
            ("deprecated", "Old", 2, false),
            // A closure's parameter: its pattern, not its type, nor the closure's body.
            ("non_snake_case", "Param", 0, true),
            ("deprecated", "Old", 3, false),
            ("non_snake_case", "Body", 0, false),
            ("non_snake_case", "First", 0, true),
            ("non_snake_case", "Second", 0, false),
            ("non_snake_case", "Tail", 0, false),
            // A pattern with braces: the whole pattern, not the type after it, nor the body.
            ("non_snake_case", "X", 0, true),
            ("deprecated", "Point", 1, false),
            ("non_snake_case", "X", 1, false),
        ] {
            let place = sources.place("lib.rs", needle, occurrence);
            assert_eq!(
                sources.covered(lint, &place),
                covered,
                "{needle} {occurrence}"
            );
        }
    }

    #[test]
    fn an_expect_on_a_variant_or_a_field_of_a_tuple_covers_it() {
        let mut sources = Sources::new(
            "variant",
            &[(
                "lib.rs",
                r#"
pub struct Pair(#[expect(clippy::disallowed_types, reason = "r")] pub HashMap<u8, u8>, pub HashSet<u8>);
pub enum Kinds {
    #[expect(clippy::disallowed_types, reason = "r")]
    Map { map: HashMap<u8, u8> },
    Set(#[expect(clippy::disallowed_types, reason = "r")] HashSet<u8>, HashMap<u8, u8>),
    Other(HashMap<u8, u8>),
}
"#,
            )],
        );
        for (needle, occurrence, covered) in [
            ("HashMap", 0, true),
            ("HashSet", 0, false),
            ("HashMap", 1, true),
            ("HashSet", 1, true),
            ("HashMap", 2, false),
            ("HashMap", 3, false),
        ] {
            let place = sources.place("lib.rs", needle, occurrence);
            assert_eq!(
                sources.covered(MAP, &place),
                covered,
                "{needle} {occurrence}"
            );
        }
    }

    #[test]
    fn an_expect_covers_the_built_in_attributes_of_its_item_but_not_a_derive() {
        let mut sources = Sources::new(
            "attributes",
            &[(
                "lib.rs",
                r#"
#[expect(unsafe_code, reason = "exported to C without name mangling")]
#[derive(Debug)]
#[unsafe(no_mangle)]
pub extern "C" fn version() -> u8 { 1 }
"#,
            )],
        );
        let unsafe_attribute = sources.place("lib.rs", "unsafe(no_mangle)", 0);
        assert!(sources.covered("unsafe_code", &unsafe_attribute));
        let derive = sources.place("lib.rs", "Debug", 0);
        assert!(!sources.covered("unsafe_code", &derive));
    }

    #[test]
    fn a_file_that_the_source_check_did_not_read_is_never_covered() {
        let mut sources = Sources::new(
            "unread",
            &[(
                "lib.rs",
                "#![expect(dead_code, reason = \"r\")]\nfn a() {}\n",
            )],
        );
        let outside = Place {
            file: PathBuf::from("/rustc/library/std/src/thread/local.rs"),
            byte: 0,
            expansion: None,
        };
        assert!(!sources.covered("dead_code", &outside));
        assert!(!sources.model.reads(&outside.file));
        let inside = sources.place("lib.rs", "fn a", 0);
        assert!(sources.covered("dead_code", &inside));
    }

    #[test]
    fn counts_lines_from_one() {
        let mut sources = Sources::new("lines", &[("lib.rs", "a\nbc\n\nd")]);
        let file = sources.path("lib.rs");
        for (byte, line) in [(0, 1), (1, 1), (2, 2), (4, 2), (5, 3), (6, 4)] {
            assert_eq!(
                sources.model.line_of(&file, byte).unwrap(),
                line,
                "byte {byte}"
            );
        }
    }
}
