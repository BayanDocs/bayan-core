//! A corpus run: every document's metrics, their aggregation, and what changed against a baseline, as the JSON summary that [`crate::report`] renders and a later run reads back as its baseline (Fidelity Lab specification §6 step 3, §9).
//!
//! - **Aggregation** ([`Aggregate`]): over all documents, and per tier, per compatibility mode, per script and per feature tag of the manifest. A document counts in every group it belongs to. Agreement is summed over breaks (the share of all the group's breaks that agree), and glyph errors are the percentiles of all the group's glyphs, from merged [`Histogram`]s.
//! - **Worst documents** ([`Worst`]): the public documents with the lowest line-break agreement (M3), and those with the lowest hint agreement (M7).
//! - **Correlations** ([`Correlation`]): for each feature tag, how strongly having it goes with disagreement, as the point-biserial correlation between the tag and each document's share of breaks that disagree, computed with integers only. The tags most correlated with disagreement are the ones to work on first.
//! - **Regressions** ([`BaselineComparison`]): a document regresses when a break that agreed in the baseline no longer does (M2, M3, M7), or its page counts matched and no longer do (M1). Breaks are compared one by one, by their index in the reference, so a document that loses one break and gains another still regresses. M1 to M3 are compared only when the reference is the same file as in the baseline (its SHA-256); otherwise the document was re-baselined and is listed as such.
//!
//! Private (T2) documents are counted in the aggregates and in the numbers of regressions, but never named: their entries carry only their SHA-256, as their manifest does, and the HTML report never lists them one by one. The summary of a private corpus is therefore as private as its manifest.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::histogram::Histogram;
use crate::metrics::{Breaks, Comparison, HintAgreement, Measured, Spread};

/// The value of [`Summary::format`].
pub const FORMAT: &str = "bayan-lab-fidelity-summary";

/// The version of the summary's layout, [`Summary::version`].
pub const VERSION: u32 = 1;

/// The version of the metrics' definitions and of the hint reader, [`Summary::metrics`]. It changes whenever the same layouts or documents would give other numbers, so that a baseline made by older rules is refused rather than compared.
pub const METRICS_VERSION: u32 = 1;

/// How many documents each list of worst documents names.
pub const WORST_LISTED: usize = 20;

/// How many feature tags each list of correlations names.
pub const CORRELATIONS_LISTED: usize = 20;

/// The fewest documents with, and the fewest without, a feature tag for its correlation to be computed.
pub const MIN_CORRELATION_GROUP: u64 = 5;

/// What the notes of every summary say about the metrics it does not compute.
pub const NOTES: [&str; 2] = [
    crate::visual::NOT_MEASURED,
    "M6 (round-trip integrity) is a process of the reference machine, not of this run: Word opens BayanDocs' re-saved copy of each document, which must open without a repair prompt and show the same content (lab/compare.md)",
];

/// A corpus tier, as the manifest names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    /// Probes.
    T0,
    /// The public corpus.
    T1,
    /// The private corpus: never named.
    T2,
    /// Torture documents (public).
    T3,
}

impl Tier {
    /// The tier's name, such as `T1`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::T0 => "T0",
            Self::T1 => "T1",
            Self::T2 => "T2",
            Self::T3 => "T3",
        }
    }

    /// Whether the tier's documents are private.
    #[must_use]
    pub const fn is_private(self) -> bool {
        matches!(self, Self::T2)
    }
}

/// What a run knows about a document from the corpus manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    /// Its SHA-256, in lowercase hexadecimal.
    pub sha256: String,
    /// Its tier.
    pub tier: Tier,
    /// Its public name, such as `python-docx:features/par-alignment.docx`; ignored for a private document.
    pub name: Option<String>,
    /// Its feature tags.
    pub features: Vec<String>,
    /// The scripts of its text.
    pub scripts: Vec<String>,
    /// Its compatibility mode, if it sets one.
    pub compatibility_mode: Option<u32>,
}

