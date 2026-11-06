//! A corpus run's summary: aggregation by tier, compatibility mode, script and feature, the worst documents, the correlations of feature tags with disagreement, and (LAB-004 AC-4) the documents that lose agreement against a baseline.

use bayan_lab_compare::metrics::{
    Breaks, Comparison, HintAgreement, Limits, Measured, PageCount, compare,
};
use bayan_lab_compare::summary::{
    Availability, BaselineComparison, Change, Correlation, Delta, Document, HintsOutcome, Named,
    Outcome, Summary, Tier,
};
use bayan_lab_layout::layout::Layout;

use crate::support::{at, empty_layout, engine_layout, lines, pdf_layout};

/// The SHA-256 of a test document: its letter, 64 times.
pub fn sha(letter: char) -> String {
    letter.to_string().repeat(64)
}

pub fn document(
    letter: char,
    tier: Tier,
    features: &[&str],
    scripts: &[&str],
    mode: Option<u32>,
) -> Document {
    Document {
        sha256: sha(letter),
        tier,
        name: Some(format!("source:{letter}.docx")),
        features: features.iter().map(|&feature| feature.to_owned()).collect(),
        scripts: scripts.iter().map(|&script| script.to_owned()).collect(),
        compatibility_mode: mode,
    }
}

/// Three lines on one page, as Word lays them out.
pub fn word() -> Layout {
    pdf_layout(vec![lines(&[
        at("The quick brown fox", 100),
        at("jumps over the lazy dog", 114),
        at("Next paragraph", 128),
    ])])
}

/// BayanDocs' layout of [`word`], alike.
pub fn alike() -> Layout {
    engine_layout(vec![lines(&[
        at("The quick brown fox", 100),
        at("jumps over the lazy dog", 114),
        at("Next paragraph", 128),
    ])])
}

/// BayanDocs' layout of [`word`], with the second line starting a word earlier.
pub fn broken() -> Layout {
    engine_layout(vec![lines(&[
        at("The quick brown", 100),
        at("fox jumps over the lazy dog", 114),
        at("Next paragraph", 128),
    ])])
}

/// The outcome of comparing `candidate` with Word's layout `reference`, whose file has the SHA-256 `digest`.
pub fn compared(
    document: Document,
    reference: &Layout,
    candidate: Option<&Layout>,
    digest: char,
) -> Outcome {
    let empty = empty_layout();
    let measured = compare(reference, candidate.unwrap_or(&empty), &Limits::DEFAULT).unwrap();
    Outcome {
        document,
        reference: Availability::Present,
        reference_digest: Some(sha(digest)),
        candidate: if candidate.is_some() {
            Availability::Present
        } else {
            Availability::Missing
        },
        measured: Some(measured),
        hints: HintsOutcome::NotRead,
        problem: None,
    }
}

pub fn hinted(document: Document, total: u64, agreed: &[[u64; 2]]) -> Outcome {
    Outcome {
        document,
        reference: Availability::Missing,
        reference_digest: None,
        candidate: Availability::Missing,
        measured: None,
        hints: HintsOutcome::Agreement(HintAgreement {
            hints: Breaks {
                total,
                agreed: agreed.iter().map(|range| range[1] - range[0]).sum(),
                agreed_breaks: agreed.to_vec(),
                first_difference: None,
            },
            pages: PageCount {
                reference: u32::try_from(total + 1).unwrap(),
                candidate: 0,
            },
        }),
        problem: None,
    }
}

/// A run of five documents of every kind: two public ones compared, a private one without BayanDocs' layout, a public one with hints only, and a private one whose hints were refused.
pub fn run() -> Vec<Outcome> {
    vec![
        compared(
            document('a', Tier::T1, &["Tables", "Fields"], &["Latin"], Some(15)),
            &word(),
            Some(&alike()),
            'A',
        ),
        compared(
            document('b', Tier::T1, &["Tables"], &["Latin", "Hebrew"], None),
            &word(),
            Some(&broken()),
            'B',
        ),
        compared(
            document('c', Tier::T2, &["Fields"], &["Latin"], Some(14)),
            &word(),
            None,
            'C',
        ),
        hinted(
            document('d', Tier::T1, &["Footnotes"], &["Latin"], Some(15)),
            4,
            &[[0, 1], [2, 3]],
        ),
        Outcome {
            hints: HintsOutcome::Refused("the package was refused: limit exceeded".to_owned()),
            ..hinted(document('e', Tier::T2, &[], &[], None), 0, &[])
        },
    ]
}

