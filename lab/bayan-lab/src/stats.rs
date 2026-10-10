//! Frequencies over a corpus manifest: by feature, font, script, language, compatibility mode, legacy compatibility option, source, license and application.
//!
//! Everything is counted in documents (how many documents use a feature), with integer arithmetic only, and written in a fixed order (most frequent first, then by name), so the same manifest always gives the same report. Reports contain only aggregate numbers and names of features, fonts, scripts and languages, never document content or file names, so they may also be made for the private corpus.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::{Value, json};

use crate::manifest::Manifest;
use crate::scan::features::{FEATURES, Feature};
use crate::scan::{TAGGER_VERSION, is_hidden_character};

/// Document counts of a manifest.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Documents in the manifest.
    pub documents: u64,
    /// Documents that are tagged.
    pub tagged: u64,
    /// Their total size, in bytes.
    pub total_size: u64,
    /// Documents per tier.
    pub tiers: BTreeMap<String, u64>,
    /// Documents per source (by first provenance).
    pub sources: BTreeMap<String, u64>,
    /// Documents per license.
    pub licenses: BTreeMap<String, u64>,
    /// Documents per application that last saved them (`(none recorded)` when absent).
    pub applications: BTreeMap<String, u64>,
    /// Documents per feature.
    pub features: BTreeMap<Feature, u64>,
    /// Documents per font.
    pub fonts: BTreeMap<String, u64>,
    /// Documents per script, and characters per script.
    pub scripts: BTreeMap<String, (u64, u64)>,
    /// Documents per language tag.
    pub languages: BTreeMap<String, u64>,
    /// Documents per compatibility mode (`none` when absent).
    pub compatibility_modes: BTreeMap<String, u64>,
    /// Documents per legacy compatibility option.
    pub compatibility_options: BTreeMap<String, u64>,
}

fn bump<K: Ord>(map: &mut BTreeMap<K, u64>, key: K) {
    let count = map.entry(key).or_insert(0);
    *count = count.saturating_add(1);
}

impl Stats {
    /// Counts a manifest.
    #[must_use]
    pub fn of(manifest: &Manifest) -> Self {
        let mut stats = Self::default();
        for document in &manifest.documents {
            stats.documents += 1;
            stats.total_size = stats.total_size.saturating_add(document.size);
            bump(&mut stats.tiers, document.tier.name().to_owned());
            bump(&mut stats.licenses, document.license.clone());
            if let Some(first) = document.provenance.first() {
                bump(&mut stats.sources, first.source.clone());
            }
            let Some(scan) = &document.scan else {
                continue;
            };
            stats.tagged += 1;
            bump(
                &mut stats.applications,
                scan.application
                    .clone()
                    .unwrap_or_else(|| "(none recorded)".to_owned()),
            );
            for name in &scan.features {
                if let Some(feature) = Feature::from_name(name) {
                    bump(&mut stats.features, feature);
                }
            }
            for font in &scan.fonts {
                bump(&mut stats.fonts, font.clone());
            }
            for (script, characters) in &scan.scripts {
                let entry = stats
                    .scripts
                    .entry(script.name().to_owned())
                    .or_insert((0, 0));
                entry.0 = entry.0.saturating_add(1);
                entry.1 = entry.1.saturating_add(*characters);
            }
            for language in &scan.languages {
                bump(&mut stats.languages, language.clone());
            }
            let mode = scan
                .compatibility
                .mode
                .map_or_else(|| "none".to_owned(), |mode| mode.to_string());
            bump(&mut stats.compatibility_modes, mode);
            for option in &scan.compatibility.options {
                bump(&mut stats.compatibility_options, option.clone());
            }
        }
        stats
    }