/// Whether one side of a comparison was there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// A valid layout.
    Present,
    /// No layout: no ground truth for the reference; for the candidate, compared as empty output (no pages).
    Missing,
    /// A layout that is not valid.
    Invalid,
}

/// What M7 found in a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HintsOutcome {
    /// The run did not read the document (no store).
    NotRead,
    /// The document has no `w:lastRenderedPageBreak` hints.
    Absent,
    /// The document's hints could not be read: why, never quoting the document.
    Refused(String),
    /// The agreement with its hints.
    Agreement(HintAgreement),
}

/// One document's results in a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The document.
    pub document: Document,
    /// Whether Word's layout was there.
    pub reference: Availability,
    /// The SHA-256 of Word's layout file, if there.
    pub reference_digest: Option<String>,
    /// Whether BayanDocs' layout was there.
    pub candidate: Availability,
    /// The metrics M1 to M4, when the reference is present and the candidate present or missing.
    pub measured: Option<Measured>,
    /// M7.
    pub hints: HintsOutcome,
    /// What went wrong, if the document could not be compared, never quoting it.
    pub problem: Option<String>,
}

/// Breaks summed over a group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Totals {
    /// The reference's breaks.
    pub total: u64,
    /// How many agree.
    pub agreed: u64,
}

impl Totals {
    fn add(&mut self, breaks: &Breaks) {
        self.total += breaks.total;
        self.agreed += breaks.agreed;
    }
}

/// M1 summed over a group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageTotals {
    /// Documents compared.
    pub documents: u64,
    /// Documents whose page counts match.
    pub matching: u64,
    /// The sum of the documents' page count differences, without their signs.
    pub absolute_difference: u64,
}

/// M4 over a group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlyphTotals {
    /// Lines measured.
    pub lines: u64,
    /// Glyphs measured.
    pub glyphs: u64,
    /// Horizontal distances of all the group's glyphs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Spread>,
    /// Vertical distances of all the group's glyphs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Spread>,
}

/// M7 over a group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HintTotals {
    /// Documents with hints.
    pub documents: u64,
    /// Their hints, and how many agree.
    pub total: u64,
    /// How many hints agree.
    pub agreed: u64,
    /// Documents whose candidate has as many pages as the hints imply.
    pub pages_matching: u64,
    /// Documents whose hints could not be read, because bayan-opc or bayan-xml refuses them or they exceed the limits: documents BayanDocs cannot open.
    pub refused: u64,
}

/// The text matched by the alignments of a group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextTotals {
    /// Characters of the references' streams.
    pub reference: u64,
    /// Of those, the characters the alignments matched.
    pub matched: u64,
    /// Alignments that the work limit cut short.
    pub incomplete: u64,
}

/// The metrics of a group of documents.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Aggregate {
    /// The group's documents.
    pub documents: u64,
    /// Documents compared with Word's layout (M1 to M4).
    pub compared: u64,
    /// Documents without Word's layout (no ground truth yet).
    pub without_reference: u64,
    /// Documents without BayanDocs' layout, which are compared as empty output: with Word's layout where it exists (M1 to M4), and with the hints (M7).
    pub without_candidate: u64,
    /// Documents that could not be compared: a layout that is not valid, or a comparison beyond the limits.
    pub failed: u64,
    /// The text the alignments matched.
    pub text: TextTotals,
    /// M1.
    pub m1: PageTotals,
    /// M2.
    pub m2: Totals,
    /// M3.
    pub m3: Totals,
    /// M4.
    pub m4: GlyphTotals,
    /// M7.
    pub m7: HintTotals,
}

/// An aggregate on its way, with the histograms of its glyph errors.
#[derive(Default)]
struct Accumulator {
    aggregate: Aggregate,
    x: Histogram,
    y: Histogram,
}

