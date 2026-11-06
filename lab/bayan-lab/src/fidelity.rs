//! The fidelity commands of LAB-004: `bayan-lab compare`, which compares two layouts of one document, and `bayan-lab report`, which compares every document of a corpus manifest and writes the JSON summary and the HTML report. The comparisons themselves are in the bayan-lab-compare crate; this module reads the files and the store and writes the results.
//!
//! Messages name documents by their SHA-256 and files by their role (the reference, the candidate), never by a path or a name a document could have chosen (AGENTS.md §6).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use bayan_lab_compare::hints::{self, Hints};
use bayan_lab_compare::metrics::{self, Comparison, HintAgreement};
use bayan_lab_compare::report;
use bayan_lab_compare::summary::{
    self, Availability, Document, HintsOutcome, Outcome as DocumentOutcome, Summary, Tier,
};
use bayan_lab_layout::layout::{Layout, LineCoverage, SchemaVersion, Source, TextOffsets};
use serde::Serialize;

use crate::cli::{
    Arguments, Environment, MAX_LAYOUT_FILE, Outcome, load, manifest_path, read_up_to, store_of,
    with_common,
};
use crate::hash::Sha256;
use crate::manifest::{self, Provenance};
use crate::scan::printable;
use crate::store::{Key, MAX_OBJECT_SIZE, Store};

/// The largest summary `report --baseline` reads, in bytes.
const MAX_SUMMARY_FILE: u64 = 1 << 30;

/// The title of a report when `--title` is not given.
const DEFAULT_TITLE: &str = "Fidelity report";

/// The output of `bayan-lab compare`.
#[derive(Serialize)]
struct Single<'a> {
    /// Always `bayan-lab-fidelity-comparison`.
    format: &'static str,
    /// The version of the metrics.
    metrics: u32,
    /// M1 to M4.
    comparison: &'a Comparison,
    /// M7, if `--hints` was given and the document has hints.
    #[serde(skip_serializing_if = "Option::is_none")]
    hints: Option<&'a HintAgreement>,
    /// What the comparison does not measure.
    notes: [&'static str; 2],
}

/// `bayan-lab compare [--hints DOCX] [--output FILE] REFERENCE CANDIDATE`.
pub(crate) fn compare(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Outcome, String> {
    let arguments = Arguments::parse(args, &["hints", "output"], &[])?;
    let [reference, candidate] = arguments.operands.as_slice() else {
        return Err(
            "`compare` takes two layout JSON files: Word's (the reference) and BayanDocs' (the candidate)"
                .to_owned(),
        );
    };
    let output = arguments.one("output")?;
    if output == Some("") {
        return Err("--output cannot be empty".to_owned());
    }
    let reference = match read_layout(Path::new(reference), "the reference")? {
        Ok((layout, _)) => layout,
        Err(message) => {
            let _ = writeln!(err, "bayan-lab: {message}");
            return Ok(Outcome::Failed);
        }
    };
    let candidate = match read_layout(Path::new(candidate), "the candidate")? {
        Ok((layout, _)) => layout,
        Err(message) => {
            let _ = writeln!(err, "bayan-lab: {message}");
            return Ok(Outcome::Failed);
        }
    };
    let limits = metrics::Limits::DEFAULT;
    let measured = match metrics::compare(&reference, &candidate, &limits) {
        Ok(measured) => measured,
        Err(error) => {
            let _ = writeln!(err, "bayan-lab: {error}");
            return Ok(Outcome::Failed);
        }
    };
    let mut hint_agreement = None;
    if let Some(docx) = arguments.one("hints")? {
        let bytes = match read_up_to(Path::new(docx), MAX_OBJECT_SIZE) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return Err(format!(
                    "the .docx file is larger than {MAX_OBJECT_SIZE} bytes"
                ));
            }
            Err(error) => return Err(format!("cannot read the .docx file: {error}")),
        };
        match read_hints(&bytes) {
            Ok(hints) if hints.breaks.is_empty() => {
                let _ = writeln!(
                    err,
                    "bayan-lab: the document has no w:lastRenderedPageBreak hints, so M7 does not apply"
                );
            }
            Ok(hints) => match metrics::hint_agreement(&hints, &candidate, &limits) {
                Ok(agreement) => hint_agreement = Some(agreement),
                Err(error) => {
                    let _ = writeln!(err, "bayan-lab: {error}");
                    return Ok(Outcome::Failed);
                }
            },
            Err(error) => {
                let _ = writeln!(err, "bayan-lab: the .docx file's hints: {error}");
                return Ok(Outcome::Failed);
            }
        }
    }
    let single = Single {
        format: "bayan-lab-fidelity-comparison",
        metrics: summary::METRICS_VERSION,
        comparison: &measured.comparison,
        hints: hint_agreement.as_ref(),
        notes: summary::NOTES,
    };
    let mut text = serde_json::to_string_pretty(&single).map_err(|error| error.to_string())?;
    text.push('\n');
    write_output(output, &text, out)?;
    Ok(Outcome::Success)
}