#[test]
fn documents_are_aggregated_by_tier_mode_script_and_feature() {
    let summary = Summary::new("Test run", &run(), None).unwrap();
    let all = &summary.overall;
    assert_eq!(all.documents, 5);
    assert_eq!(all.compared, 3);
    assert_eq!(all.without_reference, 2);
    assert_eq!(all.without_candidate, 1);
    assert_eq!(all.failed, 0);
    // Lines: 3 of 3 (a), 2 of 3 (b), 0 of 3 (c).
    assert_eq!((all.m3.total, all.m3.agreed), (9, 5));
    assert_eq!((all.m2.total, all.m2.agreed), (3, 2));
    assert_eq!(
        (
            all.m1.documents,
            all.m1.matching,
            all.m1.absolute_difference
        ),
        (3, 2, 1)
    );
    // Glyphs: all 48 of a (16, 19 and 13 on its lines), the 13 of b's last line.
    assert_eq!((all.m4.lines, all.m4.glyphs), (4, 61));
    assert_eq!(
        (
            all.m7.documents,
            all.m7.total,
            all.m7.agreed,
            all.m7.refused
        ),
        (1, 4, 2, 1)
    );
    assert_eq!(all.text.reference, 3 * 48);
    assert_eq!(all.text.matched, 48 + 48);

    let names =
        |map: &std::collections::BTreeMap<String, _>| map.keys().cloned().collect::<Vec<String>>();
    assert_eq!(names(&summary.tiers), ["T1", "T2"]);
    assert_eq!(summary.tiers["T1"].documents, 3);
    assert_eq!(summary.tiers["T2"].documents, 2);
    assert_eq!(names(&summary.compatibility_modes), ["14", "15", "not set"]);
    assert_eq!(summary.compatibility_modes["15"].documents, 2);
    assert_eq!(names(&summary.scripts), ["Hebrew", "Latin"]);
    assert_eq!(summary.scripts["Hebrew"].m3.agreed, 2);
    assert_eq!(names(&summary.features), ["Fields", "Footnotes", "Tables"]);
    assert_eq!(
        (
            summary.features["Tables"].m3.total,
            summary.features["Tables"].m3.agreed
        ),
        (6, 5)
    );
    assert_eq!(summary.features["Fields"].without_candidate, 1);

    // Worst: public documents only, lowest agreement first.
    let worst: Vec<&str> = summary
        .worst
        .line_breaks
        .iter()
        .map(|ranked| ranked.document.sha256.as_str())
        .collect();
    assert_eq!(worst, [sha('b'), sha('a')]);
    assert_eq!(summary.worst.hints.len(), 1);
    // Private documents carry no name, in any list.
    for entry in &summary.documents {
        assert_eq!(
            entry.document.name.is_none(),
            entry.document.tier == Tier::T2
        );
    }
    assert_eq!(
        summary.documents[4].hints_problem.as_deref(),
        Some("the package was refused: limit exceeded")
    );
}

#[test]
fn summaries_are_deterministic_and_read_back() {
    let summary = Summary::new("Test run", &run(), None).unwrap();
    let text = summary.to_json().unwrap();
    assert!(text.ends_with("}\n"));
    // The order of the outcomes does not matter.
    let mut reversed = run();
    reversed.reverse();
    assert_eq!(
        Summary::new("Test run", &reversed, None)
            .unwrap()
            .to_json()
            .unwrap(),
        text
    );
    assert_eq!(Summary::from_json(&text).unwrap(), summary);
    assert!(
        Summary::from_json(&text.replace("\"metrics\": 1", "\"metrics\": 2"))
            .unwrap_err()
            .contains("version 2")
    );
    assert!(Summary::from_json("{}").is_err());
    let twice = [run().remove(0), run().remove(0)];
    assert!(
        Summary::new("Test run", &twice, None)
            .unwrap_err()
            .contains("listed twice")
    );
}