impl Accumulator {
    fn add(&mut self, outcome: &Outcome) {
        let aggregate = &mut self.aggregate;
        aggregate.documents += 1;
        if outcome.reference == Availability::Missing {
            aggregate.without_reference += 1;
        }
        if outcome.candidate == Availability::Missing {
            aggregate.without_candidate += 1;
        }
        if outcome.problem.is_some() {
            aggregate.failed += 1;
        }
        if let Some(measured) = &outcome.measured {
            let comparison = &measured.comparison;
            aggregate.compared += 1;
            aggregate.text.reference += comparison.text.reference;
            aggregate.text.matched += comparison.text.matched;
            if !comparison.text.complete {
                aggregate.text.incomplete += 1;
            }
            aggregate.m1.documents += 1;
            if comparison.m1.matches() {
                aggregate.m1.matching += 1;
            }
            aggregate.m1.absolute_difference += comparison.m1.difference().unsigned_abs();
            aggregate.m2.add(&comparison.m2);
            aggregate.m3.add(&comparison.m3);
            aggregate.m4.lines += comparison.m4.lines;
            aggregate.m4.glyphs += comparison.m4.glyphs;
            self.x.merge(&measured.x);
            self.y.merge(&measured.y);
        }
        match &outcome.hints {
            HintsOutcome::Agreement(hints) => {
                aggregate.m7.documents += 1;
                aggregate.m7.total += hints.hints.total;
                aggregate.m7.agreed += hints.hints.agreed;
                if hints.pages.matches() {
                    aggregate.m7.pages_matching += 1;
                }
            }
            HintsOutcome::Refused(_) => aggregate.m7.refused += 1,
            HintsOutcome::NotRead | HintsOutcome::Absent => {}
        }
    }

    fn finish(mut self) -> Aggregate {
        self.aggregate.m4.x = Spread::of(&self.x);
        self.aggregate.m4.y = Spread::of(&self.y);
        self.aggregate
    }
}

/// A document in a list: its SHA-256, and its name if it is public.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Named {
    /// Its SHA-256.
    pub sha256: String,
    /// Its public name; absent for a private document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its tier.
    pub tier: Tier,
}

impl Named {
    fn of(document: &Document) -> Self {
        Self {
            sha256: document.sha256.clone(),
            name: if document.tier.is_private() {
                None
            } else {
                document.name.clone()
            },
            tier: document.tier,
        }
    }
}

/// One document's entry in the summary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The document.
    #[serde(flatten)]
    pub document: Named,
    /// Whether Word's layout was there.
    pub reference: Availability,
    /// The SHA-256 of Word's layout file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_sha256: Option<String>,
    /// Whether BayanDocs' layout was there.
    pub candidate: Availability,
    /// M1 to M4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparison: Option<Comparison>,
    /// M7, if the document has hints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints: Option<HintAgreement>,
    /// Why its hints could not be read, if they could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints_problem: Option<String>,
    /// Why it could not be compared, if it could not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// A document of a list of worst documents, with the numbers it was ranked by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ranked {
    /// The document.
    #[serde(flatten)]
    pub document: Named,
    /// Its breaks and how many agree.
    pub breaks: Totals,
    /// Its page count difference (M1 for line breaks, the hints' for M7).
    pub page_difference: i64,
}

/// The worst public documents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Worst {
    /// By line-break agreement (M3), lowest first; then page-break agreement, the larger page count difference, the SHA-256.
    pub line_breaks: Vec<Ranked>,
    /// By hint agreement (M7), lowest first; then more hints first, the SHA-256.
    pub hints: Vec<Ranked>,
}

/// How strongly a feature tag goes with disagreement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correlation {
    /// The feature tag.
    pub feature: String,
    /// Documents with the tag.
    pub with: u64,
    /// Documents without it.
    pub without: u64,
    /// The average share of breaks that disagree in documents with the tag, in thousandths.
    pub disagreement_with: u64,
    /// The same without the tag.
    pub disagreement_without: u64,
    /// The point-biserial correlation of the tag with the share of breaks that disagree, in thousandths, from -1000 to 1000: positive when documents with the tag disagree more.
    pub correlation: i64,
}

