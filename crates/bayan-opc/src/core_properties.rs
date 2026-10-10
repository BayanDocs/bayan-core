//! The core properties part: title, author, dates and the other package metadata of ECMA-376 Part 2 §8, read and written minimally, as text.

use crate::content_types::DECLARATION;
use crate::xml::{self, Element, Node};
use crate::{CorePropertiesError, Error, Limits, PackageError};

/// The relationship type of the package relationship to the core properties part.
pub const RELATIONSHIP_TYPE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";

/// The media type of the core properties part.
pub const CONTENT_TYPE: &str = "application/vnd.openxmlformats-package.core-properties+xml";

/// The namespace of the core properties part's own elements.
pub const NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";

/// The Dublin Core elements (ISO 15836-1).
const DC: &str = "http://purl.org/dc/elements/1.1/";
/// The Dublin Core terms (ISO 15836-2).
const DCTERMS: &str = "http://purl.org/dc/terms/";
/// The DCMI type vocabulary, which Word declares.
const DCMITYPE: &str = "http://purl.org/dc/dcmitype/";
/// XML Schema instance attributes, for `xsi:type`.
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// The core properties of a package, each as the text of its element. A property that the part does not have is `None`.
///
/// Values are kept as written, without interpretation: dates, for example, stay W3CDTF text such as `2026-10-09T12:00:00Z`. bayan-opc never sets a date itself, because the engine does not read the clock (ADR-0012); the caller supplies every value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreProperties {
    /// `dc:title`.
    pub title: Option<String>,
    /// `dc:subject`.
    pub subject: Option<String>,
    /// `dc:creator`: who made the content.
    pub creator: Option<String>,
    /// `cp:keywords`; keywords that the part writes in `value` elements are joined to the text in document order.
    pub keywords: Option<String>,
    /// `dc:description`.
    pub description: Option<String>,
    /// `cp:lastModifiedBy`.
    pub last_modified_by: Option<String>,
    /// `cp:revision`.
    pub revision: Option<String>,
    /// `cp:lastPrinted`.
    pub last_printed: Option<String>,
    /// `dcterms:created`.
    pub created: Option<String>,
    /// `dcterms:modified`.
    pub modified: Option<String>,
    /// `cp:category`.
    pub category: Option<String>,
    /// `cp:contentStatus`.
    pub content_status: Option<String>,
    /// `dc:language`.
    pub language: Option<String>,
    /// `cp:version`.
    pub version: Option<String>,
    /// `dc:identifier`.
    pub identifier: Option<String>,
}

/// Which element holds which property, in the order Word writes them.
const PROPERTIES: [(&str, &str); 15] = [
    (DC, "title"),
    (DC, "subject"),
    (DC, "creator"),
    (NAMESPACE, "keywords"),
    (DC, "description"),
    (NAMESPACE, "lastModifiedBy"),
    (NAMESPACE, "revision"),
    (NAMESPACE, "lastPrinted"),
    (DCTERMS, "created"),
    (DCTERMS, "modified"),
    (NAMESPACE, "category"),
    (NAMESPACE, "contentStatus"),
    (DC, "language"),
    (NAMESPACE, "version"),
    (DC, "identifier"),
];

impl CoreProperties {
    /// The fields in the order of [`PROPERTIES`].
    fn fields(&self) -> [&Option<String>; 15] {
        [
            &self.title,
            &self.subject,
            &self.creator,
            &self.keywords,
            &self.description,
            &self.last_modified_by,
            &self.revision,
            &self.last_printed,
            &self.created,
            &self.modified,
            &self.category,
            &self.content_status,
            &self.language,
            &self.version,
            &self.identifier,
        ]
    }

    fn fields_mut(&mut self) -> [&mut Option<String>; 15] {
        [
            &mut self.title,
            &mut self.subject,
            &mut self.creator,
            &mut self.keywords,
            &mut self.description,
            &mut self.last_modified_by,
            &mut self.revision,
            &mut self.last_printed,
            &mut self.created,
            &mut self.modified,
            &mut self.category,
            &mut self.content_status,
            &mut self.language,
            &mut self.version,
            &mut self.identifier,
        ]
    }

