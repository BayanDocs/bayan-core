//! M7: the `w:lastRenderedPageBreak` hints of synthetic `.docx` files, and their agreement with BayanDocs' page starts.

use bayan_lab_compare::hints::{Error, Hints, Limits, read};
use bayan_lab_compare::metrics::{self, Breaks, HintAgreement, PageCount, Place, hint_agreement};
use bayan_opc::{PackageWriter, PartName, Relationship, RelationshipSource, TargetMode};

use crate::support::{at, empty_layout, engine_layout, lines};

const OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";

/// A package whose main document part has `body` as the content of its `w:body`.
fn docx(body: &str) -> Vec<u8> {
    let part = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:wps=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\" \
         xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"wps\">\
         <w:body>{body}</w:body></w:document>"
    );
    let mut writer = PackageWriter::new();
    let name = PartName::new("/word/document.xml").unwrap();
    writer
        .add_part(
            &name,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            part.into_bytes(),
        )
        .unwrap();
    let relationships = writer
        .relationships_mut(&RelationshipSource::Package)
        .unwrap();
    let id = relationships.next_id();
    relationships
        .add(Relationship::new(
            id,
            OFFICE_DOCUMENT,
            "word/document.xml",
            TargetMode::Internal,
        ))
        .unwrap();
    writer.finish().unwrap()
}

fn text(hints: &Hints) -> String {
    hints.chars.iter().collect()
}

#[test]
fn the_body_text_and_its_hints() {
    let body = "\
        <w:p><w:r><w:t>Page one</w:t></w:r></w:p>\
        <w:p><w:r><w:lastRenderedPageBreak/><w:t xml:space=\"preserve\">Page two </w:t></w:r>\
        <w:r><w:tab/><w:t>tab</w:t><w:noBreakHyphen/><w:t>x</w:t><w:sym w:font=\"Symbol\" w:char=\"F0B7\"/></w:r></w:p>\
        <w:tbl><w:tr><w:tc><w:p><w:r><w:lastRenderedPageBreak/><w:t>Cell</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
    let hints = read(&docx(body), &Limits::DEFAULT).unwrap();
    assert_eq!(text(&hints), "PageonePagetwotab-x\u{F0B7}Cell");
    assert_eq!(hints.breaks, [7, 20]);
}

#[test]
fn text_that_is_not_displayed_is_left_out() {
    let body = "\
        <w:p><w:r><w:t>Kept</w:t></w:r>\
        <w:del w:id=\"1\" w:author=\"A\"><w:r><w:delText>Deleted</w:delText></w:r></w:del>\
        <w:ins w:id=\"2\" w:author=\"A\"><w:r><w:t>Inserted</w:t></w:r></w:ins>\
        <w:moveFrom w:id=\"3\" w:author=\"A\"><w:r><w:t>Moved away</w:t></w:r></w:moveFrom>\
        <w:r><w:rPr><w:vanish/></w:rPr><w:t>Hidden</w:t></w:r>\
        <w:r><w:rPr><w:vanish w:val=\"false\"/></w:rPr><w:t>Shown</w:t></w:r>\
        <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText>PAGE</w:instrText></w:r>\
        <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
        <w:r><w:drawing><wps:txbx><w:txbxContent><w:p><w:r><w:t>Floating</w:t></w:r></w:p></w:txbxContent></wps:txbx></w:drawing></w:r>\
        <w:r><w:t>End</w:t></w:r></w:p>\
        <w:p><w:pPr><w:rPr><w:del w:id=\"4\" w:author=\"A\"/><w:vanish/></w:rPr></w:pPr><w:r><w:t>Mark</w:t></w:r></w:p>";
    let hints = read(&docx(body), &Limits::DEFAULT).unwrap();
    assert_eq!(text(&hints), "KeptInsertedShown3EndMark");
    assert!(hints.breaks.is_empty());
}