/// The feature tags most correlated with disagreement.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Correlations {
    /// With line-break disagreement (M3), over the documents compared.
    pub line_breaks: Vec<Correlation>,
    /// With hint disagreement (M7), over the documents with hints.
    pub hints: Vec<Correlation>,
}

/// What one document lost or gained against the baseline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    /// The document.
    #[serde(flatten)]
    pub document: Named,
    /// Whether its page counts matched in the baseline and no longer do.
    pub page_count_lost: bool,
    /// Whether its page counts match now and did not in the baseline.
    pub page_count_gained: bool,
    /// Page breaks (M2) that agreed in the baseline and no longer do, and the reverse.
    pub page_breaks: Delta,
    /// Line breaks (M3), the same.
    pub line_breaks: Delta,
    /// Hints (M7), the same.
    pub hints: Delta,
}

impl Change {
    /// Whether the document lost anything.
    #[must_use]
    pub const fn regressed(&self) -> bool {
        self.page_count_lost
            || self.page_breaks.lost > 0
            || self.line_breaks.lost > 0
            || self.hints.lost > 0
    }

    const fn improved(&self) -> bool {
        self.page_count_gained
            || self.page_breaks.gained > 0
            || self.line_breaks.gained > 0
            || self.hints.gained > 0
    }
}

/// Breaks lost and gained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    /// Breaks that agreed in the baseline and no longer do.
    pub lost: u64,
    /// Breaks that agree now and did not in the baseline.
    pub gained: u64,
}

/// What changed against a baseline.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineComparison {
    /// The baseline's title.
    pub title: String,
    /// Documents of this run that the baseline has.
    pub documents: u64,
    /// Documents that lost agreement: public ones by name, private ones by SHA-256 only.
    pub regressions: Vec<Change>,
    /// Documents that gained agreement and lost none.
    pub improvements: Vec<Change>,
    /// Documents whose Word layout is another file than in the baseline, so that M1 to M3 were not compared.
    pub rebaselined: Vec<Named>,
    /// Documents of the baseline that this run does not have.
    pub missing: Vec<Named>,
    /// Documents of this run that the baseline does not have.
    pub added: u64,
}

/// A corpus run (see the module's documentation).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    /// Always `bayan-lab-fidelity-summary`.
    pub format: String,
    /// The summary's layout: 1.
    pub version: u32,
    /// The version of the metrics' definitions ([`METRICS_VERSION`]).
    pub metrics: u32,
    /// The title the run was given.
    pub title: String,
    /// What the run does not measure, and why.
    pub notes: Vec<String>,
    /// All documents.
    pub overall: Aggregate,
    /// Per tier.
    pub tiers: BTreeMap<String, Aggregate>,
    /// Per compatibility mode: the mode's number, or `not set` (Word then uses mode 12).
    pub compatibility_modes: BTreeMap<String, Aggregate>,
    /// Per script of the documents' text.
    pub scripts: BTreeMap<String, Aggregate>,
    /// Per feature tag.
    pub features: BTreeMap<String, Aggregate>,
    /// The worst public documents.
    pub worst: Worst,
    /// The feature tags most correlated with disagreement.
    pub correlations: Correlations,
    /// What changed against the baseline, if the run was given one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineComparison>,
    /// Every document, by SHA-256.
    pub documents: Vec<Entry>,
}

/// The name of a compatibility mode group.
#[must_use]
pub fn mode_name(mode: Option<u32>) -> String {
    mode.map_or_else(|| "not set".to_owned(), |mode| mode.to_string())
}

