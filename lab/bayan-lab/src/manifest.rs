//! The corpus manifest: one JSON file, committed to Git, that describes every document of a corpus without containing any of them.
//!
//! Each document is identified by the SHA-256 of its bytes and carries its provenance, its license, its tier, what the tagger found in it, and its ground-truth status. Each source (an upstream repository at a pinned revision, for example) is described once, with its license and the texts of its license and notice files, which the store keeps beside the documents. `lab/corpus/manifest-schema.md` documents every field; [`crate::schema`] generates the JSON Schema.
//!
//! The manifest is written deterministically: documents sorted by SHA-256, sources and every list sorted, two-space indentation, a final line feed. The same corpus always gives the same bytes on every platform.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::hash::Sha256;
use crate::scan::features::Feature;
use crate::scan::scripts::Script;
use crate::scan::{Findings, TAGGER_VERSION};

/// The value of [`Manifest::format`].
pub const FORMAT: &str = "bayan-lab-corpus-manifest";

/// The version of the manifest's layout, [`Manifest::version`].
pub const VERSION: u32 = 1;

/// The licenses a document of a public tier may have: redistributable licenses, each checked to allow publishing the corpus with its notices (LAB-001). Adding one needs a reviewed pull request.
pub const PUBLIC_LICENSES: [&str; 7] = [
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC-BY-4.0",
    "CC0-1.0",
    "MIT",
    "MPL-2.0",
];

/// The license of a private (T2) document: no license is asserted, because such documents are never redistributed.
pub const NO_ASSERTION: &str = "NOASSERTION";

/// A corpus manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Always `bayan-lab-corpus-manifest`.
    pub format: String,
    /// The version of the manifest's layout; this is version 1.
    pub version: u32,
    /// The sources of the documents, by name.
    pub sources: BTreeMap<String, Source>,
    /// The documents, sorted by SHA-256.
    pub documents: Vec<Document>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            format: FORMAT.to_owned(),
            version: VERSION,
            sources: BTreeMap::new(),
            documents: Vec::new(),
        }
    }
}

/// Where documents come from, described once for all of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Where the source can be found, such as a repository's HTTPS address.
    pub url: String,
    /// The exact revision the documents were taken from, such as a full Git commit hash.
    pub revision: String,
    /// The SPDX license expression under which the source distributes its documents.
    pub license: String,
    /// The copyright notice of the source, as its license file states it.
    pub copyright: String,
    /// The source's license and notice files, which the store keeps by SHA-256 so the corpus can be redistributed with them.
    pub license_texts: Vec<LicenseText>,
    /// Why the license applies to the documents, and anything else a reviewer should know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// A license or notice file of a source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LicenseText {
    /// The file's path in the source, such as `LICENSE`.
    pub path: String,
    /// The SHA-256 of the file, under which the store keeps it.
    pub sha256: Sha256,
}

/// The corpus tiers of the Fidelity Lab specification (§3).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
pub enum Tier {
    /// Probes: small generated documents, each isolating one behavior.
    T0,
    /// Public: real or realistic documents with redistributable licenses.
    T1,
    /// Private: real-world documents that are never redistributed.
    T2,
    /// Torture: known-hard documents, public or private as their licenses allow.
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
}

impl fmt::Display for Tier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl std::str::FromStr for Tier {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "T0" => Ok(Self::T0),
            "T1" => Ok(Self::T1),
            "T2" => Ok(Self::T2),
            "T3" => Ok(Self::T3),
            _ => Err(format!(
                "unknown tier `{text}`; the tiers are T0, T1, T2 and T3"
            )),
        }
    }
}

/// One document of the corpus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// The SHA-256 of the document's bytes: its identity, and its key in the store.
    pub sha256: Sha256,
    /// Its size in bytes.
    pub size: u64,
    /// Its corpus tier.
    pub tier: Tier,
    /// The SPDX license expression under which the corpus redistributes it; `NOASSERTION` for private documents.
    pub license: String,
    /// Where it was found: at least one source, the first being the one whose license applies.
    pub provenance: Vec<Provenance>,
    /// What the tagger found; `null` until the document is tagged.
    pub scan: Option<Scan>,
    /// Its page count in Word, once ground truth exists (LAB-002); `null` until then.
    pub page_count: Option<u32>,
    /// Whether Word ground truth exists for it.
    pub ground_truth: GroundTruth,
    /// Curator's notes: never document content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Where a document was found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// The name of a source in [`Manifest::sources`].
    pub source: String,
    /// The document's path in the source. Never recorded for private documents, whose file names can reveal personal data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Whether Word ground truth exists for a document (LAB-002 fills it in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GroundTruth {
    /// `none`, `produced` or `failed`.
    pub status: GroundTruthStatus,
    /// The Word build that produced it, once produced.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_build: Option<String>,
}

