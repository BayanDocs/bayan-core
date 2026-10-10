//! The `bayan-lab corpus …` commands.
//!
//! Each command reads its arguments, does its work, and writes what it did to `out` and problems to `err`, so tests can run it in-process. [`run`] returns the process's exit status: 0 when everything succeeded, 1 when some documents or checks failed (refused documents, a failed verification), and 2 when the command could not run at all (wrong arguments, an unreadable manifest, an unreachable store).
//!
//! Messages identify documents by SHA-256 and by the provenance path the operator chose to record (public sources only); they never contain document content (LAB-001, AGENTS.md §6).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{ErrorKind, Read as _, Write};
use std::path::{Path, PathBuf};

use crate::exclusions::{Exclusions, MAX_LIST_SIZE};
use crate::hash::Sha256;
use crate::manifest::{
    self, Document, GroundTruth, LicenseText, Manifest, NO_ASSERTION, PUBLIC_LICENSES, Provenance,
    Scan, Source, Tier,
};
use crate::scan::{self, Limits, TAGGER_VERSION, printable};
use crate::schema;
use crate::stats::Stats;
use crate::store::{self, Key, MAX_OBJECT_SIZE, Store};

/// The largest license or notice file accepted, in bytes.
const MAX_LICENSE_TEXT: u64 = 1024 * 1024;

/// The usage text.
pub const USAGE: &str = "\
Usage: bayan-lab corpus <command> [options]

Commands:
  source add NAME --url URL --revision REV --license SPDX --copyright TEXT
                  --license-file PATH=FILE... [--notes TEXT]
                      Register a source and store its license texts. Sources never change:
                      a new revision is a new source.
  add --source NAME [--tier T1] [--root DIR] [--notes TEXT] [--exclude LIST] FILE...
                      Hash, deduplicate, tag and store documents. With --root, each FILE is a
                      path relative to DIR and is recorded as the document's provenance path;
                      without it, no path is recorded (always so for private documents).
                      Documents the exclusion list LIST names are skipped. Documents already
                      in the manifest are stored again if the store lacks them.
  tag [--all]         Tag again the documents whose tags are missing or older than this
                      tagger (version 1); --all tags every document again.
  verify [--exclude LIST]
                      Check the manifest, and with a store every object's SHA-256; with an
                      exclusion list, also that the manifest contains none of its documents.
  list [--feature NAME] [--font NAME] [--script NAME] [--source NAME] [--format text|json]
                      List documents, optionally filtered.
  stats [--format markdown|json] [--title TEXT]
                      Frequencies by feature, font, script, language and compatibility mode.
  schema              Print the JSON Schema of the manifest.

Options for every command:
  --manifest FILE     The corpus manifest (or the environment variable BAYAN_LAB_MANIFEST).
  --store ADDRESS     A directory; s3://BUCKET/PREFIX?endpoint=https://HOST&region=REGION; or
                      https://HOST/PATH, read-only and anonymous (or BAYAN_LAB_STORE). S3
                      credentials come only from AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY and
                      AWS_SESSION_TOKEN.

Exit status: 0 success, 1 some documents or checks failed, 2 the command could not run.";

/// How a command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Everything succeeded.
    Success,
    /// Some documents or checks failed; the messages say which.
    Failed,
    /// The command could not run.
    Error,
}

impl Outcome {
    /// The process exit status.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Failed => 1,
            Self::Error => 2,
        }
    }
}

/// Command-line arguments split into options and operands.
#[derive(Debug, Default)]
struct Arguments {
    options: BTreeMap<String, Vec<String>>,
    flags: BTreeSet<String>,
    operands: Vec<String>,
}