impl Summary {
    /// The summary of `outcomes` (in any order; the summary sorts them by SHA-256), titled `title`, compared with `baseline` if given.
    ///
    /// # Errors
    ///
    /// A message if two outcomes are for the same document.
    pub fn new(title: &str, outcomes: &[Outcome], baseline: Option<&Self>) -> Result<Self, String> {
        let mut sorted: Vec<&Outcome> = outcomes.iter().collect();
        sorted.sort_by(|a, b| a.document.sha256.cmp(&b.document.sha256));
        if let Some(pair) = sorted
            .windows(2)
            .find(|pair| pair[0].document.sha256 == pair[1].document.sha256)
        {
            return Err(format!(
                "the document {} is listed twice",
                pair[0].document.sha256
            ));
        }
        let mut overall = Accumulator::default();
        let mut tiers: BTreeMap<String, Accumulator> = BTreeMap::new();
        let mut modes: BTreeMap<String, Accumulator> = BTreeMap::new();
        let mut scripts: BTreeMap<String, Accumulator> = BTreeMap::new();
        let mut features: BTreeMap<String, Accumulator> = BTreeMap::new();
        for outcome in &sorted {
            let document = &outcome.document;
            overall.add(outcome);
            tiers
                .entry(document.tier.name().to_owned())
                .or_default()
                .add(outcome);
            modes
                .entry(mode_name(document.compatibility_mode))
                .or_default()
                .add(outcome);
            for script in document.scripts.iter().collect::<BTreeSet<_>>() {
                scripts.entry(script.clone()).or_default().add(outcome);
            }
            for feature in document.features.iter().collect::<BTreeSet<_>>() {
                features.entry(feature.clone()).or_default().add(outcome);
            }
        }
        let finish = |groups: BTreeMap<String, Accumulator>| {
            groups
                .into_iter()
                .map(|(name, accumulator)| (name, accumulator.finish()))
                .collect()
        };
        let documents: Vec<Entry> = sorted.iter().map(|outcome| entry(outcome)).collect();
        let mut summary = Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            metrics: METRICS_VERSION,
            title: title.to_owned(),
            notes: NOTES.iter().map(|note| (*note).to_owned()).collect(),
            overall: overall.finish(),
            tiers: finish(tiers),
            compatibility_modes: finish(modes),
            scripts: finish(scripts),
            features: finish(features),
            worst: worst(&sorted),
            correlations: Correlations {
                line_breaks: correlations(&sorted, |outcome| {
                    outcome
                        .measured
                        .as_ref()
                        .map(|measured| measured.comparison.m3.clone())
                }),
                hints: correlations(&sorted, |outcome| match &outcome.hints {
                    HintsOutcome::Agreement(hints) => Some(hints.hints.clone()),
                    _ => None,
                }),
            },
            baseline: None,
            documents,
        };
        if let Some(baseline) = baseline {
            summary.baseline = Some(compare_with(&summary.documents, baseline));
        }
        Ok(summary)
    }

    /// Reads a summary, such as a baseline, from its JSON.
    ///
    /// # Errors
    ///
    /// A message if the text is not a summary of this layout, its metrics are of another version, or a document's agreed breaks are not as this tool writes them (see [`Breaks::agreed_breaks`]): a damaged or edited baseline is refused rather than compared.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let summary: Self = serde_json::from_str(text).map_err(|error| {
            format!(
                "not a fidelity summary (line {}, column {})",
                error.line(),
                error.column()
            )
        })?;
        if summary.format != FORMAT || summary.version != VERSION {
            return Err(format!(
                "not a fidelity summary of version {VERSION} (format `{FORMAT}`)"
            ));
        }
        if summary.metrics != METRICS_VERSION {
            return Err(format!(
                "the summary was made with metrics of version {}, and this tool's are version {METRICS_VERSION}: run the baseline again with this tool",
                summary.metrics
            ));
        }
        for entry in &summary.documents {
            let all = entry
                .comparison
                .iter()
                .flat_map(|comparison| {
                    [
                        ("page breaks (M2)", &comparison.m2),
                        ("line breaks (M3)", &comparison.m3),
                    ]
                })
                .chain(entry.hints.iter().map(|hints| ("hints (M7)", &hints.hints)));
            for (what, breaks) in all {
                if !well_formed(breaks) {
                    return Err(format!(
                        "the agreed {what} of document {} are not as this tool writes them: ranges that are not empty, ascend without touching, lie within the {} breaks and hold the {} that agree",
                        entry.document.sha256, breaks.total, breaks.agreed
                    ));
                }
            }
        }
        Ok(summary)
    }

    /// The summary as JSON: two-space indentation, a final line feed, and every list and map in a fixed order, so that the same run gives the same bytes.
    ///
    /// # Errors
    ///
    /// Never in practice: every value of a summary can be written as JSON.
    pub fn to_json(&self) -> Result<String, String> {
        let mut text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        text.push('\n');
        Ok(text)
    }

    /// Whether any document regressed against the baseline.
    #[must_use]
    pub fn regressed(&self) -> bool {
        self.baseline
            .as_ref()
            .is_some_and(|baseline| !baseline.regressions.is_empty())
    }
}