/// An outcome whose line breaks are `agreed` of `total`, for correlations.
fn disagreeing(letter: char, features: &[&str], total: u64, agreed: u64) -> Outcome {
    let mut outcome = compared(
        document(letter, Tier::T1, features, &[], None),
        &word(),
        Some(&alike()),
        letter,
    );
    outcome.measured = Some(Measured {
        comparison: Comparison {
            m3: Breaks {
                total,
                agreed,
                agreed_breaks: if agreed > 0 {
                    vec![[0, agreed]]
                } else {
                    Vec::new()
                },
                first_difference: None,
            },
            ..Comparison::default()
        },
        ..Measured::default()
    });
    outcome
}

#[test]
fn feature_tags_correlated_with_disagreement() {
    // Five documents that disagree entirely (0 to 4) and five that agree entirely (5 to 9).
    // F is on exactly the first five; H on 0, 1, 2, 5 and 6; G on 0, 1, 5, 6 and 7, which goes with agreement and is not listed.
    let tags = |index: u32| {
        let mut tags = Vec::new();
        if index < 5 {
            tags.push("F");
        }
        if [0, 1, 2, 5, 6].contains(&index) {
            tags.push("H");
        }
        if [0, 1, 5, 6, 7].contains(&index) {
            tags.push("G");
        }
        tags
    };
    let outcomes: Vec<Outcome> = (0..10_u32)
        .map(|index| {
            let letter = char::from_digit(index, 10).unwrap();
            disagreeing(letter, &tags(index), 4, if index < 5 { 0 } else { 4 })
        })
        .collect();
    let summary = Summary::new("Correlations", &outcomes, None).unwrap();
    assert_eq!(
        summary.correlations.line_breaks,
        [
            Correlation {
                feature: "F".to_owned(),
                with: 5,
                without: 5,
                disagreement_with: 1000,
                disagreement_without: 0,
                correlation: 1000,
            },
            // r = (10 × 3000 − 5 × 5000) / sqrt(5 × 5 × (10 × 5·10⁶ − 5000²)) = 0.2.
            Correlation {
                feature: "H".to_owned(),
                with: 5,
                without: 5,
                disagreement_with: 600,
                disagreement_without: 400,
                correlation: 200,
            },
        ]
    );
    assert!(summary.correlations.hints.is_empty());
    // When every document disagrees alike, nothing correlates.
    let alike: Vec<Outcome> = (0..10_u32)
        .map(|index| disagreeing(char::from_digit(index, 10).unwrap(), &tags(index), 4, 0))
        .collect();
    assert!(
        Summary::new("Alike", &alike, None)
            .unwrap()
            .correlations
            .line_breaks
            .is_empty()
    );
}

fn named(letter: char, tier: Tier) -> Named {
    Named {
        sha256: sha(letter),
        name: (!tier.is_private()).then(|| format!("source:{letter}.docx")),
        tier,
    }
}

