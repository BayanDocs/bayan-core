//! Exclusion lists: documents a curator reviewed and decided to keep out of a corpus, each identified by its SHA-256 and given a reason.
//!
//! An exclusion list is a tab-separated text file, which GitHub shows as a table. Its first line names the columns, `sha256`, `source`, `path` and `reason`; every other line describes one excluded document: the SHA-256 of its bytes, the source and the path where the reviewer found it (one of them, if the same bytes occur several times), and why it is kept out. Lines are sorted by source, then path, and no SHA-256 appears twice.
//!
//! `bayan-lab corpus add --exclude FILE` skips every listed document wherever it is found, because the SHA-256, not the path, identifies it, and `bayan-lab corpus verify --exclude FILE` fails if the manifest contains one.

use std::collections::BTreeMap;

use crate::hash::Sha256;
use crate::manifest::{is_relative_path, is_source_name};

/// The first line of an exclusion list.
pub const HEADER: &str = "sha256\tsource\tpath\treason";

/// The largest exclusion list accepted, in bytes.
pub const MAX_LIST_SIZE: u64 = 16 * 1024 * 1024;

/// Why one document is excluded, and where it was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclusion {
    /// The source in which the reviewer found it.
    pub source: String,
    /// Its path in that source.
    pub path: String,
    /// Why it is excluded.
    pub reason: String,
}

/// An exclusion list, by SHA-256.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Exclusions {
    by_sha256: BTreeMap<Sha256, Exclusion>,
}

impl Exclusions {
    /// Reads an exclusion list.
    ///
    /// # Errors
    ///
    /// Every problem found, one per line, each naming its line: a wrong header, a line without exactly four fields, an invalid SHA-256, source name or path, an empty reason, lines out of order, or a SHA-256 listed twice.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut problems = Vec::new();
        let Some(body) = text.strip_suffix('\n') else {
            return Err("an exclusion list ends with a line feed".to_owned());
        };
        if body.contains('\r') {
            return Err(
                "an exclusion list uses line feeds only, without carriage returns".to_owned(),
            );
        }
        let mut lines = body.split('\n');
        if lines.next() != Some(HEADER) {
            return Err(format!(
                "the first line of an exclusion list is `{}`",
                HEADER.replace('\t', "<TAB>")
            ));
        }
        let mut exclusions = Self::default();
        let mut previous: Option<(String, String)> = None;
        for (index, line) in lines.enumerate() {
            let number = index + 2;
            let fields: Vec<&str> = line.split('\t').collect();
            let [sha256, source, path, reason] = fields.as_slice() else {
                problems.push(format!(
                    "line {number}: four fields separated by tabs are expected"
                ));
                continue;
            };
            let Ok(sha256) = sha256.parse::<Sha256>() else {
                problems.push(format!(
                    "line {number}: `{sha256}` is not a SHA-256 in lower-case hexadecimal"
                ));
                continue;
            };
            if !is_source_name(source) {
                problems.push(format!("line {number}: `{source}` is not a source name"));
            }
            if !is_relative_path(path) {
                problems.push(format!(
                    "line {number}: the path is not a relative path inside its source"
                ));
            }
            if reason.trim().is_empty() || reason.trim() != *reason {
                problems.push(format!(
                    "line {number}: the reason is empty or starts or ends with spaces"
                ));
            }
            let place = ((*source).to_owned(), (*path).to_owned());
            if previous.as_ref().is_some_and(|previous| *previous >= place) {
                problems.push(format!(
                    "line {number}: lines are sorted by source and path, each pair once"
                ));
            }
            previous = Some(place);
            let exclusion = Exclusion {
                source: (*source).to_owned(),
                path: (*path).to_owned(),
                reason: (*reason).to_owned(),
            };
            if exclusions.by_sha256.insert(sha256, exclusion).is_some() {
                problems.push(format!("line {number}: {} is listed twice", sha256.short()));
            }
        }
        if problems.is_empty() {
            Ok(exclusions)
        } else {
            Err(problems.join("\n"))
        }
    }

    /// Why the document with this SHA-256 is excluded, if it is.
    #[must_use]
    pub fn get(&self, sha256: &Sha256) -> Option<&Exclusion> {
        self.by_sha256.get(sha256)
    }

    /// How many documents are excluded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_sha256.len()
    }

    /// Whether the list excludes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_sha256.is_empty()
    }

    /// The excluded documents, by SHA-256.
    pub fn iter(&self) -> impl Iterator<Item = (&Sha256, &Exclusion)> {
        self.by_sha256.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(bytes: &[u8], source: &str, path: &str, reason: &str) -> String {
        format!("{}\t{source}\t{path}\t{reason}\n", Sha256::of(bytes))
    }

    #[test]
    fn reads_a_list() {
        let text = format!(
            "{HEADER}\n{}{}",
            line(b"a", "alpha", "docs/a.docx", "third-party content"),
            line(b"b", "beta", "b.docx", "refused by the scanner")
        );
        let list = Exclusions::parse(&text).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.get(&Sha256::of(b"a"))
                .map(|found| found.reason.as_str()),
            Some("third-party content")
        );
        assert!(list.get(&Sha256::of(b"c")).is_none());
        assert!(
            Exclusions::parse(&format!("{HEADER}\n"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn refuses_malformed_lists() {
        let good = line(b"a", "alpha", "a.docx", "reason");
        for (text, expected) in [
            (
                format!("{HEADER}\n{good}").replace('\n', "\r\n"),
                "carriage",
            ),
            (
                format!("{HEADER}\n{}", good.trim_end()),
                "ends with a line feed",
            ),
            (format!("sha256,source,path,reason\n{good}"), "first line"),
            (format!("{HEADER}\n{good}{good}"), "listed twice"),
            (
                format!(
                    "{HEADER}\n{}{}",
                    line(b"b", "beta", "b.docx", "reason"),
                    line(b"a", "alpha", "a.docx", "reason")
                ),
                "sorted",
            ),
            (
                format!("{HEADER}\nx\talpha\ta.docx\treason\n"),
                "not a SHA-256",
            ),
            (
                format!("{HEADER}\n{}", line(b"a", "Alpha", "a.docx", "reason")),
                "source name",
            ),
            (
                format!("{HEADER}\n{}", line(b"a", "alpha", "../a.docx", "reason")),
                "relative path",
            ),
            (
                format!("{HEADER}\n{}", line(b"a", "alpha", "a.docx", " ")),
                "reason",
            ),
            (
                format!("{HEADER}\n{}\textra\n", good.trim_end()),
                "four fields",
            ),
        ] {
            let error = Exclusions::parse(&text).unwrap_err();
            assert!(error.contains(expected), "{expected:?} not in {error:?}");
        }
    }
}