fn entry(outcome: &Outcome) -> Entry {
    let (hints, hints_problem) = match &outcome.hints {
        HintsOutcome::Agreement(hints) => (Some(hints.clone()), None),
        HintsOutcome::Refused(problem) => (None, Some(problem.clone())),
        HintsOutcome::NotRead | HintsOutcome::Absent => (None, None),
    };
    Entry {
        document: Named::of(&outcome.document),
        reference: outcome.reference,
        reference_sha256: outcome.reference_digest.clone(),
        candidate: outcome.candidate,
        comparison: outcome
            .measured
            .as_ref()
            .map(|measured| measured.comparison.clone()),
        hints,
        hints_problem,
        problem: outcome.problem.clone(),
    }
}

/// Whether `a / b < c / d` for shares of breaks, a document without breaks counting as agreeing fully.
fn share_less(agreed: u64, total: u64, other_agreed: u64, other_total: u64) -> core::cmp::Ordering {
    let (a, b) = if total == 0 { (1, 1) } else { (agreed, total) };
    let (c, d) = if other_total == 0 {
        (1, 1)
    } else {
        (other_agreed, other_total)
    };
    (u128::from(a) * u128::from(d)).cmp(&(u128::from(c) * u128::from(b)))
}

fn worst(outcomes: &[&Outcome]) -> Worst {
    let public = |outcome: &&&Outcome| !outcome.document.tier.is_private();
    let mut lines: Vec<(&Outcome, &Comparison)> = outcomes
        .iter()
        .filter(public)
        .filter_map(|outcome| {
            outcome
                .measured
                .as_ref()
                .map(|measured| (*outcome, &measured.comparison))
        })
        .filter(|(_, comparison)| comparison.m3.total > 0)
        .collect();
    lines.sort_by(|(a, x), (b, y)| {
        share_less(x.m3.agreed, x.m3.total, y.m3.agreed, y.m3.total)
            .then_with(|| share_less(x.m2.agreed, x.m2.total, y.m2.agreed, y.m2.total))
            .then_with(|| {
                y.m1.difference()
                    .unsigned_abs()
                    .cmp(&x.m1.difference().unsigned_abs())
            })
            .then_with(|| a.document.sha256.cmp(&b.document.sha256))
    });
    let mut hints: Vec<(&Outcome, &HintAgreement)> = outcomes
        .iter()
        .filter(public)
        .filter_map(|outcome| match &outcome.hints {
            HintsOutcome::Agreement(hints) => Some((*outcome, hints)),
            _ => None,
        })
        .collect();
    hints.sort_by(|(a, x), (b, y)| {
        share_less(x.hints.agreed, x.hints.total, y.hints.agreed, y.hints.total)
            .then_with(|| y.hints.total.cmp(&x.hints.total))
            .then_with(|| a.document.sha256.cmp(&b.document.sha256))
    });
    Worst {
        line_breaks: lines
            .into_iter()
            .take(WORST_LISTED)
            .map(|(outcome, comparison)| Ranked {
                document: Named::of(&outcome.document),
                breaks: Totals {
                    total: comparison.m3.total,
                    agreed: comparison.m3.agreed,
                },
                page_difference: comparison.m1.difference(),
            })
            .collect(),
        hints: hints
            .into_iter()
            .take(WORST_LISTED)
            .map(|(outcome, hints)| Ranked {
                document: Named::of(&outcome.document),
                breaks: Totals {
                    total: hints.hints.total,
                    agreed: hints.hints.agreed,
                },
                page_difference: hints.pages.difference(),
            })
            .collect(),
    }
}