/// `bayan-lab report --manifest FILE [--references DIR] [--candidates DIR] [--store ADDRESS] [--baseline FILE] [--json FILE] [--html FILE] [--title TEXT]`.
pub(crate) fn report(
    environment: &Environment,
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Outcome, String> {
    let mut arguments = Arguments::parse(
        args,
        &with_common(&[
            "references",
            "candidates",
            "baseline",
            "json",
            "html",
            "title",
        ]),
        &[],
    )?;
    arguments.environment = environment.clone();
    if let Some(operand) = arguments.operands.first() {
        return Err(format!(
            "`report` takes no operand, but was given `{}`; options are written --name VALUE",
            printable(operand, 100)
        ));
    }
    let json = arguments.one("json")?;
    let html = arguments.one("html")?;
    if json.is_none() && html.is_none() {
        return Err("`report` needs --json FILE, --html FILE or both".to_owned());
    }
    if json == Some("") || html == Some("") {
        return Err("--json and --html cannot be empty".to_owned());
    }
    let title = arguments.one("title")?.unwrap_or(DEFAULT_TITLE);
    if title.trim().is_empty() || title.chars().any(char::is_control) {
        return Err("--title must be printable text".to_owned());
    }
    let folder = |name: &str| -> Result<Option<PathBuf>, String> {
        match arguments.one(name)? {
            Some("") => Err(format!("--{name} cannot be empty")),
            Some(path) if !Path::new(path).is_dir() => Err(format!("--{name} must be a folder")),
            Some(path) => Ok(Some(PathBuf::from(path))),
            None => Ok(None),
        }
    };
    let references = folder("references")?;
    let candidates = folder("candidates")?;
    let manifest = load(&manifest_path(&arguments)?, false)?;
    manifest
        .check()
        .map_err(|problems| format!("the manifest is not consistent:\n{problems}"))?;
    let store = store_of(&arguments)?;
    let baseline = match arguments.one("baseline")? {
        Some("") => return Err("--baseline cannot be empty".to_owned()),
        Some(path) => {
            let bytes = read_up_to(Path::new(path), MAX_SUMMARY_FILE)
                .map_err(|error| format!("cannot read the baseline: {error}"))?
                .ok_or_else(|| format!("the baseline is larger than {MAX_SUMMARY_FILE} bytes"))?;
            let text = String::from_utf8(bytes)
                .map_err(|_| "the baseline is not UTF-8 text".to_owned())?;
            Some(Summary::from_json(&text).map_err(|error| format!("the baseline: {error}"))?)
        }
        None => None,
    };
    let limits = metrics::Limits::DEFAULT;
    let mut outcomes = Vec::with_capacity(manifest.documents.len());
    for document in &manifest.documents {
        outcomes.push(run_document(
            document,
            references.as_deref(),
            candidates.as_deref(),
            store.as_deref(),
            &limits,
        )?);
    }
    let summary = Summary::new(title, &outcomes, baseline.as_ref())?;
    if let Some(path) = json {
        fs::write(path, summary.to_json()?)
            .map_err(|error| format!("cannot write the JSON summary: {error}"))?;
    }
    if let Some(path) = html {
        fs::write(path, report::html(&summary))
            .map_err(|error| format!("cannot write the HTML report: {error}"))?;
    }
    let all = &summary.overall;
    let _ = writeln!(
        out,
        "report: {} documents; {} compared with Word's layout ({} without it, {} without BayanDocs' layout), {} with hints; {} failed",
        all.documents,
        all.compared,
        all.without_reference,
        all.without_candidate,
        all.m7.documents,
        all.failed
    );
    if all.m7.refused > 0 {
        let _ = writeln!(
            out,
            "hints: {} documents could not be read for M7, because bayan-opc or bayan-xml refuses them (the summary says why)",
            all.m7.refused
        );
    }
    let mut outcome = Outcome::Success;
    for entry in &summary.documents {
        if let Some(problem) = &entry.problem {
            let _ = writeln!(
                err,
                "bayan-lab: document {}: {problem}",
                entry.document.sha256
            );
            outcome = Outcome::Failed;
        }
    }
    if let Some(baseline) = &summary.baseline {
        let _ = writeln!(
            out,
            "baseline: {} documents compared, {} lost agreement, {} gained, {} re-baselined, {} gone, {} new",
            baseline.documents,
            baseline.regressions.len(),
            baseline.improvements.len(),
            baseline.rebaselined.len(),
            baseline.missing.len(),
            baseline.added
        );
        for change in &baseline.regressions {
            let _ = writeln!(
                err,
                "bayan-lab: document {} lost agreement: {} page breaks, {} line breaks, {} hints{}",
                change.document.sha256,
                change.page_breaks.lost,
                change.line_breaks.lost,
                change.hints.lost,
                if change.page_count_lost {
                    ", and its page count no longer matches"
                } else {
                    ""
                }
            );
            outcome = Outcome::Failed;
        }
    }
    Ok(outcome)
}

/// Compares one document of a run.
fn run_document(
    document: &manifest::Document,
    references: Option<&Path>,
    candidates: Option<&Path>,
    store: Option<&dyn Store>,
    limits: &metrics::Limits,
) -> Result<DocumentOutcome, String> {
    let file = format!("{}.json", document.sha256);
    let mut problems = Vec::new();
    let (reference, reference_digest) = match references.map(|folder| folder.join(&file)) {
        Some(path) if path.is_file() => match read_layout(&path, "the reference")? {
            Ok((layout, digest)) => (Ok(layout), Some(digest)),
            Err(message) => {
                problems.push(message);
                (Err(Availability::Invalid), None)
            }
        },
        _ => (Err(Availability::Missing), None),
    };
    let candidate = match candidates.map(|folder| folder.join(&file)) {
        Some(path) if path.is_file() => match read_layout(&path, "the candidate")? {
            Ok((layout, _)) => Ok(layout),
            Err(message) => {
                problems.push(message);
                Err(Availability::Invalid)
            }
        },
        _ => Err(Availability::Missing),
    };
    let empty = empty_layout();
    let candidate_layout = match &candidate {
        Ok(layout) => Some(layout),
        Err(Availability::Missing) => Some(&empty),
        Err(_) => None,
    };
    let mut measured = None;
    if let (Ok(reference), Some(candidate)) = (&reference, candidate_layout) {
        match metrics::compare(reference, candidate, limits) {
            Ok(result) => measured = Some(result),
            Err(error) => problems.push(error.to_string()),
        }
    }
    let hints = match (store, candidate_layout) {
        (None, _) => HintsOutcome::NotRead,
        (Some(_), None) => HintsOutcome::Refused("the candidate is not valid".to_owned()),
        (Some(store), Some(candidate)) => hints_of(store, document.sha256, candidate, limits)?,
    };
    let scan = document.scan.as_ref();
    let tier = match document.tier {
        manifest::Tier::T0 => Tier::T0,
        manifest::Tier::T1 => Tier::T1,
        manifest::Tier::T2 => Tier::T2,
        manifest::Tier::T3 => Tier::T3,
    };
    Ok(DocumentOutcome {
        document: Document {
            sha256: document.sha256.to_string(),
            tier,
            name: if tier.is_private() {
                None
            } else {
                public_name(document.provenance.first())
            },
            features: scan.map_or_else(Vec::new, |scan| scan.features.clone()),
            scripts: scan.map_or_else(Vec::new, |scan| {
                scan.scripts
                    .keys()
                    .map(|script| script.name().to_owned())
                    .collect()
            }),
            compatibility_mode: scan.and_then(|scan| scan.compatibility.mode),
        },
        reference: match &reference {
            Ok(_) => Availability::Present,
            Err(availability) => *availability,
        },
        reference_digest,
        candidate: match &candidate {
            Ok(_) => Availability::Present,
            Err(availability) => *availability,
        },
        measured,
        hints,
        problem: if problems.is_empty() {
            None
        } else {
            Some(problems.join("; "))
        },
    })
}

/// M7 for a document of the store.
fn hints_of(
    store: &dyn Store,
    sha256: Sha256,
    candidate: &Layout,
    limits: &metrics::Limits,
) -> Result<HintsOutcome, String> {
    let bytes = store
        .get(&Key::document(sha256))
        .map_err(|error| format!("cannot read document {sha256} from the store: {error}"))?
        .ok_or_else(|| format!("the store does not have document {sha256}"))?;
    if Sha256::of(&bytes) != sha256 {
        return Err(format!(
            "the store's copy of document {sha256} does not have that SHA-256"
        ));
    }
    Ok(match read_hints(&bytes) {
        Ok(hints) if hints.breaks.is_empty() => HintsOutcome::Absent,
        Ok(hints) => match metrics::hint_agreement(&hints, candidate, limits) {
            Ok(agreement) => HintsOutcome::Agreement(agreement),
            Err(error) => HintsOutcome::Refused(error.to_string()),
        },
        Err(error) => HintsOutcome::Refused(format!("its hints: {error}")),
    })
}

fn read_hints(bytes: &[u8]) -> Result<Hints, hints::Error> {
    hints::read(bytes, &hints::Limits::DEFAULT)
}

/// The public name of a document: its first source and path, as `bayan-lab corpus list` prints it.
fn public_name(provenance: Option<&Provenance>) -> Option<String> {
    let provenance = provenance?;
    Some(match &provenance.path {
        Some(path) => format!(
            "{}:{}",
            printable(&provenance.source, 100),
            printable(path, 300)
        ),
        None => printable(&provenance.source, 100),
    })
}

/// A layout JSON file and the SHA-256 of its bytes; the inner error when it is not a valid layout, the outer one when it cannot be read. `role` names the file in messages.
fn read_layout(path: &Path, role: &str) -> Result<Result<(Layout, String), String>, String> {
    let bytes = match read_up_to(path, MAX_LAYOUT_FILE) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            return Ok(Err(format!(
                "{role} is larger than {MAX_LAYOUT_FILE} bytes"
            )));
        }
        Err(error) => return Err(format!("cannot read {role}: {error}")),
    };
    let digest = Sha256::of(&bytes).to_string();
    let Ok(text) = String::from_utf8(bytes) else {
        return Ok(Err(format!("{role} is not UTF-8 text")));
    };
    Ok(Layout::from_json(&text)
        .map(|layout| (layout, digest))
        .map_err(|message| format!("{role} is not a valid layout JSON: {message}")))
}

/// What a document without BayanDocs' layout is compared with: no pages.
fn empty_layout() -> Layout {
    Layout {
        schema: SchemaVersion::V1,
        source: Source::Engine,
        producer: "bayan-lab report: no layout".to_owned(),
        text_offsets: TextOffsets::Document,
        line_coverage: LineCoverage::All,
        page_count: 0,
        fonts: Vec::new(),
        pages: Vec::new(),
    }
}

/// Writes `text` to the file `output`, or to `out`.
fn write_output(output: Option<&str>, text: &str, out: &mut dyn Write) -> Result<(), String> {
    match output {
        Some(path) => {
            fs::write(path, text).map_err(|error| format!("cannot write the output: {error}"))
        }
        None => out
            .write_all(text.as_bytes())
            .map_err(|error| error.to_string()),
    }
}