impl Default for GroundTruth {
    fn default() -> Self {
        Self {
            status: GroundTruthStatus::None,
            word_build: None,
        }
    }
}

/// The ground-truth status of a document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroundTruthStatus {
    /// Not produced yet.
    None,
    /// Produced by the Word build in [`GroundTruth::word_build`].
    Produced,
    /// Word could not produce it (the harness logs why).
    Failed,
}

/// What the tagger found in a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scan {
    /// The version of the tagging rules that produced this ([`TAGGER_VERSION`]).
    pub tagger: u32,
    /// The coverage-matrix features the document uses, by their names in the matrix, sorted.
    pub features: Vec<String>,
    /// The fonts it names, sorted.
    pub fonts: Vec<String>,
    /// The language tags it declares, sorted.
    pub languages: Vec<String>,
    /// Characters per script in its text.
    pub scripts: BTreeMap<Script, u64>,
    /// Its compatibility settings.
    pub compatibility: Compatibility,
    /// The application that last saved it, from its application properties.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application: Option<String>,
    /// The page count that application recorded: a hint until Word's own count is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages_hint: Option<u32>,
    /// Non-fatal observations of the tagger.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// A document's compatibility settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Compatibility {
    /// The `compatibilityMode` setting (15 for Word 2013 and later, 14, 12 or 11), or `null` when the document has none.
    pub mode: Option<u32>,
    /// The legacy compatibility options it switches on, by element name, sorted.
    pub options: Vec<String>,
}

impl Scan {
    /// The manifest form of what the tagger found.
    #[must_use]
    pub fn from_findings(findings: &Findings) -> Self {
        Self {
            tagger: TAGGER_VERSION,
            features: findings
                .features
                .iter()
                .map(|feature| feature.name().to_owned())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            fonts: findings.fonts.iter().cloned().collect(),
            languages: findings.languages.iter().cloned().collect(),
            scripts: findings.scripts.clone(),
            compatibility: Compatibility {
                mode: findings.compatibility_mode,
                options: findings.compatibility_options.iter().cloned().collect(),
            },
            application: findings.application.clone(),
            pages_hint: findings.pages_hint,
            notes: findings
                .notes
                .iter()
                .map(|note| (*note).to_owned())
                .collect(),
        }
    }
}

/// Why a manifest is not consistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problems(pub Vec<String>);

impl fmt::Display for Problems {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for problem in &self.0 {
            writeln!(formatter, "- {problem}")?;
        }
        Ok(())
    }
}