/// A document's share of breaks that disagree, in thousandths, rounded.
fn disagreement(breaks: &Breaks) -> u64 {
    let disagreeing = u128::from(breaks.total - breaks.agreed) * 1000;
    let total = u128::from(breaks.total);
    u64::try_from((disagreeing + total / 2) / total).unwrap_or(1000)
}

/// The correlations of every feature tag with the disagreement of the breaks that `breaks` picks from each outcome, over the outcomes that have such breaks; the [`CORRELATIONS_LISTED`] strongest positive ones.
fn correlations(
    outcomes: &[&Outcome],
    breaks: impl Fn(&Outcome) -> Option<Breaks>,
) -> Vec<Correlation> {
    // Each document's disagreement, with its features.
    let documents: Vec<(u64, &Document)> = outcomes
        .iter()
        .filter_map(|outcome| {
            let breaks = breaks(outcome)?;
            (breaks.total > 0).then(|| (disagreement(&breaks), &outcome.document))
        })
        .collect();
    let n = u128::try_from(documents.len()).unwrap_or(u128::MAX);
    let sum: u128 = documents.iter().map(|(d, _)| u128::from(*d)).sum();
    let squares: u128 = documents
        .iter()
        .map(|(d, _)| u128::from(*d) * u128::from(*d))
        .sum();
    // n × the variance × n: zero when every document disagrees equally, and then nothing correlates.
    let spread = n * squares - sum * sum;
    let mut per_feature: BTreeMap<&str, (u128, u128)> = BTreeMap::new();
    for (d, document) in &documents {
        for feature in document.features.iter().collect::<BTreeSet<_>>() {
            let entry = per_feature.entry(feature.as_str()).or_default();
            entry.0 += 1;
            entry.1 += u128::from(*d);
        }
    }
    let minimum = u128::from(MIN_CORRELATION_GROUP);
    let mut found: Vec<Correlation> = Vec::new();
    for (feature, (with, with_sum)) in per_feature {
        let without = n - with;
        if spread == 0 || with < minimum || without < minimum {
            continue;
        }
        // r = (n Σd_f − n_f Σd) / sqrt(n_f (n − n_f) (n Σd² − (Σd)²)), in thousandths: sign × sqrt(numerator² × 10⁶ / denominator²).
        let numerator = i128::try_from(n * with_sum).unwrap_or(i128::MAX)
            - i128::try_from(with * sum).unwrap_or(i128::MAX);
        let denominator = with * without * spread;
        let squared = numerator.unsigned_abs().pow(2).saturating_mul(1_000_000) / denominator;
        let root = squared.isqrt();
        let rounded = if squared > root * root + root {
            root + 1
        } else {
            root
        };
        let magnitude = i64::try_from(rounded.min(1000)).unwrap_or(1000);
        let correlation = if numerator < 0 { -magnitude } else { magnitude };
        let average =
            |total: u128, count: u128| u64::try_from((total + count / 2) / count).unwrap_or(1000);
        found.push(Correlation {
            feature: feature.to_owned(),
            with: u64::try_from(with).unwrap_or(u64::MAX),
            without: u64::try_from(without).unwrap_or(u64::MAX),
            disagreement_with: average(with_sum, with),
            disagreement_without: average(sum - with_sum, without),
            correlation,
        });
    }
    found.retain(|correlation| correlation.correlation > 0);
    found.sort_by(|a, b| {
        b.correlation
            .cmp(&a.correlation)
            .then_with(|| a.feature.cmp(&b.feature))
    });
    found.truncate(CORRELATIONS_LISTED);
    found
}

