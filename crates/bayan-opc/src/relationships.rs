//! Relationships parts, which connect the package and its parts to other parts and to external resources (ECMA-376 Part 2 §6.5).

use std::collections::BTreeMap;
use std::ops::Range;

use crate::content_types::DECLARATION;
use crate::part_name::{PartName, RelationshipSource};
use crate::xml::{self, Element, Encoding, Node, XML_NAMESPACE};
use crate::{Error, Limits, PackageError, RelationshipsError, TargetError};

/// The namespace of relationships parts.
pub const NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// The media type of relationships parts.
pub const CONTENT_TYPE: &str = "application/vnd.openxmlformats-package.relationships+xml";

/// How a relationship's target is resolved: to a part of the package, or to a resource outside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetMode {
    /// The target is a part of the package (the default).
    Internal,
    /// The target is a resource outside the package, such as a web page or a file. BayanDocs records such targets and never fetches them on its own (ADR-0023).
    External,
}

/// One relationship: an identifier, a relationship type and a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relationship {
    id: String,
    relationship_type: String,
    target: String,
    target_mode: TargetMode,
}

/// What a relationship's target resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A part of the package (which may or may not exist).
    Part(PartName),
    /// A resource outside the package, as written: a reference that is only recorded, never fetched.
    External(String),
}

impl Relationship {
    /// A relationship with the identifier `id`, the type `relationship_type` and the target `target`.
    pub fn new(
        id: impl Into<String>,
        relationship_type: impl Into<String>,
        target: impl Into<String>,
        target_mode: TargetMode,
    ) -> Self {
        Relationship {
            id: id.into(),
            relationship_type: relationship_type.into(),
            target: target.into(),
            target_mode,
        }
    }

    /// The identifier, unique within its relationships part, by which the source refers to the relationship (such as `rId1`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The relationship type, an IRI that says what the target is to the source.
    pub fn relationship_type(&self) -> &str {
        &self.relationship_type
    }

    /// The target as written: a relative reference to a part, or for an external relationship, any IRI.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The target mode.
    pub fn target_mode(&self) -> TargetMode {
        self.target_mode
    }

    /// What the target resolves to, for a relationship from `source`: the part name of an internal target ([`RelationshipSource::resolve`]), or an external target as written.
    ///
    /// # Errors
    ///
    /// A [`TargetError`] if an internal target does not resolve to a valid part name.
    pub fn resolve(&self, source: &RelationshipSource) -> Result<Target, TargetError> {
        match self.target_mode {
            TargetMode::Internal => source.resolve(&self.target).map(Target::Part),
            TargetMode::External => Ok(Target::External(self.target.clone())),
        }
    }
}

/// The relationships of one source, as one relationships part holds them.
///
/// Like [`ContentTypes`](crate::ContentTypes), it keeps the document it was read from: written back without changes, it gives exactly the original text, and a change writes only the relationships it adds; the others, elements from other namespaces (extensions that Markup Compatibility allows here, §6.5.3.2), comments and formatting are copied as they were.
///
/// The relationships are kept in an ordered map by identifier, so finding one ([`Relationships::get`]), refusing a duplicate and choosing a new identifier take logarithmic time per relationship however many there are.
#[derive(Clone, Debug)]
pub struct Relationships {
    original: Option<Original>,
    /// The relationships, the elements from other namespaces, and the text between them, in document order.
    items: Vec<Item>,
    /// The relationships, by identifier.
    relationships: BTreeMap<String, Entry>,
    modified: bool,
}

#[derive(Clone, Debug)]
struct Original {
    text: String,
    encoding: Encoding,
    root_start: Range<usize>,
    root_end: Option<Range<usize>>,
    root_span_end: usize,
    root_name: String,
}

#[derive(Clone, Debug)]
enum Item {
    /// White space, comments or processing instructions between relationships, kept as written.
    Trivia(Range<usize>),
    /// An element from another namespace, kept as written and not interpreted.
    Extension(Range<usize>),
    /// The relationship with this identifier, kept in the map.
    Relationship(String),
}