#[test]
fn bayandocs_pages_that_start_at_the_hints_agree() {
    let body = "\
        <w:p><w:r><w:t>First page</w:t></w:r></w:p>\
        <w:p><w:r><w:lastRenderedPageBreak/><w:t>Second page</w:t></w:r></w:p>\
        <w:p><w:r><w:lastRenderedPageBreak/><w:t>Third page</w:t></w:r></w:p>";
    let hints = read(&docx(body), &Limits::DEFAULT).unwrap();
    // BayanDocs repeats a header on every page, which the body text lacks: the hints still agree.
    let agreeing = engine_layout(vec![
        lines(&[at("Header", 40), at("First page", 100)]),
        lines(&[at("Header", 40), at("Second page", 100)]),
        lines(&[at("Header", 40), at("Third page", 100)]),
    ]);
    assert_eq!(
        hint_agreement(&hints, &agreeing, &metrics::Limits::DEFAULT).unwrap(),
        HintAgreement {
            hints: Breaks {
                total: 2,
                agreed: 2,
                agreed_breaks: vec![[0, 2]],
                first_difference: None,
            },
            pages: PageCount {
                reference: 3,
                candidate: 3,
            },
        }
    );
    // BayanDocs keeps the second and third pages' text on one page.
    let fewer = engine_layout(vec![
        lines(&[at("First page", 100)]),
        lines(&[at("Second page", 100), at("Third page", 114)]),
    ]);
    assert_eq!(
        hint_agreement(&hints, &fewer, &metrics::Limits::DEFAULT).unwrap(),
        HintAgreement {
            hints: Breaks {
                total: 2,
                agreed: 1,
                agreed_breaks: vec![[0, 1]],
                first_difference: Some(Place {
                    page: 3,
                    line: None
                }),
            },
            pages: PageCount {
                reference: 3,
                candidate: 2,
            },
        }
    );
    // No layout at all: no hint agrees.
    let none = hint_agreement(&hints, &empty_layout(), &metrics::Limits::DEFAULT).unwrap();
    assert_eq!(none.hints.agreed, 0);
    assert_eq!(none.pages.difference(), -3);
}

#[test]
fn files_that_are_not_documents_are_refused_without_quoting_them() {
    assert!(matches!(
        read(b"not a package", &Limits::DEFAULT),
        Err(Error::Package(_))
    ));
    let mut writer = PackageWriter::new();
    writer
        .add_part(
            &PartName::new("/secret-name.xml").unwrap(),
            "application/xml",
            b"<a/>".to_vec(),
        )
        .unwrap();
    let error = read(&writer.finish().unwrap(), &Limits::DEFAULT).unwrap_err();
    assert_eq!(error, Error::NoMainDocument);
    let error = read(&docx("<w:p><w:r><w:t>secret</w:t></w:p>"), &Limits::DEFAULT).unwrap_err();
    assert!(matches!(error, Error::Xml(_)));
    assert!(!error.to_string().contains("secret"), "{error}");
}

#[test]
fn the_reader_keeps_to_its_limits() {
    let body = "<w:p><w:r><w:t>abcdef</w:t><w:lastRenderedPageBreak/><w:lastRenderedPageBreak/></w:r></w:p>";
    let bytes = docx(body);
    let text_limit = Limits {
        max_chars: 5,
        ..Limits::DEFAULT
    };
    assert_eq!(read(&bytes, &text_limit), Err(Error::TooMuchText(5)));
    let hint_limit = Limits {
        max_hints: 1,
        ..Limits::DEFAULT
    };
    assert_eq!(read(&bytes, &hint_limit), Err(Error::TooManyHints(1)));
    assert_eq!(read(&bytes, &Limits::DEFAULT).unwrap().breaks, [6, 6]);
    // Nesting costs constant time per element: a hundred nested content controls with a hint at the bottom.
    let depth = 100;
    let nested = format!(
        "{}<w:p><w:r><w:lastRenderedPageBreak/><w:t>x</w:t></w:r></w:p>{}",
        "<w:sdt><w:sdtContent>".repeat(depth),
        "</w:sdtContent></w:sdt>".repeat(depth)
    );
    assert_eq!(read(&docx(&nested), &Limits::DEFAULT).unwrap().breaks, [0]);
}