/// LAB-004 AC-4: a document that loses agreement relative to the baseline is flagged; one that only gains is not.
#[test]
fn regressions_against_a_baseline() {
    let before = Summary::new(
        "Baseline",
        &[
            compared(
                document('a', Tier::T1, &[], &[], None),
                &word(),
                Some(&alike()),
                'A',
            ),
            compared(
                document('b', Tier::T1, &[], &[], None),
                &word(),
                Some(&broken()),
                'B',
            ),
            compared(
                document('c', Tier::T2, &[], &[], None),
                &word(),
                Some(&alike()),
                'C',
            ),
            hinted(document('d', Tier::T1, &[], &[], None), 3, &[[0, 3]]),
            compared(
                document('e', Tier::T1, &[], &[], None),
                &word(),
                Some(&alike()),
                'E',
            ),
            compared(
                document('g', Tier::T1, &[], &[], None),
                &word(),
                Some(&alike()),
                'G',
            ),
        ],
        None,
    )
    .unwrap();
    // Round trip through JSON, as a stored baseline is read.
    let before = Summary::from_json(&before.to_json().unwrap()).unwrap();
    let now = Summary::new(
        "Now",
        &[
            // a now breaks its second line elsewhere: it loses that line.
            compared(
                document('a', Tier::T1, &[], &[], None),
                &word(),
                Some(&broken()),
                'A',
            ),
            // b now agrees everywhere: it gains a line and loses nothing.
            compared(
                document('b', Tier::T1, &[], &[], None),
                &word(),
                Some(&alike()),
                'B',
            ),
            // c, private, lost BayanDocs' layout: everything is lost, its page count too.
            compared(document('c', Tier::T2, &[], &[], None), &word(), None, 'C'),
            // d's second hint no longer agrees.
            hinted(
                document('d', Tier::T1, &[], &[], None),
                3,
                &[[0, 1], [2, 3]],
            ),
            // e is gone; f is new; g's ground truth was made again.
            compared(
                document('f', Tier::T1, &[], &[], None),
                &word(),
                Some(&alike()),
                'F',
            ),
            compared(
                document('g', Tier::T1, &[], &[], None),
                &word(),
                Some(&broken()),
                'H',
            ),
        ],
        Some(&before),
    )
    .unwrap();
    assert!(now.regressed());
    let unchanged = Delta::default();
    assert_eq!(
        now.baseline.as_ref().unwrap(),
        &BaselineComparison {
            title: "Baseline".to_owned(),
            documents: 5,
            regressions: vec![
                Change {
                    document: named('a', Tier::T1),
                    page_count_lost: false,
                    page_count_gained: false,
                    page_breaks: unchanged,
                    line_breaks: Delta { lost: 1, gained: 0 },
                    hints: unchanged,
                },
                Change {
                    document: named('c', Tier::T2),
                    page_count_lost: true,
                    page_count_gained: false,
                    page_breaks: Delta { lost: 1, gained: 0 },
                    line_breaks: Delta { lost: 3, gained: 0 },
                    hints: unchanged,
                },
                Change {
                    document: named('d', Tier::T1),
                    page_count_lost: false,
                    page_count_gained: false,
                    page_breaks: unchanged,
                    line_breaks: unchanged,
                    hints: Delta { lost: 1, gained: 0 },
                },
            ],
            improvements: vec![Change {
                document: named('b', Tier::T1),
                page_count_lost: false,
                page_count_gained: false,
                page_breaks: unchanged,
                line_breaks: Delta { lost: 0, gained: 1 },
                hints: unchanged,
            }],
            rebaselined: vec![named('g', Tier::T1)],
            missing: vec![named('e', Tier::T1)],
            added: 1,
        }
    );
    // Against itself, nothing regresses.
    let same = Summary::new(
        "Again",
        &run(),
        Some(&Summary::new("Before", &run(), None).unwrap()),
    )
    .unwrap();
    assert!(!same.regressed());
    assert!(same.baseline.as_ref().unwrap().improvements.is_empty());
}

#[test]
fn a_document_that_loses_one_break_and_gains_another_still_regresses() {
    let breaks = |agreed: &[[u64; 2]]| Breaks {
        total: 4,
        agreed: agreed.iter().map(|range| range[1] - range[0]).sum(),
        agreed_breaks: agreed.to_vec(),
        first_difference: None,
    };
    let with = |letter, agreed: &[[u64; 2]]| {
        let mut outcome = compared(
            document(letter, Tier::T1, &[], &[], None),
            &word(),
            Some(&alike()),
            'A',
        );
        if let Some(measured) = outcome.measured.as_mut() {
            measured.comparison.m3 = breaks(agreed);
        }
        outcome
    };
    let before = Summary::new("Before", &[with('a', &[[0, 2]])], None).unwrap();
    let now = Summary::new("Now", &[with('a', &[[0, 1], [2, 3]])], Some(&before)).unwrap();
    let change = &now.baseline.as_ref().unwrap().regressions[0];
    assert_eq!(change.line_breaks, Delta { lost: 1, gained: 1 });
    assert!(now.regressed());
}