#[derive(Clone, Debug)]
struct Entry {
    relationship: Relationship,
    /// The element in the original text.
    raw: Option<Range<usize>>,
    /// White space written before a new relationship, copied from its neighbours.
    indent: String,
}

impl Default for Relationships {
    fn default() -> Self {
        Relationships::new()
    }
}

impl Relationships {
    /// No relationships, for a new relationships part.
    pub fn new() -> Self {
        Relationships {
            original: None,
            items: Vec::new(),
            relationships: BTreeMap::new(),
            modified: true,
        }
    }

    /// Reads a relationships part.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] if it is larger than [`Limits::max_metadata_size`], [`Error::Xml`] if it is not acceptable XML or has more than [`Limits::max_xml_nodes`] nodes, and [`PackageError::Relationships`] if it breaks a rule of §6.5: an unexpected root, element or text, a missing `Id`, `Type` or `Target`, an invalid `TargetMode`, two relationships with one `Id`, or an `xml:base` attribute.
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self, Error> {
        let mut nodes = limits.max_xml_nodes;
        Relationships::parse_entry(bytes, limits, None, &mut nodes)
    }

    /// Reads the part at ZIP entry `entry` (if known), counting its XML nodes against `nodes`, a budget that the metadata parts of one package share.
    pub(crate) fn parse_entry(
        bytes: &[u8],
        limits: &Limits,
        entry: Option<usize>,
        nodes: &mut usize,
    ) -> Result<Self, Error> {
        let xml::Document {
            text,
            encoding,
            mut root,
        } = xml::parse_part(bytes, limits, entry, nodes)?;
        let invalid = |error| Error::Package(PackageError::Relationships { entry, error });
        if !root.is(NAMESPACE, "Relationships") {
            return Err(invalid(RelationshipsError::UnexpectedRoot));
        }
        if uses_xml_base(&root) {
            return Err(invalid(RelationshipsError::XmlBase));
        }
        let mut items = Vec::with_capacity(root.children.len());
        let mut relationships = BTreeMap::new();
        // Each child is dropped as soon as it has been read, so the tree's memory goes down while the model's goes up.
        for node in std::mem::take(&mut root.children) {
            match node {
                Node::Element(element) if element.is(NAMESPACE, "Relationship") => {
                    let relationship = read_relationship(&element).map_err(invalid)?;
                    if relationships.contains_key(&relationship.id) {
                        return Err(invalid(RelationshipsError::DuplicateId));
                    }
                    items.push(Item::Relationship(relationship.id.clone()));
                    relationships.insert(
                        relationship.id.clone(),
                        Entry {
                            relationship,
                            raw: Some(element.span.clone()),
                            indent: String::new(),
                        },
                    );
                }
                Node::Element(element) if element.namespace.as_deref() == Some(NAMESPACE) => {
                    return Err(invalid(RelationshipsError::UnexpectedElement));
                }
                Node::Element(element) => items.push(Item::Extension(element.span.clone())),
                Node::Text { span, value } if value.chars().all(xml::is_space) => {
                    items.push(Item::Trivia(span.clone()));
                }
                Node::Markup { span } => items.push(Item::Trivia(span.clone())),
                Node::Text { .. } | Node::CData { .. } => {
                    return Err(invalid(RelationshipsError::UnexpectedText));
                }
            }
        }
        Ok(Relationships {
            original: Some(Original {
                root_start: root.start_tag.clone(),
                root_end: root.end_tag.clone(),
                root_span_end: root.span.end,
                root_name: root.qualified_name,
                encoding,
                text,
            }),
            items,
            relationships,
            modified: false,
        })
    }

    /// The relationships, in document order.
    pub fn iter(&self) -> impl Iterator<Item = &Relationship> {
        self.items.iter().filter_map(|item| match item {
            Item::Relationship(id) => self.get(id),
            Item::Trivia(_) | Item::Extension(_) => None,
        })
    }

    /// The number of relationships.
    pub fn len(&self) -> usize {
        self.relationships.len()
    }

    /// Whether there are no relationships.
    pub fn is_empty(&self) -> bool {
        self.relationships.is_empty()
    }