    /// The counts as JSON.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let features: BTreeMap<&str, u64> = FEATURES
            .iter()
            .map(|info| {
                (
                    info.name,
                    self.features.get(&info.feature).copied().unwrap_or(0),
                )
            })
            .collect();
        let scripts: BTreeMap<&str, Value> = self
            .scripts
            .iter()
            .map(|(script, (documents, characters))| {
                (
                    script.as_str(),
                    json!({ "documents": documents, "characters": characters }),
                )
            })
            .collect();
        json!({
            "documents": self.documents,
            "tagged": self.tagged,
            "total_size": self.total_size,
            "tagger": TAGGER_VERSION,
            "tiers": self.tiers,
            "sources": self.sources,
            "licenses": self.licenses,
            "applications": self.applications,
            "features": features,
            "fonts": self.fonts,
            "scripts": scripts,
            "languages": self.languages,
            "compatibility_modes": self.compatibility_modes,
            "compatibility_options": self.compatibility_options,
        })
    }

    /// The counts as a Markdown report, headed by `title`.
    #[must_use]
    pub fn to_markdown(&self, title: &str, manifest: &Manifest) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# {}\n", escape(title));
        let _ = writeln!(
            out,
            "Generated by `bayan-lab corpus stats --format markdown` from the corpus manifest; do not edit it by hand. Counts are documents (how many documents use a feature, a font, a script…), with shares rounded to one decimal: of all {} documents for licenses, and of the {} tagged documents for everything the tagger found. Feature names are those of the [Word feature coverage matrix](https://github.com/BayanDocs/docs/blob/HEAD/specs/coverage-matrix.md); how each is detected is described in `lab/corpus/tagging-rules.md`. Test documents of software projects use features more often than everyday documents do, so these shares rank features by how often they occur in this corpus, not in the world.\n",
            self.documents, self.tagged
        );
        let _ = writeln!(out, "## Corpus\n");
        let _ = writeln!(out, "| | |\n|---|---|");
        let _ = writeln!(out, "| Documents | {} |", self.documents);
        let _ = writeln!(
            out,
            "| Tagged (tagger version {TAGGER_VERSION}) | {} |",
            self.tagged
        );
        let _ = writeln!(out, "| Total size | {} |", size(self.total_size));
        for (tier, count) in &self.tiers {
            let _ = writeln!(out, "| Tier {} | {count} |", escape(tier));
        }
        let _ = writeln!(out, "\n### Sources\n");
        let _ = writeln!(
            out,
            "| Source | Revision | License | Documents |\n|---|---|---|---:|"
        );
        for (name, count) in by_count(&self.sources) {
            let (revision, license) = manifest.sources.get(name).map_or(("", ""), |source| {
                (source.revision.as_str(), source.license.as_str())
            });
            let short: String = revision.chars().take(12).collect();
            let _ = writeln!(
                out,
                "| {} | `{}` | {} | {count} |",
                escape(name),
                escape(&short),
                escape(license)
            );
        }
        self.table(
            &mut out,
            "### Licenses",
            "License",
            &self.licenses,
            self.documents,
        );
        self.table(
            &mut out,
            "### Applications that last saved the documents",
            "Application",
            &self.applications,
            self.tagged,
        );
        let _ = writeln!(out, "\n## Features\n");
        let mut section = "";
        for info in &FEATURES {
            if info.section != section {
                section = info.section;
                let _ = writeln!(out, "\n### {}\n", escape(section));
                let _ = writeln!(out, "| Feature | Documents | Share |\n|---|---:|---:|");
            }
            match info.feature.not_detectable() {
                Some(_) => {
                    let _ = writeln!(
                        out,
                        "| {} | not detectable by scanning | |",
                        escape(info.name)
                    );
                }
                None => {
                    let count = self.features.get(&info.feature).copied().unwrap_or(0);
                    let _ = writeln!(
                        out,
                        "| {} | {count} | {} |",
                        escape(info.name),
                        share(count, self.tagged)
                    );
                }
            }
        }
        let _ = writeln!(out, "\n## Scripts\n");
        let _ = writeln!(
            out,
            "Characters of the document text by Unicode block; digits, punctuation and symbols shared by all scripts are not counted.\n"
        );
        let _ = writeln!(
            out,
            "| Script | Documents | Share | Characters |\n|---|---:|---:|---:|"
        );
        let mut scripts: Vec<(&String, &(u64, u64))> = self.scripts.iter().collect();
        scripts.sort_by(|a, b| b.1.0.cmp(&a.1.0).then_with(|| a.0.cmp(b.0)));
        for (script, (documents, characters)) in scripts {
            let _ = writeln!(
                out,
                "| {} | {documents} | {} | {characters} |",
                escape(script),
                share(*documents, self.tagged)
            );
        }
        self.table(
            &mut out,
            "## Compatibility modes",
            "Mode",
            &self.compatibility_modes,
            self.tagged,
        );
        self.table(
            &mut out,
            "## Legacy compatibility options",
            "Option",
            &self.compatibility_options,
            self.tagged,
        );
        self.table(
            &mut out,
            "## Languages declared",
            "Language tag",
            &self.languages,
            self.tagged,
        );
        self.table(&mut out, "## Fonts named", "Font", &self.fonts, self.tagged);
        out
    }

    /// A table of counts, each with its share of `total`.
    fn table(
        &self,
        out: &mut String,
        heading: &str,
        column: &str,
        counts: &BTreeMap<String, u64>,
        total: u64,
    ) {
        let _ = writeln!(out, "\n{heading}\n");
        if counts.is_empty() {
            let _ = writeln!(out, "None.");
            return;
        }
        let _ = writeln!(out, "| {column} | Documents | Share |\n|---|---:|---:|");
        for (name, count) in by_count(counts) {
            let _ = writeln!(
                out,
                "| {} | {count} | {} |",
                escape(name),
                share(count, total)
            );
        }
    }
}