impl Manifest {
    /// Reads a manifest from JSON.
    ///
    /// # Errors
    ///
    /// When the text is not a manifest of this format and version.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let manifest: Self = serde_json::from_str(text)
            .map_err(|error| format!("not a corpus manifest: {error}"))?;
        if manifest.format != FORMAT {
            return Err(format!("not a corpus manifest: `format` is not `{FORMAT}`"));
        }
        if manifest.version != VERSION {
            return Err(format!(
                "manifest version {} is not supported; this tool reads version {VERSION}",
                manifest.version
            ));
        }
        Ok(manifest)
    }

    /// The manifest as JSON, in its deterministic layout.
    ///
    /// # Errors
    ///
    /// Never in practice: every value of a manifest can be written as JSON.
    pub fn to_json(&self) -> Result<String, String> {
        let mut text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        text.push('\n');
        Ok(text)
    }

    /// The document with this SHA-256.
    #[must_use]
    pub fn document(&self, sha256: &Sha256) -> Option<&Document> {
        self.documents
            .binary_search_by(|document| document.sha256.cmp(sha256))
            .ok()
            .and_then(|index| self.documents.get(index))
    }

    /// Inserts a document, keeping the list sorted.
    ///
    /// # Errors
    ///
    /// The existing entry, unchanged, if the manifest already has a document with this SHA-256.
    pub fn insert(&mut self, document: Document) -> Result<(), &mut Document> {
        match self
            .documents
            .binary_search_by(|existing| existing.sha256.cmp(&document.sha256))
        {
            Ok(index) => match self.documents.get_mut(index) {
                Some(existing) => Err(existing),
                // Unreachable: binary_search returned a valid index.
                None => Ok(()),
            },
            Err(index) => {
                self.documents.insert(index, document);
                Ok(())
            }
        }
    }

    /// Checks that the manifest is consistent on its own, without looking at the store: identities unique and sorted, sources defined, licenses allowed for each tier, tags known and current, and the privacy rules of private documents.
    ///
    /// # Errors
    ///
    /// Every problem found.
    pub fn check(&self) -> Result<(), Problems> {
        let mut problems = Vec::new();
        for (name, source) in &self.sources {
            check_source(name, source, &mut problems);
        }
        let mut provenance_paths = BTreeMap::new();
        for pair in self.documents.windows(2) {
            if let [first, second] = pair
                && first.sha256 >= second.sha256
            {
                problems.push(format!(
                    "documents are not sorted by SHA-256, or {} appears twice",
                    second.sha256.short()
                ));
            }
        }
        for document in &self.documents {
            self.check_document(document, &mut problems, &mut provenance_paths);
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(Problems(problems))
        }
    }

    fn check_document<'m>(
        &'m self,
        document: &'m Document,
        problems: &mut Vec<String>,
        provenance_paths: &mut BTreeMap<(&'m str, &'m str), Sha256>,
    ) {
        let id = document.sha256.short();
        if document.size == 0 {
            problems.push(format!("{id}: size is 0"));
        }
        match document.tier {
            Tier::T2 => {
                if document.license != NO_ASSERTION {
                    problems.push(format!(
                        "{id}: a private (T2) document's license must be {NO_ASSERTION}"
                    ));
                }
                if document
                    .provenance
                    .iter()
                    .any(|provenance| provenance.path.is_some())
                {
                    problems.push(format!(
                        "{id}: a private (T2) document must not record paths"
                    ));
                }
            }
            Tier::T0 | Tier::T1 | Tier::T3 => {
                if !PUBLIC_LICENSES.contains(&document.license.as_str()) {
                    problems.push(format!(
                        "{id}: license `{}` is not one of the licenses allowed for public tiers ({})",
                        document.license,
                        PUBLIC_LICENSES.join(", ")
                    ));
                }
            }
        }
        let Some(first) = document.provenance.first() else {
            problems.push(format!("{id}: no provenance"));
            return;
        };
        if let Some(source) = self.sources.get(&first.source)
            && source.license != document.license
        {
            problems.push(format!(
                "{id}: license `{}` differs from the license `{}` of its first source `{}`",
                document.license, source.license, first.source
            ));
        }
        let mut seen = BTreeSet::new();
        for provenance in &document.provenance {
            if !self.sources.contains_key(&provenance.source) {
                problems.push(format!(
                    "{id}: source `{}` is not defined",
                    provenance.source
                ));
            }
            if !seen.insert(provenance) {
                problems.push(format!("{id}: the same provenance is recorded twice"));
            }
            if let Some(path) = &provenance.path {
                if !is_relative_path(path) {
                    problems.push(format!(
                        "{id}: provenance path is not a relative path inside its source"
                    ));
                }
                if let Some(other) = provenance_paths
                    .insert((provenance.source.as_str(), path.as_str()), document.sha256)
                    && other != document.sha256
                {
                    problems.push(format!(
                        "{id}: the same source path is also recorded for {}",
                        other.short()
                    ));
                }
            }
        }
        match &document.scan {
            None => problems.push(format!("{id}: not tagged; run `bayan-lab corpus tag`")),
            Some(scan) => check_scan(&id, scan, problems),
        }
        if document.ground_truth.status == GroundTruthStatus::Produced
            && document.ground_truth.word_build.is_none()
        {
            problems.push(format!(
                "{id}: ground truth is produced but its Word build is not recorded"
            ));
        }
        if let Some(notes) = &document.notes
            && notes.trim().is_empty()
        {
            problems.push(format!("{id}: notes are empty; leave them out instead"));
        }
    }
}