    /// The relationship with the identifier `id` (compared exactly, as XML identifiers are).
    pub fn get(&self, id: &str) -> Option<&Relationship> {
        self.relationships.get(id).map(|entry| &entry.relationship)
    }

    /// The relationships of the type `relationship_type` (compared exactly, as §6.5.3.4 requires).
    pub fn of_type<'a>(
        &'a self,
        relationship_type: &'a str,
    ) -> impl Iterator<Item = &'a Relationship> {
        self.iter()
            .filter(move |relationship| relationship.relationship_type == relationship_type)
    }

    /// An identifier that no relationship has yet: `rId` followed by one more than the highest number among identifiers of that form (`rId1` if there is none), so that the same relationships always get the same next identifier.
    pub fn next_id(&self) -> String {
        let highest = self
            .relationships
            .keys()
            .filter_map(|id| id.strip_prefix("rId"))
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .filter_map(|digits| digits.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        // Only when the highest number is u64::MAX is there no next one; then count up from 1 to the first free one.
        let mut number = highest.checked_add(1).unwrap_or(1);
        while self.relationships.contains_key(&format!("rId{number}")) {
            number = number.saturating_add(1);
        }
        format!("rId{number}")
    }

    /// Adds a relationship after the existing ones.
    ///
    /// # Errors
    ///
    /// [`PackageError::Relationships`] with [`RelationshipsError::MissingAttribute`] if its identifier or type is empty, or with [`RelationshipsError::DuplicateId`] if a relationship with its identifier exists.
    pub fn add(&mut self, relationship: Relationship) -> Result<(), Error> {
        let invalid = |error| Error::Package(PackageError::Relationships { entry: None, error });
        if relationship.id.is_empty() || relationship.relationship_type.is_empty() {
            return Err(invalid(RelationshipsError::MissingAttribute));
        }
        if self.relationships.contains_key(&relationship.id) {
            return Err(invalid(RelationshipsError::DuplicateId));
        }
        let index = self
            .items
            .iter()
            .rposition(|item| !matches!(item, Item::Trivia(_)))
            .map_or(0, |index| index + 1);
        let indent = self.indent_before_relationships();
        self.items
            .insert(index, Item::Relationship(relationship.id.clone()));
        self.relationships.insert(
            relationship.id.clone(),
            Entry {
                relationship,
                raw: None,
                indent,
            },
        );
        self.modified = true;
        Ok(())
    }

    /// Removes the relationship with the identifier `id`, with the white space before it, and returns it.
    pub fn remove(&mut self, id: &str) -> Option<Relationship> {
        let index = self
            .items
            .iter()
            .position(|item| matches!(item, Item::Relationship(other) if other == id))?;
        let entry = self.relationships.remove(id)?;
        self.items.remove(index);
        if let Some(previous) = index.checked_sub(1)
            && let Some(Item::Trivia(span)) = self.items.get(previous)
            && self
                .original
                .as_ref()
                .and_then(|original| original.text.get(span.clone()))
                .is_some_and(|text| text.chars().all(xml::is_space))
        {
            self.items.remove(previous);
        }
        self.modified = true;
        Some(entry.relationship)
    }

    /// Whether the relationships differ from those they were read from (always true for new relationships).
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// The white space before the first original relationship, if the original is indented.
    fn indent_before_relationships(&self) -> String {
        let Some(original) = &self.original else {
            return String::new();
        };
        let mut previous: Option<&Range<usize>> = None;
        for item in &self.items {
            match item {
                Item::Trivia(span) => previous = Some(span),
                Item::Relationship(id)
                    if self
                        .relationships
                        .get(id)
                        .is_some_and(|entry| entry.raw.is_some()) =>
                {
                    return previous
                        .and_then(|span| original.text.get(span.clone()))
                        .filter(|text| text.chars().all(xml::is_space))
                        .map(str::to_owned)
                        .unwrap_or_default();
                }
                Item::Relationship(_) | Item::Extension(_) => previous = None,
            }
        }
        String::new()
    }

    /// The relationships part as XML. Unchanged, it is exactly the text it was read from, in its original encoding; changed, only the new relationships are written anew.
    pub fn to_xml(&self) -> Vec<u8> {
        let Some(original) = &self.original else {
            let mut text = String::from(DECLARATION);
            text.push_str("<Relationships xmlns=\"");
            text.push_str(NAMESPACE);
            text.push_str("\">");
            self.write_items(&mut text, None, "");
            text.push_str("</Relationships>");
            return text.into_bytes();
        };
        let source = &original.text;
        let prefix = original
            .root_name
            .split_once(':')
            .map_or("", |(prefix, _)| prefix);
        let mut text = String::with_capacity(source.len());
        text.push_str(source.get(..original.root_start.start).unwrap_or_default());
        let start_tag = source.get(original.root_start.clone()).unwrap_or_default();
        match &original.root_end {
            Some(end_tag) => {
                text.push_str(start_tag);
                self.write_items(&mut text, Some(source), prefix);
                text.push_str(source.get(end_tag.clone()).unwrap_or_default());
            }
            None if self.items.is_empty() => text.push_str(start_tag),
            None => {
                let open = start_tag.strip_suffix("/>").unwrap_or(start_tag);
                text.push_str(open);
                text.push('>');
                self.write_items(&mut text, Some(source), prefix);
                text.push_str("</");
                text.push_str(&original.root_name);
                text.push('>');
            }
        }
        text.push_str(source.get(original.root_span_end..).unwrap_or_default());
        xml::encode(&text, original.encoding)
    }

    fn write_items(&self, text: &mut String, source: Option<&str>, prefix: &str) {
        let original = |span: &Range<usize>| {
            source
                .and_then(|source| source.get(span.clone()))
                .unwrap_or_default()
        };
        for item in &self.items {
            let entry = match item {
                Item::Trivia(span) | Item::Extension(span) => {
                    text.push_str(original(span));
                    continue;
                }
                Item::Relationship(id) => match self.relationships.get(id) {
                    Some(entry) => entry,
                    None => continue,
                },
            };
            if let (Some(span), true) = (&entry.raw, source.is_some()) {
                text.push_str(original(span));
                continue;
            }
            // As Word writes it: Id, Type, Target, then TargetMode for external targets only.
            let relationship = &entry.relationship;
            text.push_str(&entry.indent);
            text.push('<');
            if !prefix.is_empty() {
                text.push_str(prefix);
                text.push(':');
            }
            text.push_str("Relationship Id=\"");
            xml::escape_attribute(&relationship.id, text);
            text.push_str("\" Type=\"");
            xml::escape_attribute(&relationship.relationship_type, text);
            text.push_str("\" Target=\"");
            xml::escape_attribute(&relationship.target, text);
            if relationship.target_mode == TargetMode::External {
                text.push_str("\" TargetMode=\"External");
            }
            text.push_str("\"/>");
        }
    }
}