    /// Reads a core properties part. Elements that are not core properties are ignored.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] if it is larger than [`Limits::max_metadata_size`], [`Error::Xml`] if it is not acceptable XML or has more than [`Limits::max_xml_nodes`] nodes, and [`PackageError::CoreProperties`] if the root is not `coreProperties` or a property appears twice (§8.3.4.1: core properties are not repeatable).
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self, Error> {
        let mut nodes = limits.max_xml_nodes;
        CoreProperties::parse_entry(bytes, limits, None, &mut nodes)
    }

    /// Reads the part at ZIP entry `entry` (if known), counting its XML nodes against `nodes`, a budget that the metadata parts of one package share.
    pub(crate) fn parse_entry(
        bytes: &[u8],
        limits: &Limits,
        entry: Option<usize>,
        nodes: &mut usize,
    ) -> Result<Self, Error> {
        let document = xml::parse_part(bytes, limits, entry, nodes)?;
        let invalid = |error| Error::Package(PackageError::CoreProperties(error));
        if !document.root.is(NAMESPACE, "coreProperties") {
            return Err(invalid(CorePropertiesError::UnexpectedRoot));
        }
        let mut properties = CoreProperties::default();
        for node in &document.root.children {
            let Node::Element(element) = node else {
                continue;
            };
            let known = PROPERTIES
                .iter()
                .position(|&(namespace, local)| element.is(namespace, local));
            let Some(index) = known else {
                continue;
            };
            let mut fields = properties.fields_mut();
            let Some(field) = fields.get_mut(index) else {
                continue;
            };
            if field.is_some() {
                return Err(invalid(CorePropertiesError::DuplicateProperty));
            }
            **field = Some(text_content(element));
        }
        Ok(properties)
    }

    /// The core properties part as XML, laid out as Word writes it: the properties that are set, in Word's order, with `xsi:type="dcterms:W3CDTF"` on the two dates as §8.3.4.3 requires.
    pub fn to_xml(&self) -> Vec<u8> {
        let mut text = String::from(DECLARATION);
        text.push_str("<cp:coreProperties xmlns:cp=\"");
        text.push_str(NAMESPACE);
        text.push_str("\" xmlns:dc=\"");
        text.push_str(DC);
        text.push_str("\" xmlns:dcterms=\"");
        text.push_str(DCTERMS);
        text.push_str("\" xmlns:dcmitype=\"");
        text.push_str(DCMITYPE);
        text.push_str("\" xmlns:xsi=\"");
        text.push_str(XSI);
        text.push_str("\">");
        for ((namespace, local), value) in PROPERTIES.iter().zip(self.fields()) {
            let Some(value) = value else {
                continue;
            };
            let prefix = match *namespace {
                DC => "dc",
                DCTERMS => "dcterms",
                _ => "cp",
            };
            text.push('<');
            text.push_str(prefix);
            text.push(':');
            text.push_str(local);
            if *namespace == DCTERMS {
                text.push_str(" xsi:type=\"dcterms:W3CDTF\"");
            }
            text.push('>');
            xml::escape_text(value, &mut text);
            text.push_str("</");
            text.push_str(prefix);
            text.push(':');
            text.push_str(local);
            text.push('>');
        }
        text.push_str("</cp:coreProperties>");
        text.into_bytes()
    }
}