/// Entries sorted by count, most frequent first, then by name.
fn by_count(counts: &BTreeMap<String, u64>) -> Vec<(&String, u64)> {
    let mut entries: Vec<(&String, u64)> =
        counts.iter().map(|(name, count)| (name, *count)).collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    entries
}

/// `count` as a share of `total`, in per cent with one decimal, rounded half up, by integer arithmetic.
#[must_use]
pub fn share(count: u64, total: u64) -> String {
    if total == 0 {
        return "–".to_owned();
    }
    let tenths = (u128::from(count) * 1000 + u128::from(total) / 2) / u128::from(total);
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// A size in bytes, with its value in MiB to one decimal.
#[must_use]
pub fn size(bytes: u64) -> String {
    let tenths = (u128::from(bytes) * 10 + 524_288) / 1_048_576;
    format!("{bytes} bytes ({}.{} MiB)", tenths / 10, tenths % 10)
}

/// Text for a Markdown table cell: `|`, `\` and the characters that start Markdown markup are escaped, and control characters replaced.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '|' | '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' => {
                out.push('\\');
                out.push(character);
            }
            character if is_hidden_character(character) => out.push('?'),
            character => out.push(character),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shares_round_half_up_with_integers() {
        assert_eq!(share(1, 3), "33.3%");
        assert_eq!(share(2, 3), "66.7%");
        assert_eq!(share(1, 8), "12.5%");
        assert_eq!(share(1, 2000), "0.1%");
        assert_eq!(share(1, 2001), "0.0%");
        assert_eq!(share(5, 5), "100.0%");
        assert_eq!(share(0, 0), "–");
        assert_eq!(size(1_048_576), "1048576 bytes (1.0 MiB)");
        assert_eq!(size(0), "0 bytes (0.0 MiB)");
    }

    #[test]
    fn markdown_cells_are_escaped() {
        assert_eq!(escape("a|b_c\u{7}"), "a\\|b\\_c?");
        assert_eq!(escape("Ari\u{202E}la"), "Ari?la");
    }

    #[test]
    fn shares_never_exceed_the_whole() {
        use crate::hash::Sha256;
        use crate::manifest::{Document, GroundTruth, Provenance, Tier};
        let mut manifest = Manifest::default();
        for number in 0_u8..6 {
            let document = Document {
                sha256: Sha256::of(&[number]),
                size: 1,
                tier: Tier::T1,
                license: "MIT".to_owned(),
                provenance: vec![Provenance {
                    source: "example".to_owned(),
                    path: None,
                }],
                scan: None,
                page_count: None,
                ground_truth: GroundTruth::default(),
                notes: None,
            };
            assert!(manifest.insert(document).is_ok());
        }
        let report = Stats::of(&manifest).to_markdown("Untagged", &manifest);
        assert!(report.contains("| MIT | 6 | 100.0% |"), "{report}");
    }
}