/// Whether the agreed breaks of `breaks` are as this tool writes them: ranges that are not empty, ascend and do not touch (two that touch would be one), lie within the total, and hold exactly as many breaks as agree.
fn well_formed(breaks: &Breaks) -> bool {
    let mut previous_end = None;
    let mut count: u64 = 0;
    for &[first, end] in &breaks.agreed_breaks {
        if first >= end || end > breaks.total || previous_end.is_some_and(|last| first <= last) {
            return false;
        }
        // At most the total: the ranges are apart and within it.
        count += end - first;
        previous_end = Some(end);
    }
    count == breaks.agreed
}

/// The breaks lost and gained between the baseline's `before` and `now`.
fn delta(before: Option<&Breaks>, now: Option<&Breaks>) -> Delta {
    fn ranges(breaks: Option<&Breaks>) -> &[[u64; 2]] {
        breaks.map_or(&[], |breaks| &breaks.agreed_breaks)
    }
    Delta {
        lost: outside(ranges(before), ranges(now)),
        gained: outside(ranges(now), ranges(before)),
    }
}

/// How many indices of the ranges `a` are in none of the ranges `b` (both sorted and disjoint).
fn outside(a: &[[u64; 2]], b: &[[u64; 2]]) -> u64 {
    let mut count = 0;
    let mut j = 0;
    for &[start, end] in a {
        let mut position = start;
        while position < end {
            while b.get(j).is_some_and(|range| range[1] <= position) {
                j += 1;
            }
            match b.get(j) {
                Some(&[b_start, b_end]) if b_start <= position => position = b_end.min(end),
                Some(&[b_start, _]) => {
                    let next = b_start.min(end);
                    count += next - position;
                    position = next;
                }
                None => {
                    count += end - position;
                    position = end;
                }
            }
        }
    }
    count
}

fn compare_with(documents: &[Entry], baseline: &Summary) -> BaselineComparison {
    let before: BTreeMap<&str, &Entry> = baseline
        .documents
        .iter()
        .map(|entry| (entry.document.sha256.as_str(), entry))
        .collect();
    let now: BTreeSet<&str> = documents
        .iter()
        .map(|entry| entry.document.sha256.as_str())
        .collect();
    let mut comparison = BaselineComparison {
        title: baseline.title.clone(),
        ..BaselineComparison::default()
    };
    for entry in documents {
        let Some(old) = before.get(entry.document.sha256.as_str()) else {
            comparison.added += 1;
            continue;
        };
        comparison.documents += 1;
        // Word's layout is another file: M1 to M3 start again from this run. A layout that appeared or disappeared is compared: a lost one loses every break.
        let rebaselined = old.reference_sha256.is_some()
            && entry.reference_sha256.is_some()
            && old.reference_sha256 != entry.reference_sha256;
        if rebaselined {
            comparison.rebaselined.push(entry.document.clone());
        }
        let (old_comparison, new_comparison) = if rebaselined {
            (None, None)
        } else {
            (old.comparison.as_ref(), entry.comparison.as_ref())
        };
        let change = Change {
            document: entry.document.clone(),
            page_count_lost: old_comparison.is_some_and(|old| old.m1.matches())
                && !new_comparison.is_some_and(|new| new.m1.matches()),
            page_count_gained: new_comparison.is_some_and(|new| new.m1.matches())
                && !old_comparison.is_some_and(|old| old.m1.matches()),
            page_breaks: delta(
                old_comparison.map(|old| &old.m2),
                new_comparison.map(|new| &new.m2),
            ),
            line_breaks: delta(
                old_comparison.map(|old| &old.m3),
                new_comparison.map(|new| &new.m3),
            ),
            hints: delta(
                old.hints.as_ref().map(|old| &old.hints),
                entry.hints.as_ref().map(|new| &new.hints),
            ),
        };
        if change.regressed() {
            comparison.regressions.push(change);
        } else if change.improved() {
            comparison.improvements.push(change);
        }
    }
    comparison.missing = baseline
        .documents
        .iter()
        .filter(|entry| !now.contains(entry.document.sha256.as_str()))
        .map(|entry| entry.document.clone())
        .collect();
    comparison
}
