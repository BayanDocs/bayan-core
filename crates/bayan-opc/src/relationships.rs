//! Relationships parts, which connect the package and its parts to other parts and to external resources (ECMA-376 Part 2 §6.5).

use std::collections::BTreeMap;
use std::ops::Range;

use bayan_xml::{Event, StartTag};

use crate::content_types::DECLARATION;
// The XML of the metadata parts, read with bayan-xml (`crate::metadata_xml`).
use crate::metadata_xml::{self as xml, Encoding, XML_NAMESPACE};
use crate::part_name::{PartName, RelationshipSource};
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
    /// The highest number among the identifiers `rId` followed by decimal digits, 0 if there is none, so that [`Relationships::next_id`] takes constant time.
    highest: u64,
    /// A number below which every identifier `rId1`, `rId2`, … is taken, so that [`Relationships::next_id`] finds the first free one quickly when the highest number is `u64::MAX`.
    free_from: u64,
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
            highest: 0,
            free_from: 1,
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
    ///
    /// The part is read event by event, and only the relationships, the spans of the elements from other namespaces and the text between them are kept. Its rules are checked in this order, as when the whole part was parsed before it was read: the root element; then an `xml:base` attribute anywhere in the part; then the relationships, the elements from other namespaces and the text between them, in the order of the document, and within a relationship its content before its attributes. The first rule broken is the error, unless the part is not acceptable XML at all, which comes first.
    pub(crate) fn parse_entry(
        bytes: &[u8],
        limits: &Limits,
        entry: Option<usize>,
        nodes: &mut usize,
    ) -> Result<Self, Error> {
        let invalid = |error| Error::Package(PackageError::Relationships { entry, error });
        let mut reading = Reading {
            items: Vec::new(),
            relationships: BTreeMap::new(),
            highest: 0,
            root: None,
            root_error: None,
            xml_base: false,
            broken: None,
            open: None,
        };
        let document = xml::read(bytes, limits, entry, nodes, |event, depth| {
            reading.event(event, depth, limits.max_xml_name_length);
            Ok(())
        })?;
        let Reading {
            mut items,
            relationships,
            highest,
            root,
            root_error,
            xml_base,
            broken,
            ..
        } = reading;
        if let Some(error) = root_error {
            return Err(invalid(error));
        }
        if xml_base {
            return Err(invalid(RelationshipsError::XmlBase));
        }
        if let Some(error) = broken {
            return Err(invalid(error));
        }
        // The root element is the first event of every document that bayan-xml reads without an error.
        let Some(root) = root else {
            return Err(invalid(RelationshipsError::UnexpectedRoot));
        };
        items.shrink_to_fit();
        let mut relationships = Relationships {
            original: Some(Original {
                root_start: root.start_tag,
                root_end: root.end_tag,
                root_span_end: root.span_end,
                root_name: root.name,
                encoding: document.encoding,
                text: document.text,
            }),
            items,
            relationships,
            highest,
            free_from: 1,
            modified: false,
        };
        relationships.skip_taken_numbers();
        Ok(relationships)
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

    /// An identifier that no relationship has yet: `rId` followed by one more than the highest number among identifiers of that form (`rId1` if there is none), so that the same relationships always get the same next identifier. When the highest number is `u64::MAX`, which has no next one, it is the first free number counting up from 1. It takes constant time, so adding relationships one after another, each with the next identifier, takes time in proportion to their number.
    pub fn next_id(&self) -> String {
        let number = match self.highest.checked_add(1) {
            Some(next) => next,
            None => {
                let mut number = self.free_from;
                while self.relationships.contains_key(&format!("rId{number}")) {
                    number = number.saturating_add(1);
                }
                number
            }
        };
        format!("rId{number}")
    }

    /// Adds a relationship after the existing ones.
    ///
    /// # Errors
    ///
    /// [`PackageError::Relationships`] with [`RelationshipsError::MissingAttribute`] if its identifier or type is empty, with [`RelationshipsError::InvalidId`] if its identifier is not an XML identifier (a name without a colon, as `xsd:ID` requires), with [`RelationshipsError::InvalidCharacter`] if its type or target has a character that XML cannot hold (a control character other than tab, line feed and carriage return, U+FFFE or U+FFFF), so that it could not be written, or with [`RelationshipsError::DuplicateId`] if a relationship with its identifier exists. Relationships that are read are not held to the first two rules, so that documents which break them still open.
    pub fn add(&mut self, relationship: Relationship) -> Result<(), Error> {
        let invalid = |error| Error::Package(PackageError::Relationships { entry: None, error });
        if relationship.id.is_empty() || relationship.relationship_type.is_empty() {
            return Err(invalid(RelationshipsError::MissingAttribute));
        }
        if !xml::is_ncname(&relationship.id) {
            return Err(invalid(RelationshipsError::InvalidId));
        }
        let writable = |text: &str| text.chars().all(xml::is_xml_character);
        if !writable(&relationship.relationship_type) || !writable(&relationship.target) {
            return Err(invalid(RelationshipsError::InvalidCharacter));
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
        self.highest = self.highest.max(id_number(&relationship.id).unwrap_or(0));
        self.relationships.insert(
            relationship.id.clone(),
            Entry {
                relationship,
                raw: None,
                indent,
            },
        );
        self.skip_taken_numbers();
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
        let number = id_number(id);
        if number.is_some_and(|number| number == self.highest) {
            // The highest identifier is gone: find the new highest, so that the next identifier depends only on the identifiers there are (removing takes linear time anyway).
            self.highest = self
                .relationships
                .keys()
                .filter_map(|id| id_number(id))
                .max()
                .unwrap_or(0);
        }
        if let Some(number) = number
            && number < self.free_from
            && id == format!("rId{number}")
        {
            self.free_from = number;
        }
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
    /// Moves `free_from` past the identifiers `rId1`, `rId2`, … that are taken.
    fn skip_taken_numbers(&mut self) {
        while self
            .relationships
            .contains_key(&format!("rId{}", self.free_from))
        {
            self.free_from = self.free_from.saturating_add(1);
        }
    }

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
/// The number of an identifier `rId` followed by decimal digits (leading zeros included), if it has that form and the number fits in a `u64`.
fn id_number(id: &str) -> Option<u64> {
    let digits = id.strip_prefix("rId")?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// What reading a relationships part has found so far. Its rules are checked in their order of precedence at the end ([`Relationships::parse_entry`]), so reading notes the first break of each, and stops keeping relationships after one.
struct Reading {
    items: Vec<Item>,
    relationships: BTreeMap<String, Entry>,
    highest: u64,
    root: Option<Root>,
    /// The root element is not `Relationships` in the relationships namespace.
    root_error: Option<RelationshipsError>,
    /// An element of the part has an `xml:base` attribute.
    xml_base: bool,
    /// The first rule that a relationship, an element from another namespace or the text between them breaks.
    broken: Option<RelationshipsError>,
    /// The `Relationship` element being read.
    open: Option<OpenRelationship>,
}

/// Where the root element is, and its name as written.
struct Root {
    start_tag: Range<usize>,
    /// `None` while it is open, and for an empty-element tag.
    end_tag: Option<Range<usize>>,
    /// Where the root element ends, and what follows it begins.
    span_end: usize,
    name: String,
}

/// A `Relationship` element whose end has not been read yet.
struct OpenRelationship {
    /// What its start tag gives.
    relationship: Result<Relationship, RelationshipsError>,
    /// Whether one of its children is an element in the relationships namespace, which comes before what its attributes break.
    nested: bool,
}

impl Reading {
    /// Reads one event of the part at `depth` (see [`xml::read`]).
    fn event(&mut self, event: &Event<'_>, depth: usize, max_id_length: usize) {
        if let Event::Start(tag) = event
            && tag.attributes().any(|attribute| {
                let name = attribute.name();
                name.local() == "base" && xml::namespace_of(&name) == Some(XML_NAMESPACE)
            })
        {
            self.xml_base = true;
        }
        // After the first broken rule, only `xml:base` still matters.
        if self.root_error.is_some() || self.broken.is_some() {
            return;
        }
        match (event, depth) {
            (Event::Start(tag), 1) => {
                let name = tag.name();
                if !xml::is(&name, NAMESPACE, "Relationships") {
                    self.root_error = Some(RelationshipsError::UnexpectedRoot);
                    return;
                }
                self.root = Some(Root {
                    start_tag: tag.span(),
                    end_tag: None,
                    span_end: tag.span().end,
                    name: xml::qualified_name(&name),
                });
            }
            (Event::Start(tag), 2) => {
                let name = tag.name();
                if xml::is(&name, NAMESPACE, "Relationship") {
                    self.open = Some(OpenRelationship {
                        relationship: read_relationship(tag, max_id_length),
                        nested: false,
                    });
                } else if xml::namespace_of(&name) == Some(NAMESPACE) {
                    self.broken = Some(RelationshipsError::UnexpectedElement);
                }
            }
            // A child of a relationship: one in the relationships namespace is not allowed; any other is kept with it, as written.
            (Event::Start(tag), 3) => {
                if let Some(open) = &mut self.open
                    && xml::namespace_of(&tag.name()) == Some(NAMESPACE)
                {
                    open.nested = true;
                }
            }
            (Event::End(end), 2) => match self.open.take() {
                Some(OpenRelationship { nested: true, .. }) => {
                    self.broken = Some(RelationshipsError::UnexpectedElement);
                }
                Some(OpenRelationship {
                    relationship: Err(error),
                    ..
                }) => self.broken = Some(error),
                Some(OpenRelationship {
                    relationship: Ok(relationship),
                    ..
                }) => {
                    if self.relationships.contains_key(&relationship.id) {
                        self.broken = Some(RelationshipsError::DuplicateId);
                        return;
                    }
                    self.items.push(Item::Relationship(relationship.id.clone()));
                    self.highest = self.highest.max(id_number(&relationship.id).unwrap_or(0));
                    self.relationships.insert(
                        relationship.id.clone(),
                        Entry {
                            relationship,
                            raw: Some(end.element_span()),
                            indent: String::new(),
                        },
                    );
                }
                // An element from another namespace, kept as written and not interpreted.
                None => self.items.push(Item::Extension(end.element_span())),
            },
            (Event::End(end), 1) => {
                if let Some(root) = &mut self.root {
                    if !end.span().is_empty() {
                        root.end_tag = Some(end.span());
                    }
                    root.span_end = end.span().end;
                }
            }
            // White space, comments and processing instructions between relationships are kept as written; other text is not allowed there.
            (Event::Text(text), 1) => {
                if text.value().chars().all(xml::is_space) {
                    self.items.push(Item::Trivia(text.span()));
                } else {
                    self.broken = Some(RelationshipsError::UnexpectedText);
                }
            }
            (Event::CData(_), 1) => self.broken = Some(RelationshipsError::UnexpectedText),
            (Event::Comment(comment), 1) => self.items.push(Item::Trivia(comment.span())),
            (Event::ProcessingInstruction(instruction), 1) => {
                self.items.push(Item::Trivia(instruction.span()));
            }
            _ => {}
        }
    }
}

/// Reads the attributes of a `Relationship` start tag. Attributes it does not know, and its content, stay in the original text. Identifiers are XML names (`xsd:ID`), so one longer than `max_id_length` bytes, the limit on XML names, is refused: each is kept several times.
fn read_relationship(
    tag: &StartTag<'_>,
    max_id_length: usize,
) -> Result<Relationship, RelationshipsError> {
    let attribute = |local| tag.attribute(None, local).map(|value| value.value());
    let required = |local| {
        attribute(local)
            .filter(|value| !value.is_empty())
            .ok_or(RelationshipsError::MissingAttribute)
    };
    let id = required("Id")?;
    if id.len() > max_id_length {
        return Err(RelationshipsError::IdTooLong);
    }
    let relationship_type = required("Type")?;
    let target = attribute("Target").ok_or(RelationshipsError::MissingAttribute)?;
    let target_mode = match attribute("TargetMode") {
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
        let invalid = |error| {
            Err(Error::Package(PackageError::Relationships {
                entry: None,
                error,
            }))
        };
        assert_eq!(
            relationships.add(Relationship::new("rId1", "t", "x", TargetMode::Internal)),
            invalid(RelationshipsError::DuplicateId)
        );
        assert_eq!(
            relationships.add(Relationship::new("", "t", "x", TargetMode::Internal)),
            invalid(RelationshipsError::MissingAttribute)
        );
    }

    #[test]
    fn refuses_identifiers_longer_than_xml_names() {
        // Identifiers are XML names, and each is kept several times, so one longer than the limit on XML names is refused (the review of pull request 17).
        let limit = Limits::default().max_xml_name_length;
        let part = |length: usize| {
            format!(
                "<Relationships xmlns=\"{NAMESPACE}\"><Relationship Id=\"{}\" Type=\"t\" Target=\"x\"/></Relationships>",
                "r".repeat(length)
            )
        };
        assert!(parse(&part(limit)).is_ok());
        assert_eq!(error(&part(limit + 1)), RelationshipsError::IdTooLong);
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
    fn checks_the_rules_in_their_order() {
        // The part is read event by event, and its rules are checked in the order they always were: the root element, then `xml:base` anywhere, then the relationships and what is between them in the order of the part, a relationship's content before its attributes; the first rule broken is the error, unless the part is not XML, which comes first.
        let wrap =
            |body: &str| format!("<Relationships xmlns=\"{NAMESPACE}\">{body}</Relationships>");
        assert_eq!(
            error("<Other xml:base='http://evil/'/>"),
            RelationshipsError::UnexpectedRoot
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Type='t' Target='x'/><x:e xmlns:x='urn:x'><x:f xml:base='/'/></x:e>"
            )),
            RelationshipsError::XmlBase
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Type='t' Target='x'><Relationship Id='b' Type='t' Target='y'/></Relationship>"
            )),
            RelationshipsError::UnexpectedElement
        );
        assert_eq!(
            error(&wrap(
                "<Relationship Id='a' Type='t' Target='x'/><Relationship Id='a' Type='t' Target='y'/>text"
            )),
            RelationshipsError::DuplicateId
        );
        assert_eq!(
            error(&wrap("text<Relationship Type='t' Target='x'/>")),
            RelationshipsError::UnexpectedText
        );
        // Only a relationship's own children may not be relationships; deeper, and inside elements from other namespaces, they are kept as written.
        let kept = wrap(
            "<Relationship Id='a' Type='t' Target='x'><x:e xmlns:x='urn:x'><Relationship/></x:e></Relationship><x:f xmlns:x='urn:x'><Relationship/></x:f>",
        );
        let relationships = parse(&kept).unwrap();
        assert_eq!(relationships.len(), 1);
        assert_eq!(relationships.to_xml(), kept.as_bytes());
        assert!(matches!(
            parse(&format!("<Other xmlns=\"{NAMESPACE}\"><x></Other>")),
            Err(Error::Xml {
                error: crate::XmlError {
                    kind: crate::XmlErrorKind::MismatchedEndTag,
                    ..
                },
                ..
            })
        ));
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
        // The first free number is found again after every addition and removal; `rId0012` does not take `rId12`.
        let add = |relationships: &mut Relationships, id: &str| {
            relationships
                .add(Relationship::new(id, "t", "x", TargetMode::Internal))
                .unwrap();
        };
        for id in ["rId2", "rId3", "rId4", "rId5", "rId6"] {
            add(&mut relationships, id);
        }
        assert_eq!(relationships.next_id(), "rId8");
        assert!(relationships.remove("rId3").is_some());
        assert_eq!(relationships.next_id(), "rId3");
        add(&mut relationships, "rId3");
        for id in ["rId8", "rId9", "rId10", "rId11"] {
            add(&mut relationships, id);
        }
        assert_eq!(relationships.next_id(), "rId12");
        // Without the largest number, the next identifier follows the highest one there is (12, from `rId0012`), as if the relationships had been read so.
        assert!(relationships.remove(&format!("rId{}", u64::MAX)).is_some());
        assert_eq!(relationships.next_id(), "rId13");
        assert!(relationships.remove("rId0012").is_some());
        assert_eq!(relationships.next_id(), "rId12");
        let read = Relationships::parse(&relationships.to_xml(), &Limits::default()).unwrap();
        assert_eq!(read.next_id(), "rId12");
        // Read relationships are counted the same way.
        let text = format!(
            "<Relationships xmlns='{NAMESPACE}'><Relationship Id='rId{}' Type='t' Target='x'/><Relationship Id='rId1' Type='t' Target='x'/><Relationship Id='rId2' Type='t' Target='x'/></Relationships>",
            u64::MAX
        );
        let mut read = Relationships::parse(text.as_bytes(), &Limits::default()).unwrap();
        assert_eq!(read.next_id(), "rId3");
        add(&mut read, "rId3");
        assert_eq!(read.next_id(), "rId4");
    }

    #[test]
    fn writes_only_what_it_reads_back() {
        // A relationship's type and target either read back as they were added, or are refused when it is added, because XML cannot hold the character.
        let invalid = |error| Error::Package(PackageError::Relationships { entry: None, error });
        let characters = (0..=0x2FF_u32)
            .chain(0xFFF0..=0xFFFF)
            .chain(0x1_0000..=0x1_000F)
            .filter_map(char::from_u32);
        for character in characters {
            let cases = [
                (format!("urn:{character}"), "t".to_owned()),
                ("urn:t".to_owned(), format!("a{character}b")),
            ];
            for (relationship_type, target) in cases {
                let mut relationships = Relationships::new();
                let added = relationships.add(Relationship::new(
                    "rId1",
                    relationship_type.as_str(),
                    target.as_str(),
                    TargetMode::External,
                ));
                match added {
                    Ok(()) => {
                        let read =
                            Relationships::parse(&relationships.to_xml(), &Limits::default())
                                .unwrap();
                        let relationship = read.get("rId1").unwrap();
                        assert_eq!(relationship.relationship_type(), relationship_type);
                        assert_eq!(relationship.target(), target);
                    }
                    Err(error) => {
                        assert_eq!(error, invalid(RelationshipsError::InvalidCharacter));
                        assert!(
                            !xml::is_xml_character(character),
                            "{:X}",
                            u32::from(character)
                        );
                    }
                }
            }
        }
        // An identifier must be an XML identifier, a name without a colon (`xsd:ID`, ECMA-376 Part 2 M1.26).
        for id in ["rId1", "_a", "a-b.c7", "é"] {
            assert!(
                Relationships::new()
                    .add(Relationship::new(id, "t", "x", TargetMode::Internal))
                    .is_ok(),
                "{id}"
            );
        }
        for id in ["1a", "a b", "a:b", "-a", "a\u{1}", ".a"] {
            assert_eq!(
                Relationships::new()
                    .add(Relationship::new(id, "t", "x", TargetMode::Internal))
                    .unwrap_err(),
                invalid(RelationshipsError::InvalidId),
                "{id}"
            );
        }
    }
}
