//! LAB-004 AC-2: the HTML report renders offline (no scripts, nothing from other origins) and lists per-tier, per-feature and worst-document results; it never names a private document.

use bayan_lab_compare::report::html;
use bayan_lab_compare::summary::{Summary, Tier};
use bayan_xml::{Event, Limits, Reader, Source};

use crate::summary::{alike, broken, compared, document, run, sha, word};

/// The elements of the page, each with its attributes' names, read strictly as XML after the document type declaration.
fn elements(page: &str) -> Vec<(String, Vec<String>)> {
    let body = page
        .strip_prefix("<!DOCTYPE html>\n")
        .expect("the page starts with its document type");
    let limits = Limits::default();
    let source = Source::new(body.as_bytes(), &limits).expect("the page is valid XML");
    let mut reader = Reader::new(&source, &limits);
    let mut found = Vec::new();
    while let Some(event) = reader.next_event().expect("the page is well-formed") {
        if let Event::Start(tag) = event {
            found.push((
                tag.name().local().to_owned(),
                tag.attributes()
                    .map(|attribute| attribute.name().local().to_owned())
                    .collect(),
            ));
        }
    }
    found
}

fn baseline_run() -> Summary {
    let before = Summary::new(
        "Baseline",
        &[compared(
            document('a', Tier::T1, &["Tables"], &["Latin"], Some(15)),
            &word(),
            Some(&alike()),
            'A',
        )],
        None,
    )
    .unwrap();
    let mut outcomes = run();
    outcomes[0] = compared(
        document('a', Tier::T1, &["Tables"], &["Latin"], Some(15)),
        &word(),
        Some(&broken()),
        'A',
    );
    // A public name that tries to be markup.
    outcomes[1].document.name = Some("x/<script>alert('1')</script>&\"q\".docx".to_owned());
    Summary::new("Report <test>", &outcomes, Some(&before)).unwrap()
}

#[test]
fn the_report_stands_alone_and_is_well_formed() {
    let page = html(&baseline_run());
    let elements = elements(&page);
    for (name, attributes) in &elements {
        assert!(
            ![
                "script", "link", "img", "image", "iframe", "object", "embed", "base", "form",
                "use", "audio", "video", "source"
            ]
            .contains(&name.as_str()),
            "<{name}>"
        );
        for attribute in attributes {
            assert!(
                !["href", "src", "srcset", "action", "background", "poster"]
                    .contains(&attribute.as_str())
                    && !attribute.starts_with("on"),
                "<{name} {attribute}>"
            );
        }
    }
    assert!(!page.contains("http"), "no address of any origin");
    assert!(!page.contains("url("));
    assert!(!page.contains("@import"));
    // The names a document chose are text, not markup.
    assert!(
        page.contains("x/&lt;script&gt;alert(&#39;1&#39;)&lt;/script&gt;&amp;&quot;q&quot;.docx")
    );
    assert!(page.contains("<title>Report &lt;test&gt;</title>"));
    // The same summary gives the same bytes.
    assert_eq!(html(&baseline_run()), page);
}

#[test]
fn the_report_lists_tiers_features_and_the_worst_documents() {
    let summary = baseline_run();
    let page = html(&summary);
    let ids: Vec<&str> = page
        .match_indices("<section id=\"")
        .map(|(at, found)| {
            let rest = &page[at + found.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(
        ids,
        [
            "overview",
            "regressions",
            "tiers",
            "modes",
            "scripts",
            "features",
            "correlations",
            "worst",
            "refused",
            "about"
        ]
    );
    let section = |id: &str| {
        let start = page.find(&format!("<section id=\"{id}\">")).unwrap();
        let end = start + page[start..].find("</section>").unwrap();
        &page[start..end]
    };
    assert!(section("tiers").contains("<td>T1</td>"));
    assert!(section("tiers").contains("<td>T2</td>"));
    assert!(section("features").contains("<td>Tables</td>"));
    assert!(section("features").contains("<td>Footnotes</td>"));
    assert!(section("modes").contains("<td>not set</td>"));
    assert!(section("scripts").contains("<td>Hebrew</td>"));
    // Line-break agreement of a: 2 of 3 lines.
    assert!(section("worst").contains("source:a.docx"));
    assert!(section("worst").contains("title=\"2 of 3\">66.7 %"));
    assert!(section("regressions").contains("source:a.docx"));
    assert!(section("regressions").contains("1 documents lost agreement"));
    // The refused private document is counted, not listed.
    assert!(section("refused").contains("Private documents are counted, not listed."));
}

#[test]
fn private_documents_are_never_named() {
    let summary = baseline_run();
    let page = html(&summary);
    for private in ['c', 'e'] {
        assert!(!page.contains(&sha(private)), "document {private}");
        assert!(!page.contains(&sha(private)[..12]), "document {private}");
        assert!(!page.contains(&format!("source:{private}.docx")));
    }
    let json = summary.to_json().unwrap();
    assert!(!json.contains("source:c.docx") && !json.contains("source:e.docx"));
}