/// Whether `name` can name a source: lower-case ASCII letters, digits and hyphens, at least one.
#[must_use]
pub fn is_source_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn check_source(name: &str, source: &Source, problems: &mut Vec<String>) {
    if !is_source_name(name) {
        problems.push(format!(
            "source `{name}`: names use lower-case letters, digits and hyphens"
        ));
    }
    if !source.url.starts_with("https://") {
        problems.push(format!(
            "source `{name}`: its address must start with https://"
        ));
    }
    if source.revision.trim().is_empty() {
        problems.push(format!("source `{name}`: no revision"));
    }
    if !PUBLIC_LICENSES.contains(&source.license.as_str()) && source.license != NO_ASSERTION {
        problems.push(format!(
            "source `{name}`: license `{}` is not allowed",
            source.license
        ));
    }
    if source.copyright.trim().is_empty() && source.license != NO_ASSERTION {
        problems.push(format!("source `{name}`: no copyright notice"));
    }
    if source.license_texts.is_empty() && source.license != NO_ASSERTION {
        problems.push(format!("source `{name}`: no license text"));
    }
    if let Some(notes) = &source.notes
        && notes.trim().is_empty()
    {
        problems.push(format!(
            "source `{name}`: notes are empty; leave them out instead"
        ));
    }
    if !source
        .license_texts
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        problems.push(format!(
            "source `{name}`: license texts are not sorted or appear twice"
        ));
    }
    for text in &source.license_texts {
        if !is_relative_path(&text.path) {
            problems.push(format!(
                "source `{name}`: license text path `{}` is not a relative path",
                text.path
            ));
        }
    }
}

fn check_scan(id: &str, scan: &Scan, problems: &mut Vec<String>) {
    if scan.tagger != TAGGER_VERSION {
        problems.push(format!(
            "{id}: tagged by tagger version {}, but the current version is {TAGGER_VERSION}; run `bayan-lab corpus tag`",
            scan.tagger
        ));
    }
    for feature in &scan.features {
        match Feature::from_name(feature) {
            None => problems.push(format!("{id}: unknown feature `{feature}`")),
            Some(known) if known.not_detectable().is_some() => {
                problems.push(format!(
                    "{id}: feature `{feature}` cannot be detected by scanning"
                ));
            }
            Some(_) => {}
        }
    }
    for (name, list) in [
        ("features", &scan.features),
        ("fonts", &scan.fonts),
        ("languages", &scan.languages),
        ("compatibility options", &scan.compatibility.options),
        ("notes", &scan.notes),
    ] {
        if !list.windows(2).all(|pair| pair[0] < pair[1]) {
            problems.push(format!("{id}: {name} are not sorted or appear twice"));
        }
    }
    if scan.scripts.values().any(|&count| count == 0) {
        problems.push(format!("{id}: a script is listed with no characters"));
    }
}