impl Arguments {
    /// Splits `args`. Options listed in `valued` take a value (`--name value` or `--name=value`) and may repeat; those in `flags` take none; `--` ends the options.
    fn parse(args: &[String], valued: &[&str], flags: &[&str]) -> Result<Self, String> {
        let mut parsed = Self::default();
        let mut index = 0;
        while let Some(arg) = args.get(index) {
            index += 1;
            if arg == "--" {
                parsed.operands.extend(args.iter().skip(index).cloned());
                break;
            }
            let Some(option) = arg.strip_prefix("--") else {
                parsed.operands.push(arg.clone());
                continue;
            };
            let (name, inline) = match option.split_once('=') {
                Some((name, value)) => (name, Some(value.to_owned())),
                None => (option, None),
            };
            if valued.contains(&name) {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let value = args
                            .get(index)
                            .ok_or_else(|| format!("--{name} needs a value"))?;
                        index += 1;
                        value.clone()
                    }
                };
                parsed
                    .options
                    .entry(name.to_owned())
                    .or_default()
                    .push(value);
            } else if flags.contains(&name) && inline.is_none() {
                parsed.flags.insert(name.to_owned());
            } else {
                return Err(format!("unknown option --{name}"));
            }
        }
        Ok(parsed)
    }

    /// The value of an option that may appear at most once.
    fn one(&self, name: &str) -> Result<Option<&str>, String> {
        match self.options.get(name).map(Vec::as_slice) {
            None | Some([]) => Ok(None),
            Some([value]) => Ok(Some(value.as_str())),
            Some(_) => Err(format!("--{name} may be given only once")),
        }
    }

    fn required(&self, name: &str) -> Result<&str, String> {
        self.one(name)?
            .ok_or_else(|| format!("--{name} is required"))
    }

    /// The curator's notes from `--notes`, refused when empty, before a command stores anything.
    fn notes(&self) -> Result<Option<String>, String> {
        match self.one("notes")? {
            Some(notes) if notes.trim().is_empty() => {
                Err("--notes cannot be empty; leave it out instead".to_owned())
            }
            notes => Ok(notes.map(str::to_owned)),
        }
    }

    fn all(&self, name: &str) -> &[String] {
        self.options.get(name).map_or(&[], Vec::as_slice)
    }
}

/// Runs `bayan-lab` with `args` (without the program name).
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Outcome {
    let result = match args.split_first() {
        Some((group, rest)) if group == "corpus" => corpus(rest, out, err),
        Some((help, _)) if matches!(help.as_str(), "help" | "--help" | "-h") => {
            let _ = writeln!(out, "{USAGE}");
            Ok(Outcome::Success)
        }
        _ => Err(format!("expected a command\n\n{USAGE}")),
    };
    match result {
        Ok(outcome) => outcome,
        Err(message) => {
            let _ = writeln!(err, "bayan-lab: {message}");
            Outcome::Error
        }
    }
}

/// The options every command accepts.
const COMMON: [&str; 2] = ["manifest", "store"];