/// Reads one `Relationship` element. Attributes it does not know, and its text content, stay in the original text.
fn read_relationship(element: &Element) -> Result<Relationship, RelationshipsError> {
    let nested = element.children.iter().any(|node| {
        matches!(node, Node::Element(child) if child.namespace.as_deref() == Some(NAMESPACE))
    });
    if nested {
        return Err(RelationshipsError::UnexpectedElement);
    }
    let required = |name| {
        element
            .attribute(name)
            .filter(|value| !value.is_empty())
            .ok_or(RelationshipsError::MissingAttribute)
    };
    let id = required("Id")?;
    let relationship_type = required("Type")?;
    let target = element
        .attribute("Target")
        .ok_or(RelationshipsError::MissingAttribute)?;
    let target_mode = match element.attribute("TargetMode") {
        None | Some("Internal") => TargetMode::Internal,
        Some("External") => TargetMode::External,
        Some(_) => return Err(RelationshipsError::InvalidTargetMode),
    };
    Ok(Relationship::new(
        id,
        relationship_type,
        target,
        target_mode,
    ))
}

/// Whether any element of the tree under `root`, `root` included, has an `xml:base` attribute.
fn uses_xml_base(root: &Element) -> bool {
    let mut pending = vec![root];
    while let Some(element) = pending.pop() {
        if element.attributes.iter().any(|attribute| {
            attribute.local_name == "base" && attribute.namespace.as_deref() == Some(XML_NAMESPACE)
        }) {
            return true;
        }
        pending.extend(element.children.iter().filter_map(|node| match node {
            Node::Element(child) => Some(child),
            _ => None,
        }));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties\" Target=\"docProps/app.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties\" Target=\"docProps/core.xml\"/><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";

    const HYPERLINK: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";

    fn parse(text: &str) -> Result<Relationships, Error> {
        Relationships::parse(text.as_bytes(), &Limits::default())
    }

    fn error(text: &str) -> RelationshipsError {
        match parse(text).unwrap_err() {
            Error::Package(PackageError::Relationships { error, .. }) => error,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reads_relationships_in_order() {
        let relationships = parse(WORD).unwrap();
        let ids: Vec<&str> = relationships.iter().map(Relationship::id).collect();
        assert_eq!(ids, ["rId3", "rId2", "rId1"]);
        let document = relationships.get("rId1").unwrap();
        assert_eq!(document.target(), "word/document.xml");
        assert_eq!(document.target_mode(), TargetMode::Internal);
        assert_eq!(
            document.resolve(&RelationshipSource::Package).unwrap(),
            Target::Part(PartName::new("/word/document.xml").unwrap())
        );
        assert_eq!(relationships.of_type("http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties").count(), 1);
        assert_eq!(relationships.len(), 3);
        assert_eq!(relationships.next_id(), "rId4");
    }

    #[test]
    fn records_external_targets_without_resolving_them() {
        let text = format!(
            "<Relationships xmlns=\"{NAMESPACE}\"><Relationship Id=\"rId9\" Type=\"{HYPERLINK}\" Target=\"https://example.com/?q=1#x\" TargetMode=\"External\"/><Relationship Id=\"rId8\" Type=\"t\" Target=\"\\\\server\\share\\x.dotm\" TargetMode=\"External\"/></Relationships>"
        );
        let relationships = parse(&text).unwrap();
        let link = relationships.get("rId9").unwrap();
        assert_eq!(link.target_mode(), TargetMode::External);
        assert_eq!(
            link.resolve(&RelationshipSource::Package).unwrap(),
            Target::External("https://example.com/?q=1#x".to_owned())
        );
        assert_eq!(
            relationships
                .get("rId8")
                .unwrap()
                .resolve(&RelationshipSource::Package)
                .unwrap(),
            Target::External("\\\\server\\share\\x.dotm".to_owned())
        );
    }

    #[test]
    fn keeps_extensions_unknown_attributes_and_formatting() {
        let text = format!(
            "<?xml version='1.0'?>\n<r:Relationships xmlns:r=\"{NAMESPACE}\" xmlns:x=\"urn:x\" xmlns:mc=\"http://schemas.openxmlformats.org/markup-compatibility/2006\" mc:Ignorable=\"x\">\n\t<r:Relationship Id='a' Type='t' Target='b.xml' x:note='kept'>text</r:Relationship>\n\t<x:extension><x:inner/></x:extension>\n\t<!-- comment -->\n</r:Relationships>\n"
        );
        let mut relationships = parse(&text).unwrap();
        assert_eq!(relationships.to_xml(), text.as_bytes());
        relationships
            .add(Relationship::new(
                "rId1",
                HYPERLINK,
                "https://example.com/",
                TargetMode::External,
            ))
            .unwrap();
        // A new relationship goes after the last element, indented like the others.
        let written = String::from_utf8(relationships.to_xml()).unwrap();
        assert_eq!(written, text.replace(
            "<x:extension><x:inner/></x:extension>",
            "<x:extension><x:inner/></x:extension>\n\t<r:Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://example.com/\" TargetMode=\"External\"/>",
        ));
        assert!(relationships.remove("rId1").is_some());
        assert_eq!(relationships.to_xml(), text.as_bytes());
    }

    #[test]
    fn writes_new_relationships_like_word() {
        let mut relationships = Relationships::new();
        relationships.add(Relationship::new("rId1", "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument", "word/document.xml", TargetMode::Internal)).unwrap();
        relationships
            .add(Relationship::new(
                relationships.next_id(),
                HYPERLINK,
                "https://example.com/a?b=c&d=\"e\"",
                TargetMode::External,
            ))
            .unwrap();
        let text = String::from_utf8(relationships.to_xml()).unwrap();
        assert_eq!(
            text,
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://example.com/a?b=c&amp;d=&quot;e&quot;\" TargetMode=\"External\"/></Relationships>"
        );
        let read = parse(&text).unwrap();
        assert_eq!(
            read.get("rId2").unwrap().target(),
            "https://example.com/a?b=c&d=\"e\""
        );
        assert!(
            relationships
                .add(Relationship::new("rId1", "t", "x", TargetMode::Internal))
                .is_err()
        );
        assert!(
            relationships
                .add(Relationship::new("", "t", "x", TargetMode::Internal))
                .is_err()
        );
    }

    #[test]
    fn refuses_parts_that_break_the_rules() {
        let wrap =
            |body: &str| format!("<Relationships xmlns=\"{NAMESPACE}\">{body}</Relationships>");
        assert_eq!(
            error("<Relationships/>"),
            RelationshipsError::UnexpectedRoot
        );
        assert_eq!(
            error(&wrap("<Other/>")),
            RelationshipsError::UnexpectedElement
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Id='a' Type='t' Target='x'><Relationship Id='b' Type='t' Target='y'/></Relationship>"
            )),
            RelationshipsError::UnexpectedElement
        );
        assert_eq!(error(&wrap("text")), RelationshipsError::UnexpectedText);
        assert_eq!(
            error(&wrap("<Relationship Type='t' Target='x'/>")),
            RelationshipsError::MissingAttribute
        );
        assert_eq!(
            error(&wrap("<Relationship Id='' Type='t' Target='x'/>")),
            RelationshipsError::MissingAttribute
        );
        assert_eq!(
            error(&wrap("<Relationship Id='a' Target='x'/>")),
            RelationshipsError::MissingAttribute
        );
        assert_eq!(
            error(&wrap("<Relationship Id='a' Type='t'/>")),
            RelationshipsError::MissingAttribute
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Id='a' Type='t' Target='x' TargetMode='external'/>"
            )),
            RelationshipsError::InvalidTargetMode
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Id='a' Type='t' Target='x'/><Relationship Id='a' Type='t' Target='y'/>"
            )),
            RelationshipsError::DuplicateId
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Id='a' Type='t' Target='x' xml:base='http://evil/'/>"
            )),
            RelationshipsError::XmlBase
        );
        assert_eq!(
            error(&format!(
                "<Relationships xmlns=\"{NAMESPACE}\" xml:base='http://evil/'/>"
            )),
            RelationshipsError::XmlBase
        );
        assert_eq!(
            error(&wrap("<x:e xmlns:x='urn:x'><x:f xml:base='/'/></x:e>")),
            RelationshipsError::XmlBase
        );
        // Different case is a different identifier.
        assert!(parse(&wrap("<Relationship Id='a' Type='t' Target='x'/><Relationship Id='A' Type='t' Target='y'/>")).is_ok());
    }

    #[test]
    fn counts_identifiers_up() {
        let mut relationships = Relationships::new();
        assert_eq!(relationships.next_id(), "rId1");
        for id in ["rId7", "rIdx", "R3", "rId0012"] {
            relationships
                .add(Relationship::new(id, "t", "x", TargetMode::Internal))
                .unwrap();
        }
        assert_eq!(relationships.next_id(), "rId13");
        relationships
            .add(Relationship::new(
                format!("rId{}", u64::MAX),
                "t",
                "x",
                TargetMode::Internal,
            ))
            .unwrap();
        relationships
            .add(Relationship::new("rId1", "t", "x", TargetMode::Internal))
            .unwrap();
        assert_eq!(relationships.next_id(), "rId2");
    }
}