/// Whether `path` is a relative path that stays inside its root on every platform: `/`-separated, without empty, `.` or `..` segments, backslashes, colons (`C:` would name a Windows drive, `a.docx:stream` an alternate data stream) or control characters.
#[must_use]
pub fn is_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path
            .chars()
            .any(|character| matches!(character, '\\' | ':') || character.is_control())
        && path
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source {
            url: "https://example.invalid/repo".into(),
            revision: "0123456789abcdef0123456789abcdef01234567".into(),
            license: "MIT".into(),
            copyright: "Copyright (c) Example".into(),
            license_texts: vec![LicenseText {
                path: "LICENSE".into(),
                sha256: Sha256::of(b"license"),
            }],
            notes: None,
        }
    }

    fn document(bytes: &[u8], path: &str) -> Document {
        Document {
            sha256: Sha256::of(bytes),
            size: u64::try_from(bytes.len()).unwrap(),
            tier: Tier::T1,
            license: "MIT".into(),
            provenance: vec![Provenance {
                source: "example".into(),
                path: Some(path.into()),
            }],
            scan: Some(Scan {
                tagger: TAGGER_VERSION,
                features: vec!["OPC package, content types, relationships".into()],
                fonts: vec!["Calibri".into()],
                languages: vec!["en-US".into()],
                scripts: BTreeMap::from([(Script::Latin, 3)]),
                compatibility: Compatibility {
                    mode: Some(15),
                    options: vec![],
                },
                application: None,
                pages_hint: None,
                notes: vec![],
            }),
            page_count: None,
            ground_truth: GroundTruth::default(),
            notes: None,
        }
    }

    fn manifest() -> Manifest {
        let mut manifest = Manifest::default();
        manifest.sources.insert("example".into(), source());
        manifest.insert(document(b"one", "a/one.docx")).unwrap();
        manifest.insert(document(b"two", "a/two.docx")).unwrap();
        manifest
    }

    #[test]
    fn a_consistent_manifest_round_trips_through_json() {
        let manifest = manifest();
        manifest.check().unwrap();
        let json = manifest.to_json().unwrap();
        assert_eq!(Manifest::from_json(&json).unwrap(), manifest);
        assert!(json.ends_with("}\n"));
        assert!(json.contains("\n  \"documents\": ["));
    }

    #[test]
    fn documents_stay_sorted_and_unique() {
        let mut manifest = manifest();
        assert!(manifest.insert(document(b"one", "b/one.docx")).is_err());
        let ids: Vec<Sha256> = manifest
            .documents
            .iter()
            .map(|document| document.sha256)
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
        assert!(manifest.document(&Sha256::of(b"two")).is_some());
        assert!(manifest.document(&Sha256::of(b"three")).is_none());
    }

    #[test]
    fn finds_inconsistencies() {
        let mut manifest = manifest();
        manifest.documents.swap(0, 1);
        let mut document = document(b"three", "a/one.docx");
        document.license = "GPL-3.0-only".into();
        document.provenance.push(Provenance {
            source: "missing".into(),
            path: Some("../x".into()),
        });
        if let Some(scan) = &mut document.scan {
            scan.tagger = 0;
            scan.features = vec!["Zebra".into(), "Rows splitting across pages".into()];
        }
        manifest.documents.push(document);
        let mut private = self::document(b"four", "secret.docx");
        private.tier = Tier::T2;
        private.notes = Some(" ".into());
        manifest.documents.push(private);
        if let Some(source) = manifest.sources.get_mut("example") {
            source.notes = Some(String::new());
        }
        let problems = manifest.check().unwrap_err().to_string();
        for expected in [
            "not sorted",
            "license `GPL-3.0-only` is not one of the licenses allowed",
            "source `missing` is not defined",
            "not a relative path",
            "the same source path is also recorded",
            "tagger version 0",
            "unknown feature `Zebra`",
            "cannot be detected by scanning",
            "private (T2) document's license must be NOASSERTION",
            "private (T2) document must not record paths",
            "source `example`: notes are empty",
        ] {
            assert!(
                problems.contains(expected),
                "missing {expected:?} in:\n{problems}"
            );
        }
        let empty_notes = format!("{}: notes are empty", Sha256::of(b"four").short());
        assert!(
            problems.contains(&empty_notes),
            "missing {empty_notes:?} in:\n{problems}"
        );
    }

    #[test]
    fn refuses_other_formats_and_versions_and_unknown_fields() {
        assert!(
            Manifest::from_json(r#"{"format":"x","version":1,"sources":{},"documents":[]}"#)
                .is_err()
        );
        assert!(
            Manifest::from_json(
                r#"{"format":"bayan-lab-corpus-manifest","version":2,"sources":{},"documents":[]}"#
            )
            .is_err()
        );
        assert!(Manifest::from_json(r#"{"format":"bayan-lab-corpus-manifest","version":1,"sources":{},"documents":[],"extra":1}"#).is_err());
        assert!(
            Manifest::from_json(
                r#"{"format":"bayan-lab-corpus-manifest","version":1,"sources":{},"documents":[]}"#
            )
            .is_ok()
        );
    }

    #[test]
    fn relative_paths_stay_inside_their_root() {
        for good in ["a", "a/b.docx", "test-data/document/Bug 1.docx"] {
            assert!(is_relative_path(good), "{good}");
        }
        for bad in [
            "",
            "/a",
            "a/../b",
            "./a",
            "a//b",
            "a\\b",
            "a/\u{0}",
            "C:/Users/me/secret.docx",
            "a/C:",
            "a.docx:stream",
        ] {
            assert!(!is_relative_path(bad), "{bad:?}");
        }
    }
}