/// The text of `element` and of everything inside it, in document order.
fn text_content(element: &Element) -> String {
    let mut text = String::new();
    let mut pending: Vec<&Node> = element.children.iter().rev().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Text { value, .. } | Node::CData { value, .. } => text.push_str(value),
            Node::Element(child) => pending.extend(child.children.iter().rev()),
            Node::Markup { .. } => {}
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<cp:coreProperties xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dcmitype=\"http://purl.org/dc/dcmitype/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:title>A &amp; B</dc:title><dc:subject></dc:subject><dc:creator>Ana</dc:creator><cp:keywords></cp:keywords><dc:description></dc:description><cp:lastModifiedBy>Ana</cp:lastModifiedBy><cp:revision>2</cp:revision><dcterms:created xsi:type=\"dcterms:W3CDTF\">2026-10-01T09:00:00Z</dcterms:created><dcterms:modified xsi:type=\"dcterms:W3CDTF\">2026-10-02T10:30:00Z</dcterms:modified></cp:coreProperties>";

    #[test]
    fn reads_and_writes_core_properties() {
        let properties = CoreProperties::parse(WORD.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(properties.title.as_deref(), Some("A & B"));
        assert_eq!(properties.subject.as_deref(), Some(""));
        assert_eq!(properties.creator.as_deref(), Some("Ana"));
        assert_eq!(properties.revision.as_deref(), Some("2"));
        assert_eq!(properties.created.as_deref(), Some("2026-10-01T09:00:00Z"));
        assert_eq!(properties.modified.as_deref(), Some("2026-10-02T10:30:00Z"));
        assert_eq!(properties.category, None);
        // Written the way Word writes it, the part comes out byte for byte the same.
        assert_eq!(properties.to_xml(), WORD.as_bytes());
    }

    #[test]
    fn reads_the_example_of_the_standard() {
        let text = "<coreProperties xmlns=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" xmlns:dcterms=\"http://purl.org/dc/terms/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"><dc:creator>Alan Shen</dc:creator><dcterms:created xsi:type=\"dcterms:W3CDTF\">2005-06-12</dcterms:created><dc:title>OPC Core Properties</dc:title><dc:language>eng</dc:language><version>1.0</version><keywords xml:lang=\"en-US\">color <value xml:lang=\"en-CA\">colour</value> <value xml:lang=\"fr-FR\">couleur</value></keywords><unknown>ignored</unknown></coreProperties>";
        let properties = CoreProperties::parse(text.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(properties.creator.as_deref(), Some("Alan Shen"));
        assert_eq!(properties.created.as_deref(), Some("2005-06-12"));
        assert_eq!(properties.language.as_deref(), Some("eng"));
        assert_eq!(properties.version.as_deref(), Some("1.0"));
        assert_eq!(properties.keywords.as_deref(), Some("color colour couleur"));
    }

    #[test]
    fn refuses_duplicates_and_other_roots() {
        let duplicate = format!(
            "<cp:coreProperties xmlns:cp=\"{NAMESPACE}\" xmlns:dc=\"{DC}\"><dc:title>a</dc:title><dc:title>b</dc:title></cp:coreProperties>"
        );
        assert_eq!(
            CoreProperties::parse(duplicate.as_bytes(), &Limits::default()),
            Err(Error::Package(PackageError::CoreProperties(
                CorePropertiesError::DuplicateProperty
            )))
        );
        assert_eq!(
            CoreProperties::parse(b"<coreProperties/>", &Limits::default()),
            Err(Error::Package(PackageError::CoreProperties(
                CorePropertiesError::UnexpectedRoot
            )))
        );
    }

    #[test]
    fn writes_every_property_and_reads_it_back() {
        let properties = CoreProperties {
            title: Some("t <&>".to_owned()),
            subject: Some("s".to_owned()),
            creator: Some("c".to_owned()),
            keywords: Some("k".to_owned()),
            description: Some("d\r\nline".to_owned()),
            last_modified_by: Some("l".to_owned()),
            revision: Some("1".to_owned()),
            last_printed: Some("2026-01-01T00:00:00Z".to_owned()),
            created: Some("2026-01-02T00:00:00Z".to_owned()),
            modified: Some("2026-01-03T00:00:00Z".to_owned()),
            category: Some("cat".to_owned()),
            content_status: Some("Draft".to_owned()),
            language: Some("ar-SA".to_owned()),
            version: Some("1.0".to_owned()),
            identifier: Some("urn:x".to_owned()),
        };
        let read = CoreProperties::parse(&properties.to_xml(), &Limits::default()).unwrap();
        assert_eq!(read, properties);
        assert_eq!(
            CoreProperties::parse(&CoreProperties::default().to_xml(), &Limits::default()).unwrap(),
            CoreProperties::default()
        );
    }
}