fn corpus(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Result<Outcome, String> {
    let Some((command, rest)) = args.split_first() else {
        return Err(format!("expected a corpus command\n\n{USAGE}"));
    };
    match command.as_str() {
        "source" => match rest.split_first() {
            Some((sub, rest)) if sub == "add" => {
                let arguments = Arguments::parse(
                    rest,
                    &with_common(&[
                        "url",
                        "revision",
                        "license",
                        "copyright",
                        "license-file",
                        "notes",
                    ]),
                    &[],
                )?;
                source_add(&arguments, out)
            }
            _ => Err("expected `corpus source add`".to_owned()),
        },
        "add" => {
            let arguments = Arguments::parse(
                rest,
                &with_common(&["source", "tier", "root", "notes", "exclude"]),
                &[],
            )?;
            add(&arguments, out, err)
        }
        "tag" => {
            let arguments = Arguments::parse(rest, &COMMON, &["all"])?;
            tag(&arguments, out, err)
        }
        "verify" => {
            let arguments = Arguments::parse(rest, &with_common(&["exclude"]), &[])?;
            verify(&arguments, out, err)
        }
        "list" => {
            let arguments = Arguments::parse(
                rest,
                &with_common(&["feature", "font", "script", "source", "format"]),
                &[],
            )?;
            list(&arguments, out)
        }
        "stats" => {
            let arguments = Arguments::parse(rest, &with_common(&["format", "title"]), &[])?;
            stats(&arguments, out)
        }
        "schema" => {
            let text = schema::json_schema_file()?;
            out.write_all(text.as_bytes())
                .map_err(|error| error.to_string())?;
            Ok(Outcome::Success)
        }
        other => Err(format!("unknown corpus command `{other}`\n\n{USAGE}")),
    }
}

fn with_common(options: &[&'static str]) -> Vec<&'static str> {
    let mut all = COMMON.to_vec();
    all.extend_from_slice(options);
    all
}

/// The manifest path from `--manifest` or `BAYAN_LAB_MANIFEST`.
fn manifest_path(arguments: &Arguments) -> Result<PathBuf, String> {
    match arguments.one("manifest")? {
        Some(path) => Ok(PathBuf::from(path)),
        None => std::env::var_os("BAYAN_LAB_MANIFEST")
            .map(PathBuf::from)
            .ok_or_else(|| {
                "give the manifest with --manifest FILE (or BAYAN_LAB_MANIFEST)".to_owned()
            }),
    }
}

/// The store from `--store` or `BAYAN_LAB_STORE`, if either is given.
fn store_of(arguments: &Arguments) -> Result<Option<Box<dyn Store>>, String> {
    let address = match arguments.one("store")? {
        Some(address) => Some(address.to_owned()),
        None => std::env::var("BAYAN_LAB_STORE")
            .ok()
            .filter(|value| !value.is_empty()),
    };
    address
        .map(|address| store::open(&address).map_err(|error| error.to_string()))
        .transpose()
}

fn required_store(arguments: &Arguments) -> Result<Box<dyn Store>, String> {
    store_of(arguments)?
        .ok_or_else(|| "give the store with --store ADDRESS (or BAYAN_LAB_STORE)".to_owned())
}

/// Reads the manifest; a manifest that does not exist yet is empty when `create` is set.
fn load(path: &Path, create: bool) -> Result<Manifest, String> {
    match fs::read_to_string(path) {
        Ok(text) => Manifest::from_json(&text),
        Err(error) if create && error.kind() == ErrorKind::NotFound => Ok(Manifest::default()),
        Err(error) => Err(format!(
            "cannot read the manifest {}: {error}",
            path.display()
        )),
    }
}

/// Writes the manifest through a temporary file, so an interrupted write never leaves half a manifest.
fn save(path: &Path, manifest: &Manifest) -> Result<(), String> {
    let text = manifest.to_json()?;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".partial");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, text).map_err(|error| format!("cannot write the manifest: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("cannot write the manifest: {error}"))
}

/// The problems a manifest has, so that a command can refuse to add new ones.
fn problems_of(manifest: &Manifest) -> BTreeSet<String> {
    manifest
        .check()
        .err()
        .map(|problems| problems.0.into_iter().collect())
        .unwrap_or_default()
}

/// Writes the manifest unless the command made it inconsistent: problems it had before (`before`) are left for `corpus verify` to report, but no command may add one.
fn save_checked(path: &Path, manifest: &Manifest, before: &BTreeSet<String>) -> Result<(), String> {
    let new: Vec<String> = problems_of(manifest)
        .into_iter()
        .filter(|problem| !before.contains(problem))
        .collect();
    if !new.is_empty() {
        return Err(format!(
            "the manifest was not changed, because the change would make it inconsistent:\n- {}",
            new.join("\n- ")
        ));
    }
    save(path, manifest)
}

/// Reads a local file of at most `limit` bytes.
fn read_limited(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if !u64::try_from(bytes.len()).is_ok_and(|len| len <= limit) {
        return Err("larger than the limit".to_owned());
    }
    Ok(bytes)
}

/// The exclusion list from `--exclude`, if given.
fn exclusions_of(arguments: &Arguments) -> Result<Option<Exclusions>, String> {
    let Some(path) = arguments.one("exclude")? else {
        return Ok(None);
    };
    let bytes = read_limited(Path::new(path), MAX_LIST_SIZE)
        .map_err(|error| format!("cannot read the exclusion list {path}: {error}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| format!("the exclusion list {path} is not UTF-8 text"))?;
    Exclusions::parse(&text)
        .map(Some)
        .map_err(|problems| format!("the exclusion list {path} is not valid:\n{problems}"))
}

/// The file at the relative path `relative` below `root`, provided that it is a regular file and that no folder or file on the way from `root` to it is a symbolic link. With `--root`, someone else chose the files (the authors of a source's repository), and a link could otherwise lead the tool to a file outside it, such as a private document of the operator's, which would then be published with the corpus.
fn file_inside(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    let mut regular = false;
    for segment in relative.split('/') {
        path.push(segment);
        let kind = fs::symlink_metadata(&path)
            .map_err(|error| format!("cannot read it: {error}"))?
            .file_type();
        if kind.is_symlink() {
            return Err("a symbolic link is on its path, which --root does not follow".to_owned());
        }
        regular = kind.is_file();
    }
    if regular {
        Ok(path)
    } else {
        Err("not a regular file".to_owned())
    }
}

/// Stores an object, read from the file `source`, unless the store already has it.
fn store_once(store: &dyn Store, key: &Key, bytes: &[u8], source: &Path) -> Result<(), String> {
    if store.contains(key).map_err(|error| error.to_string())? {
        return Ok(());
    }
    store
        .put(key, bytes, source)
        .map_err(|error| error.to_string())
}

fn source_add(arguments: &Arguments, out: &mut dyn Write) -> Result<Outcome, String> {
    let [name] = arguments.operands.as_slice() else {
        return Err("`corpus source add` takes one source name".to_owned());
    };
    let manifest_path = manifest_path(arguments)?;
    let store = required_store(arguments)?;
    let mut manifest = load(&manifest_path, true)?;
    let license = arguments.required("license")?;
    if !PUBLIC_LICENSES.contains(&license) && license != NO_ASSERTION {
        return Err(format!(
            "license `{license}` is not one of {} or {NO_ASSERTION}",
            PUBLIC_LICENSES.join(", ")
        ));
    }
    // Every license file is read and checked before anything is stored, so a refused source leaves nothing behind in the store.
    let mut texts = Vec::new();
    for pair in arguments.all("license-file") {
        let (path, local) = pair
            .split_once('=')
            .ok_or_else(|| format!("--license-file takes PATH=FILE, not `{pair}`"))?;
        if !manifest::is_relative_path(path) {
            return Err(format!(
                "license file path `{}` is not a relative path in the source",
                printable(path, 300)
            ));
        }
        let bytes = read_limited(Path::new(local), MAX_LICENSE_TEXT)
            .map_err(|error| format!("cannot read the license file for {path}: {error}"))?;
        texts.push((path.to_owned(), PathBuf::from(local), bytes));
    }
    let mut license_texts: Vec<LicenseText> = texts
        .iter()
        .map(|(path, _, bytes)| LicenseText {
            path: path.clone(),
            sha256: Sha256::of(bytes),
        })
        .collect();
    license_texts.sort();
    license_texts.dedup();
    let store_texts = || -> Result<(), String> {
        for (_, local, bytes) in &texts {
            store_once(
                store.as_ref(),
                &Key::license_text(Sha256::of(bytes)),
                bytes,
                local,
            )?;
        }
        Ok(())
    };
    let source = Source {
        url: arguments.required("url")?.to_owned(),
        revision: arguments.required("revision")?.to_owned(),
        license: license.to_owned(),
        copyright: arguments.required("copyright")?.to_owned(),
        license_texts,
        notes: arguments.notes()?,
    };
    match manifest.sources.get(name) {
        Some(existing) if *existing == source => {
            // The store may be a new one: give it the texts it lacks.
            store_texts()?;
            let _ = writeln!(out, "source {name}: unchanged");
            return Ok(Outcome::Success);
        }
        Some(_) => {
            return Err(format!(
                "source `{name}` already exists with different details; sources never change, so register a new revision under a new name"
            ));
        }
        None => {}
    }
    let before = problems_of(&manifest);
    manifest.sources.insert(name.clone(), source);
    let new: Vec<String> = problems_of(&manifest)
        .into_iter()
        .filter(|problem| !before.contains(problem))
        .collect();
    if !new.is_empty() {
        return Err(new.join("; "));
    }
    store_texts()?;
    save(&manifest_path, &manifest)?;
    let _ = writeln!(out, "source {name}: added");
    Ok(Outcome::Success)
}

fn add(arguments: &Arguments, out: &mut dyn Write, err: &mut dyn Write) -> Result<Outcome, String> {
    let manifest_path = manifest_path(arguments)?;
    let store = required_store(arguments)?;
    let mut manifest = load(&manifest_path, true)?;
    let source_name = arguments.required("source")?;
    let source = manifest
        .sources
        .get(source_name)
        .ok_or_else(|| {
            format!("source `{source_name}` is not registered; add it with `corpus source add`")
        })?
        .clone();
    let tier: Tier = arguments.one("tier")?.unwrap_or("T1").parse()?;
    if tier == Tier::T2 && source.license != NO_ASSERTION {
        return Err(format!(
            "private (T2) documents need a source whose license is {NO_ASSERTION}"
        ));
    }
    if tier != Tier::T2 && source.license == NO_ASSERTION {
        return Err("documents of public tiers need a source with a license".to_owned());
    }
    let root = arguments.one("root")?.map(PathBuf::from);
    if tier == Tier::T2 && root.is_some() {
        return Err(
            "private (T2) documents never record paths, so --root cannot be used with --tier T2"
                .to_owned(),
        );
    }
    let notes = arguments.notes()?;
    let exclusions = exclusions_of(arguments)?.unwrap_or_default();
    if arguments.operands.is_empty() {
        return Err("give at least one FILE".to_owned());
    }
    let before = problems_of(&manifest);
    let (mut added, mut known, mut excluded, mut refused) = (0_u64, 0_u64, 0_u64, 0_u64);
    for (index, operand) in arguments.operands.iter().enumerate() {
        let (file, path) = match &root {
            Some(root) => {
                if !manifest::is_relative_path(operand) {
                    let _ = writeln!(
                        err,
                        "refused input {}: with --root, files are given as relative paths inside it",
                        index + 1
                    );
                    refused += 1;
                    continue;
                }
                match file_inside(root, operand) {
                    Ok(file) => (file, Some(operand.clone())),
                    Err(error) => {
                        let _ = writeln!(err, "refused {}: {error}", printable(operand, 300));
                        refused += 1;
                        continue;
                    }
                }
            }
            None => (PathBuf::from(operand), None),
        };
        let label = path.as_deref().map_or_else(
            || format!("input {}", index + 1),
            |path| printable(path, 300),
        );
        let bytes = match read_limited(&file, MAX_OBJECT_SIZE) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = writeln!(err, "refused {label}: cannot read it: {error}");
                refused += 1;
                continue;
            }
        };
        let sha256 = Sha256::of(&bytes);
        if let Some(exclusion) = exclusions.get(&sha256) {
            let _ = writeln!(
                out,
                "excluded {} {label}: {}",
                sha256.short(),
                exclusion.reason
            );
            excluded += 1;
            continue;
        }
        let provenance = Provenance {
            source: source_name.to_owned(),
            path: path.clone(),
        };
        if let Some(path) = &path
            && let Some(other) = manifest.documents.iter().find(|document| {
                document.sha256 != sha256
                    && document.provenance.iter().any(|existing| {
                        existing.source == source_name && existing.path.as_ref() == Some(path)
                    })
            })
        {
            let _ = writeln!(
                err,
                "refused {label}: this source path is already recorded for {}",
                other.sha256.short()
            );
            refused += 1;
            continue;
        }
        let key = Key::document(sha256);
        if let Some(existing) = manifest
            .documents
            .iter_mut()
            .find(|document| document.sha256 == sha256)
        {
            // The same bytes cannot be public and private at once, nor be in two tiers.
            if existing.tier != tier {
                let _ = writeln!(
                    err,
                    "refused {label} ({}): already in the corpus as a {} document; a document has one tier",
                    sha256.short(),
                    existing.tier
                );
                refused += 1;
                continue;
            }
            if !existing.provenance.contains(&provenance) {
                existing.provenance.push(provenance);
            }
            // A document already in the manifest goes into the store too if the store lacks it, so that adding the same files to a new store fills it.
            if let Err(error) = store_once(store.as_ref(), &key, &bytes, &file) {
                save_checked(&manifest_path, &manifest, &before)?;
                return Err(format!("cannot store {label}: {error}"));
            }
            known += 1;
            let _ = writeln!(out, "known   {} {label}", sha256.short());
            continue;
        }
        let findings = match scan::scan(&bytes, &Limits::DEFAULT) {
            Ok(findings) => findings,
            Err(error) => {
                let _ = writeln!(
                    err,
                    "refused {label} ({}): {}",
                    sha256.short(),
                    error.message(path.is_some())
                );
                refused += 1;
                continue;
            }
        };
        if let Err(error) = store_once(store.as_ref(), &key, &bytes, &file) {
            // Save what was added so far, then stop: the store is not usable.
            save_checked(&manifest_path, &manifest, &before)?;
            return Err(format!("cannot store {label}: {error}"));
        }
        let document = Document {
            sha256,
            size: u64::try_from(bytes.len()).map_err(|error| error.to_string())?,
            tier,
            license: source.license.clone(),
            provenance: vec![provenance],
            scan: Some(Scan::from_findings(&findings)),
            page_count: None,
            ground_truth: GroundTruth::default(),
            notes: notes.clone(),
        };
        if manifest.insert(document).is_err() {
            known += 1;
            continue;
        }
        added += 1;
        let _ = writeln!(out, "added   {} {label}", sha256.short());
    }
    save_checked(&manifest_path, &manifest, &before)?;
    let _ = writeln!(
        out,
        "corpus add: {added} added, {known} already in the corpus, {excluded} excluded, {refused} refused"
    );
    Ok(if refused == 0 {
        Outcome::Success
    } else {
        Outcome::Failed
    })
}

fn tag(arguments: &Arguments, out: &mut dyn Write, err: &mut dyn Write) -> Result<Outcome, String> {
    let manifest_path = manifest_path(arguments)?;
    let store = required_store(arguments)?;
    let mut manifest = load(&manifest_path, false)?;
    let before = problems_of(&manifest);
    let everything = arguments.flags.contains("all");
    let (mut tagged, mut failed) = (0_u64, 0_u64);
    for document in &mut manifest.documents {
        let current = document
            .scan
            .as_ref()
            .is_some_and(|scan| scan.tagger == TAGGER_VERSION);
        if current && !everything {
            continue;
        }
        let id = document.sha256.short();
        let bytes = match store.get(&Key::document(document.sha256)) {
            Ok(Some(bytes)) if Sha256::of(&bytes) == document.sha256 => bytes,
            Ok(Some(_)) => {
                let _ = writeln!(err, "{id}: the stored object does not match its SHA-256");
                failed += 1;
                continue;
            }
            Ok(None) => {
                let _ = writeln!(err, "{id}: not in the store");
                failed += 1;
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        match scan::scan(&bytes, &Limits::DEFAULT) {
            Ok(findings) => {
                document.scan = Some(Scan::from_findings(&findings));
                tagged += 1;
            }
            Err(error) => {
                let _ = writeln!(
                    err,
                    "{id}: cannot be tagged: {}",
                    error.message(document.tier != Tier::T2)
                );
                failed += 1;
            }
        }
    }
    save_checked(&manifest_path, &manifest, &before)?;
    let _ = writeln!(
        out,
        "corpus tag: {tagged} tagged with tagger version {TAGGER_VERSION}, {failed} failed"
    );
    Ok(if failed == 0 {
        Outcome::Success
    } else {
        Outcome::Failed
    })
}

fn verify(
    arguments: &Arguments,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Outcome, String> {
    let manifest_path = manifest_path(arguments)?;
    let manifest = load(&manifest_path, false)?;
    let exclusions = exclusions_of(arguments)?;
    let mut failures = 0_usize;
    match manifest.check() {
        Ok(()) => {
            let _ = writeln!(
                out,
                "manifest: {} documents from {} sources, consistent",
                manifest.documents.len(),
                manifest.sources.len()
            );
        }
        Err(problems) => {
            failures += problems.0.len();
            let _ = write!(err, "manifest problems:\n{problems}");
        }
    }
    if let Some(exclusions) = &exclusions {
        let listed: Vec<&Document> = manifest
            .documents
            .iter()
            .filter(|document| exclusions.get(&document.sha256).is_some())
            .collect();
        for document in &listed {
            let _ = writeln!(
                err,
                "{}: the exclusion list excludes this document",
                document.sha256.short()
            );
        }
        failures += listed.len();
        let _ = writeln!(
            out,
            "exclusions: {} of the {} excluded documents in the manifest",
            listed.len(),
            exclusions.len()
        );
    }
    let Some(store) = store_of(arguments)? else {
        let _ = writeln!(
            out,
            "store: not checked (give --store to check every object's SHA-256)"
        );
        return Ok(finish_verify(failures, out));
    };
    let mut expected = BTreeSet::new();
    let mut texts_ok = 0_usize;
    for (name, source) in &manifest.sources {
        for text in &source.license_texts {
            let key = Key::license_text(text.sha256);
            expected.insert(key.path());
            match store.get(&key).map_err(|error| error.to_string())? {
                Some(bytes) if Sha256::of(&bytes) == text.sha256 => texts_ok += 1,
                Some(_) => {
                    failures += 1;
                    let _ = writeln!(
                        err,
                        "source {name}: license text {} does not match its SHA-256",
                        text.path
                    );
                }
                None => {
                    failures += 1;
                    let _ = writeln!(
                        err,
                        "source {name}: license text {} is not in the store",
                        text.path
                    );
                }
            }
        }
    }
    let mut documents_ok = 0_usize;
    for document in &manifest.documents {
        let key = Key::document(document.sha256);
        expected.insert(key.path());
        let id = document.sha256.short();
        match store.get(&key).map_err(|error| error.to_string())? {
            Some(bytes) if Sha256::of(&bytes) != document.sha256 => {
                failures += 1;
                let _ = writeln!(err, "{id}: the stored object does not match its SHA-256");
            }
            Some(bytes) if u64::try_from(bytes.len()).ok() != Some(document.size) => {
                failures += 1;
                let _ = writeln!(
                    err,
                    "{id}: size {} in the manifest, {} in the store",
                    document.size,
                    bytes.len()
                );
            }
            Some(_) => documents_ok += 1,
            None => {
                failures += 1;
                let _ = writeln!(err, "{id}: not in the store");
            }
        }
    }
    let unexpected = match store.list().map_err(|error| error.to_string())? {
        Some(listed) => {
            let unexpected: Vec<&String> = listed
                .iter()
                .filter(|path| !expected.contains(path.as_str()))
                .collect();
            for path in &unexpected {
                let _ = writeln!(
                    err,
                    "unexpected object in the store: {}",
                    printable(path, 300)
                );
            }
            failures += unexpected.len();
            format!("{} unexpected objects", unexpected.len())
        }
        None => "unexpected objects not checked (this store cannot be listed)".to_owned(),
    };
    let _ = writeln!(
        out,
        "store ({}): {documents_ok} of {} documents and {texts_ok} license texts present with matching SHA-256; {unexpected}",
        store.describe(),
        manifest.documents.len(),
    );
    Ok(finish_verify(failures, out))
}

fn finish_verify(failures: usize, out: &mut dyn Write) -> Outcome {
    if failures == 0 {
        let _ = writeln!(out, "corpus verify: OK");
        Outcome::Success
    } else {
        let _ = writeln!(out, "corpus verify: {failures} problems");
        Outcome::Failed
    }
}

fn list(arguments: &Arguments, out: &mut dyn Write) -> Result<Outcome, String> {
    let manifest = load(&manifest_path(arguments)?, false)?;
    let feature = arguments.one("feature")?;
    if let Some(name) = feature
        && scan::features::Feature::from_name(name).is_none()
    {
        return Err(format!("`{name}` is not a feature of the coverage matrix"));
    }
    let font = arguments.one("font")?;
    let script = arguments.one("script")?;
    let source = arguments.one("source")?;
    let json = match arguments.one("format")? {
        None | Some("text") => false,
        Some("json") => true,
        Some(other) => return Err(format!("unknown format `{other}`; use text or json")),
    };
    for document in &manifest.documents {
        let scan = document.scan.as_ref();
        let matches = feature.is_none_or(|name| {
            scan.is_some_and(|scan| scan.features.iter().any(|found| found == name))
        }) && font.is_none_or(|name| {
            scan.is_some_and(|scan| scan.fonts.iter().any(|found| found == name))
        }) && script.is_none_or(|name| {
            scan.is_some_and(|scan| scan.scripts.keys().any(|found| found.name() == name))
        }) && source.is_none_or(|name| {
            document
                .provenance
                .iter()
                .any(|provenance| provenance.source == name)
        });
        if !matches {
            continue;
        }
        if json {
            let line = serde_json::to_string(document).map_err(|error| error.to_string())?;
            writeln!(out, "{line}").map_err(|error| error.to_string())?;
        } else {
            let first = document.provenance.first();
            let place = match first {
                Some(Provenance {
                    source,
                    path: Some(path),
                }) => format!("{source}:{path}"),
                Some(Provenance { source, path: None }) => source.clone(),
                None => String::new(),
            };
            writeln!(
                out,
                "{} {:>9} {} {} {place}",
                document.sha256, document.size, document.tier, document.license
            )
            .map_err(|error| error.to_string())?;
        }
    }
    Ok(Outcome::Success)
}

fn stats(arguments: &Arguments, out: &mut dyn Write) -> Result<Outcome, String> {
    let manifest = load(&manifest_path(arguments)?, false)?;
    let stats = Stats::of(&manifest);
    let text = match arguments.one("format")? {
        None | Some("markdown") => stats.to_markdown(
            arguments.one("title")?.unwrap_or("Corpus frequency report"),
            &manifest,
        ),
        Some("json") => {
            let mut text = serde_json::to_string_pretty(&stats.to_json())
                .map_err(|error| error.to_string())?;
            text.push('\n');
            text
        }
        Some(other) => return Err(format!("unknown format `{other}`; use markdown or json")),
    };
    out.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    Ok(Outcome::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_never_writes_a_manifest_it_made_inconsistent() {
        let folder =
            std::env::temp_dir().join(format!("bayan-lab-save-checked-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("manifest.json");
        let mut manifest = Manifest::default();
        // A document whose source is not defined, and which is not tagged: problems no command may add.
        let document = Document {
            sha256: Sha256::of(b"one"),
            size: 3,
            tier: Tier::T1,
            license: "MIT".to_owned(),
            provenance: vec![Provenance {
                source: "missing".to_owned(),
                path: None,
            }],
            scan: None,
            page_count: None,
            ground_truth: GroundTruth::default(),
            notes: None,
        };
        manifest.insert(document).unwrap();
        let problems = problems_of(&manifest);
        assert!(!problems.is_empty());
        let error = save_checked(&path, &manifest, &BTreeSet::new()).unwrap_err();
        assert!(error.contains("would make it inconsistent"), "{error}");
        assert!(!path.exists());
        // Problems the manifest had before the command are left for `corpus verify` to report.
        save_checked(&path, &manifest, &problems).unwrap();
        assert!(path.exists());
        fs::remove_dir_all(&folder).unwrap();
    }
}
