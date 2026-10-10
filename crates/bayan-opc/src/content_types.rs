//! The content types stream, `[Content_Types].xml`, which gives every part its media type (ECMA-376 Part 2 §7.2.3).

use std::collections::BTreeMap;
use std::ops::Range;

use crate::part_name::PartName;
use crate::percent;
use crate::xml::{self, Element, Encoding, Node};
use crate::{ContentTypesError, Error, Limits, PackageError};

/// The namespace of the content types stream.
pub const NAMESPACE: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// The name of the ZIP entry that holds the content types stream. It is not a part: the brackets make it an invalid part name on purpose (§7.3.7).
pub const ZIP_NAME: &str = "[Content_Types].xml";

/// The XML declaration that new metadata parts start with, as Word writes it.
pub(crate) const DECLARATION: &str =
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";

/// The content types stream: default media types by extension and media types for individual parts.
///
/// It keeps the document it was read from, so that writing it back without changes gives exactly the original text ([`ContentTypes::to_xml`]), and a change rewrites only the entries it touches: the rest, with its formatting, comments and order, is copied as it was (ADR-0018 rule 3).
///
/// The entries are kept in ordered maps by extension and by part name, so looking one up, or refusing a duplicate, takes logarithmic time however many entries a stream has.
#[derive(Clone, Debug)]
pub struct ContentTypes {
    original: Option<Original>,
    /// The entries, and the text between them, in document order.
    items: Vec<Item>,
    /// The defaults, by the key of their extension ([`extension_key`]).
    defaults: BTreeMap<String, Entry>,
    /// The overrides, by the key of their part name.
    overrides: BTreeMap<String, Entry>,
    modified: bool,
}

/// The document a stream was read from, and where its pieces are.
#[derive(Clone, Debug)]
struct Original {
    text: String,
    encoding: Encoding,
    /// The root's start tag (or empty-element tag).
    root_start: Range<usize>,
    /// The root's end tag; `None` if the root is an empty-element tag.
    root_end: Option<Range<usize>>,
    /// Where the root element ends, and what follows it begins.
    root_span_end: usize,
    /// The root's name as written, for an end tag that has to be added.
    root_name: String,
}

#[derive(Clone, Debug)]
enum Item {
    /// White space, comments or processing instructions between the entries of the original, kept as written.
    Trivia(Range<usize>),
    /// A `Default` or `Override` element, kept in the map of its kind.
    Entry(Key),
}

/// Where an entry is kept: its kind, and its key in the map of that kind.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Key {
    Default(String),
    Override(String),
}

#[derive(Clone, Debug)]
struct Entry {
    kind: Kind,
    content_type: String,
    /// The element in the original text, while it is unchanged.
    raw: Option<Range<usize>>,
    /// White space written before a new entry, copied from its neighbours.
    indent: String,
}

#[derive(Clone, Debug)]
enum Kind {
    Default { extension: String },
    Override { part_name: PartName },
}

impl Kind {
    fn key(&self) -> Key {
        match self {
            Kind::Default { extension } => Key::Default(extension_key(extension)),
            Kind::Override { part_name } => Key::Override(part_name.key().to_owned()),
        }
    }

    /// The extension or part name as written.
    fn spelling(&self) -> &str {
        match self {
            Kind::Default { extension } => extension,
            Kind::Override { part_name } => part_name.as_str(),
        }
    }
}

impl Default for ContentTypes {
    fn default() -> Self {
        ContentTypes::new()
    }
}

impl ContentTypes {
    /// An empty content types stream, for a new package.
    pub fn new() -> Self {
        ContentTypes {
            original: None,
            items: Vec::new(),
            defaults: BTreeMap::new(),
            overrides: BTreeMap::new(),
            modified: true,
        }
    }

    /// Reads a content types stream.
    ///
    /// # Errors
    ///
    /// [`Error::Limit`] if it is larger than [`Limits::max_metadata_size`], [`Error::Xml`] if it is not acceptable XML or has more than [`Limits::max_xml_nodes`] nodes, and [`PackageError::ContentTypes`] if it breaks a rule of §7.2.3: an unexpected element, attribute or text (the stream may not use extensions), a missing attribute, an invalid extension, media type or part name, or two entries for the same extension or part.
    pub fn parse(bytes: &[u8], limits: &Limits) -> Result<Self, Error> {
        let mut nodes = limits.max_xml_nodes;
        ContentTypes::parse_entry(bytes, limits, None, &mut nodes)
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
        let invalid = |error| Error::Package(PackageError::ContentTypes(error));
        if !root.is(NAMESPACE, "Types") {
            return Err(invalid(ContentTypesError::UnexpectedRoot));
        }
        if !root.attributes.is_empty() {
            return Err(invalid(ContentTypesError::UnexpectedAttribute));
        }
        let mut content_types = ContentTypes {
            original: None,
            items: Vec::with_capacity(root.children.len()),
            defaults: BTreeMap::new(),
            overrides: BTreeMap::new(),
            modified: false,
        };
        // Each child is dropped as soon as it has been read, so the tree's memory goes down while the model's goes up.
        for node in std::mem::take(&mut root.children) {
            match node {
                Node::Element(element) => {
                    let entry = read_entry(&element).map_err(invalid)?;
                    content_types.push_read(entry).map_err(invalid)?;
                }
                Node::Text { span, value } if value.chars().all(xml::is_space) => {
                    content_types.items.push(Item::Trivia(span.clone()));
                }
                Node::Markup { span } => content_types.items.push(Item::Trivia(span.clone())),
                Node::Text { .. } | Node::CData { .. } => {
                    return Err(invalid(ContentTypesError::UnexpectedText));
                }
            }
        }
        content_types.original = Some(Original {
            root_start: root.start_tag.clone(),
            root_end: root.end_tag.clone(),
            root_span_end: root.span.end,
            root_name: root.qualified_name,
            encoding,
            text,
        });
        Ok(content_types)
    }

    /// Adds an entry read from the stream after the others, refusing a second default for one extension and a second override for one part.
    fn push_read(&mut self, entry: Entry) -> Result<(), ContentTypesError> {
        let key = entry.kind.key();
        if self.get(&key).is_some() {
            return Err(match key {
                Key::Default(_) => ContentTypesError::DuplicateDefault,
                Key::Override(_) => ContentTypesError::DuplicateOverride,
            });
        }
        self.items.push(Item::Entry(key.clone()));
        self.insert(key, entry);
        Ok(())
    }

    fn get(&self, key: &Key) -> Option<&Entry> {
        match key {
            Key::Default(key) => self.defaults.get(key),
            Key::Override(key) => self.overrides.get(key),
        }
    }

    fn get_mut(&mut self, key: &Key) -> Option<&mut Entry> {
        match key {
            Key::Default(key) => self.defaults.get_mut(key),
            Key::Override(key) => self.overrides.get_mut(key),
        }
    }

    fn insert(&mut self, key: Key, entry: Entry) {
        match key {
            Key::Default(key) => self.defaults.insert(key, entry),
            Key::Override(key) => self.overrides.insert(key, entry),
        };
    }

    /// The entries, in document order.
    fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.items.iter().filter_map(|item| match item {
            Item::Entry(key) => self.get(key),
            Item::Trivia(_) => None,
        })
    }

    /// The media type of `part`: its override if there is one, otherwise the default for its extension (§7.2.3.5). Part names and extensions are compared without regard to the case of ASCII letters.
    pub fn content_type(&self, part: &PartName) -> Option<&str> {
        if let Some(entry) = self.overrides.get(part.key()) {
            return Some(entry.content_type.as_str());
        }
        let key = extension_key(part.extension()?);
        self.defaults
            .get(&key)
            .map(|entry| entry.content_type.as_str())
    }

    /// The default media types: pairs of an extension and a media type, in document order.
    pub fn defaults(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries().filter_map(|entry| match &entry.kind {
            Kind::Default { extension } => Some((extension.as_str(), entry.content_type.as_str())),
            Kind::Override { .. } => None,
        })
    }

    /// The overrides: pairs of a part name and a media type, in document order.
    pub fn overrides(&self) -> impl Iterator<Item = (&PartName, &str)> {
        self.entries().filter_map(|entry| match &entry.kind {
            Kind::Override { part_name } => Some((part_name, entry.content_type.as_str())),
            Kind::Default { .. } => None,
        })
    }

    /// Sets the default media type for `extension`, replacing an existing default for it in place.
    ///
    /// # Errors
    ///
    /// [`PackageError::ContentTypes`] with [`ContentTypesError::InvalidExtension`] or [`ContentTypesError::InvalidContentType`] if either is not valid.
    pub fn set_default(&mut self, extension: &str, content_type: &str) -> Result<(), Error> {
        let invalid = |error| Error::Package(PackageError::ContentTypes(error));
        if !is_extension(extension) {
            return Err(invalid(ContentTypesError::InvalidExtension));
        }
        if !is_media_type(content_type) {
            return Err(invalid(ContentTypesError::InvalidContentType));
        }
        self.put(
            Kind::Default {
                extension: extension.to_owned(),
            },
            content_type,
        );
        Ok(())
    }

    /// Removes the default media type for `extension`. Returns whether there was one.
    pub fn remove_default(&mut self, extension: &str) -> bool {
        self.remove(&Key::Default(extension_key(extension)))
    }

    /// Sets the media type of `part`, replacing an existing override for it in place.
    ///
    /// # Errors
    ///
    /// [`PackageError::ContentTypes`] with [`ContentTypesError::InvalidContentType`] if `content_type` is not a valid media type.
    pub fn set_override(&mut self, part: &PartName, content_type: &str) -> Result<(), Error> {
        if !is_media_type(content_type) {
            return Err(Error::Package(PackageError::ContentTypes(
                ContentTypesError::InvalidContentType,
            )));
        }
        self.put(
            Kind::Override {
                part_name: part.clone(),
            },
            content_type,
        );
        Ok(())
    }

    /// Removes the override for `part`. Returns whether there was one.
    pub fn remove_override(&mut self, part: &PartName) -> bool {
        self.remove(&Key::Override(part.key().to_owned()))
    }

    /// Makes sure that `part` has the media type `content_type`, the way §7.2.3.4 describes it: a part without an extension gets an override; a part whose extension has a default with another media type gets an override; an extension without a default gets one. Media types are compared without regard to case. An existing override for the part is updated.
    ///
    /// # Errors
    ///
    /// As for [`ContentTypes::set_override`].
    pub fn register(&mut self, part: &PartName, content_type: &str) -> Result<(), Error> {
        if !is_media_type(content_type) {
            return Err(Error::Package(PackageError::ContentTypes(
                ContentTypesError::InvalidContentType,
            )));
        }
        let has_override = self.overrides.contains_key(part.key());
        let extension = part.extension().filter(|extension| is_extension(extension));
        let Some(extension) = extension else {
            return self.set_override(part, content_type);
        };
        if has_override {
            if self
                .content_type(part)
                .is_some_and(|current| current.eq_ignore_ascii_case(content_type))
            {
                return Ok(());
            }
            return self.set_override(part, content_type);
        }
        let default = self
            .defaults
            .get(&extension_key(extension))
            .map(|entry| entry.content_type.clone());
        match default {
            Some(media) if media.eq_ignore_ascii_case(content_type) => Ok(()),
            Some(_) => self.set_override(part, content_type),
            None => self.set_default(extension, content_type),
        }
    }

    /// Whether the stream differs from the one it was read from (always true for a new stream).
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Replaces the entry for the same extension or part in place, or adds a new one: a default after the last default, an override at the end.
    fn put(&mut self, kind: Kind, content_type: &str) {
        let key = kind.key();
        if let Some(entry) = self.get_mut(&key) {
            let same =
                entry.content_type == content_type && entry.kind.spelling() == kind.spelling();
            if !same {
                entry.kind = kind;
                entry.content_type = content_type.to_owned();
                entry.raw = None;
                self.modified = true;
            }
            return;
        }
        // A new default goes after the last default, a new override after the last entry, so that both stay inside the original's indentation.
        let last_entry = self
            .items
            .iter()
            .rposition(|item| matches!(item, Item::Entry(_)));
        let after = match key {
            Key::Default(_) => self
                .items
                .iter()
                .rposition(|item| matches!(item, Item::Entry(Key::Default(_))))
                .or(last_entry),
            Key::Override(_) => last_entry,
        };
        let index = after.map_or(0, |index| index + 1);
        let indent = self.indent_before_entries();
        self.items.insert(index, Item::Entry(key.clone()));
        self.insert(
            key,
            Entry {
                kind,
                content_type: content_type.to_owned(),
                raw: None,
                indent,
            },
        );
        self.modified = true;
    }

    /// The white space that precedes the original entries, if they are indented, so that a new entry lines up with them.
    fn indent_before_entries(&self) -> String {
        let Some(original) = &self.original else {
            return String::new();
        };
        let mut previous: Option<&Range<usize>> = None;
        for item in &self.items {
            match item {
                Item::Trivia(span) => previous = Some(span),
                Item::Entry(key) if self.get(key).is_some_and(|entry| entry.raw.is_some()) => {
                    return previous
                        .and_then(|span| original.text.get(span.clone()))
                        .filter(|text| text.chars().all(xml::is_space))
                        .map(str::to_owned)
                        .unwrap_or_default();
                }
                Item::Entry(_) => previous = None,
            }
        }
        String::new()
    }

    /// Removes the entry kept under `key`, with the white space before it. Returns whether there was one.
    fn remove(&mut self, key: &Key) -> bool {
        let Some(index) = self
            .items
            .iter()
            .position(|item| matches!(item, Item::Entry(other) if other == key))
        else {
            return false;
        };
        self.items.remove(index);
        match key {
            Key::Default(key) => self.defaults.remove(key),
            Key::Override(key) => self.overrides.remove(key),
        };
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
        true
    }

    /// The stream as XML. Unchanged, it is exactly the text it was read from, in its original encoding; changed, only the changed entries are written anew.
    pub fn to_xml(&self) -> Vec<u8> {
        let Some(original) = &self.original else {
            let mut text = String::from(DECLARATION);
            text.push_str("<Types xmlns=\"");
            text.push_str(NAMESPACE);
            text.push_str("\">");
            self.write_items(&mut text, None, "");
            text.push_str("</Types>");
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
                // `<Types/>` gains content: it becomes a start tag and an end tag.
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
        for item in &self.items {
            let entry = match item {
                Item::Trivia(span) => {
                    text.push_str(
                        source
                            .and_then(|source| source.get(span.clone()))
                            .unwrap_or_default(),
                    );
                    continue;
                }
                Item::Entry(key) => match self.get(key) {
                    Some(entry) => entry,
                    None => continue,
                },
            };
            if let (Some(raw), Some(source)) = (&entry.raw, source) {
                text.push_str(source.get(raw.clone()).unwrap_or_default());
                continue;
            }
            text.push_str(&entry.indent);
            text.push('<');
            if !prefix.is_empty() {
                text.push_str(prefix);
                text.push(':');
            }
            match &entry.kind {
                Kind::Default { extension } => {
                    text.push_str("Default Extension=\"");
                    xml::escape_attribute(extension, text);
                }
                Kind::Override { part_name } => {
                    // In ASCII, as ZIP entry names are, for readers that follow the 2006 edition of the standard.
                    text.push_str("Override PartName=\"/");
                    xml::escape_attribute(&part_name.zip_name(), text);
                }
            }
            text.push_str("\" ContentType=\"");
            xml::escape_attribute(&entry.content_type, text);
            text.push_str("\"/>");
        }
    }
}

/// Reads one `Default` or `Override` element.
fn read_entry(element: &Element) -> Result<Entry, ContentTypesError> {
    let is_default = element.is(NAMESPACE, "Default");
    if !is_default && !element.is(NAMESPACE, "Override") {
        return Err(ContentTypesError::UnexpectedElement);
    }
    for node in &element.children {
        match node {
            Node::Element(_) => return Err(ContentTypesError::UnexpectedElement),
            Node::Text { value, .. } if !value.chars().all(xml::is_space) => {
                return Err(ContentTypesError::UnexpectedText);
            }
            Node::CData { .. } => return Err(ContentTypesError::UnexpectedText),
            Node::Text { .. } | Node::Markup { .. } => {}
        }
    }
    let allowed: &[&str] = if is_default {
        &["Extension", "ContentType"]
    } else {
        &["PartName", "ContentType"]
    };
    if element.attributes.iter().any(|attribute| {
        attribute.namespace.is_some() || !allowed.contains(&attribute.local_name.as_str())
    }) {
        return Err(ContentTypesError::UnexpectedAttribute);
    }
    let content_type = element
        .attribute("ContentType")
        .ok_or(ContentTypesError::MissingAttribute)?;
    if !is_media_type(content_type) {
        return Err(ContentTypesError::InvalidContentType);
    }
    let kind = if is_default {
        let extension = element
            .attribute("Extension")
            .ok_or(ContentTypesError::MissingAttribute)?;
        if !is_extension(extension) {
            return Err(ContentTypesError::InvalidExtension);
        }
        Kind::Default {
            extension: extension.to_owned(),
        }
    } else {
        let written = element
            .attribute("PartName")
            .ok_or(ContentTypesError::MissingAttribute)?;
        // `PartName` is an xsd:anyURI, so non-ASCII characters may be percent-encoded, as in ZIP entry names.
        let part_name = PartName::new(&percent::decode_non_ascii(written))
            .map_err(ContentTypesError::InvalidPartName)?;
        Kind::Override { part_name }
    };
    Ok(Entry {
        kind,
        content_type: content_type.to_owned(),
        raw: Some(element.span.clone()),
        indent: String::new(),
    })
}

/// The key under which two extensions are the same: non-ASCII characters percent-decoded, ASCII letters in lower case (§7.2.3.4 c).
fn extension_key(extension: &str) -> String {
    let mut key = percent::decode_non_ascii(extension).into_owned();
    key.make_ascii_lowercase();
    key
}

/// Whether `extension` matches `ST_Extension`: letters, digits, `-_~`, `!$&'()*+,:=@` and percent-encoded bytes, at least one, and no dot.
fn is_extension(extension: &str) -> bool {
    let bytes = extension.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        if byte == b'%' {
            let valid = bytes
                .get(at + 1)
                .copied()
                .and_then(percent::hex_digit)
                .is_some()
                && bytes
                    .get(at + 2)
                    .copied()
                    .and_then(percent::hex_digit)
                    .is_some();
            if !valid {
                return false;
            }
            at += 3;
        } else if byte.is_ascii_alphanumeric() || b"-_~!$&'()*+,:=@".contains(&byte) {
            at += 1;
        } else {
            return false;
        }
    }
    true
}

/// Whether `text` is a media type (RFC 7231 §3.1.1.1): `type/subtype`, optionally followed by parameters `; name=value`, where the value is a token or a quoted string.
pub(crate) fn is_media_type(text: &str) -> bool {
    let (media, parameters) = text
        .split_once(';')
        .map_or((text, None), |(media, rest)| (media, Some(rest)));
    let Some((kind, subtype)) = media.split_once('/') else {
        return false;
    };
    // White space may follow the subtype only before parameters.
    let subtype_token = subtype.trim_end_matches([' ', '\t']);
    if !is_token(kind) || !is_token(subtype_token) {
        return false;
    }
    if parameters.is_none() && subtype_token.len() != subtype.len() {
        return false;
    }
    let Some(mut rest) = parameters else {
        return true;
    };
    loop {
        rest = rest.trim_start_matches([' ', '\t']);
        let Some((name, value)) = rest.split_once('=') else {
            return false;
        };
        if !is_token(name) {
            return false;
        }
        let after_value = if let Some(quoted) = value.strip_prefix('"') {
            let Some(length) = quoted_string_length(quoted) else {
                return false;
            };
            quoted.get(length..).unwrap_or_default()
        } else {
            let end = value.find([';', ' ', '\t']).unwrap_or(value.len());
            if !is_token(value.get(..end).unwrap_or_default()) {
                return false;
            }
            value.get(end..).unwrap_or_default()
        };
        let after_value = after_value.trim_start_matches([' ', '\t']);
        if after_value.is_empty() {
            return true;
        }
        match after_value.strip_prefix(';') {
            Some(next) => rest = next,
            None => return false,
        }
    }
}

/// The length of a quoted string's content and closing quote, after its opening quote; `None` if it is not closed or has a character it may not have.
fn quoted_string_length(text: &str) -> Option<usize> {
    let mut characters = text.char_indices();
    while let Some((index, character)) = characters.next() {
        match character {
            '"' => return Some(index + 1),
            '\\' => {
                // A quoted pair: a backslash and a tab, a space, a visible ASCII character or a byte above 127.
                characters.next().filter(
                    |&(_, escaped)| matches!(escaped, '\t' | ' '..='~' | '\u{80}'..='\u{FF}'),
                )?;
            }
            '\t' | ' ' | '!' | '#'..='[' | ']'..='~' | '\u{80}'..='\u{FF}' => {}
            _ => return None,
        }
    }
    None
}

/// Whether `text` is an RFC 7230 token: one or more of the visible ASCII characters other than the separators `"(),/:;<=>?@[\]{}`.
fn is_token(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>";

    fn name(text: &str) -> PartName {
        PartName::new(text).unwrap()
    }

    fn parse(text: &str) -> Result<ContentTypes, Error> {
        ContentTypes::parse(text.as_bytes(), &Limits::default())
    }

    fn error(text: &str) -> ContentTypesError {
        match parse(text).unwrap_err() {
            Error::Package(PackageError::ContentTypes(error)) => error,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn looks_up_media_types() {
        let types = parse(WORD).unwrap();
        assert_eq!(
            types.content_type(&name("/word/document.xml")),
            Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
            )
        );
        assert_eq!(
            types.content_type(&name("/WORD/Document.XML")),
            types.content_type(&name("/word/document.xml"))
        );
        assert_eq!(
            types.content_type(&name("/word/styles.xml")),
            Some("application/xml")
        );
        assert_eq!(
            types.content_type(&name("/word/styles.XML")),
            Some("application/xml")
        );
        assert_eq!(
            types.content_type(&name("/_rels/.rels")),
            Some("application/vnd.openxmlformats-package.relationships+xml")
        );
        assert_eq!(types.content_type(&name("/word/media/image1.png")), None);
        assert_eq!(types.content_type(&name("/noextension")), None);
        assert_eq!(types.defaults().count(), 2);
        assert_eq!(types.overrides().count(), 1);
    }

    #[test]
    fn writes_an_unchanged_stream_back_exactly() {
        let indented = "<?xml version='1.0' encoding='UTF-8' standalone='yes'?>\n<!-- made by hand -->\n<ct:Types xmlns:ct=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n  <ct:Default Extension='png' ContentType='image/png' />\n  <ct:Override PartName=\"/m%C3%A9dia/a.xml\" ContentType=\"application/xml\"></ct:Override>\n</ct:Types>\n";
        for text in [WORD, indented] {
            let types = parse(text).unwrap();
            assert!(!types.is_modified());
            assert_eq!(types.to_xml(), text.as_bytes());
        }
        let types = parse(indented).unwrap();
        assert_eq!(
            types.content_type(&name("/média/a.xml")),
            Some("application/xml")
        );
    }

    #[test]
    fn changes_only_what_it_changes() {
        let mut types = parse(WORD).unwrap();
        types
            .set_override(
                &name("/word/styles.xml"),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
            )
            .unwrap();
        types.set_default("png", "image/png").unwrap();
        let text = String::from_utf8(types.to_xml()).unwrap();
        assert_eq!(text, WORD.replace(
            "<Default Extension=\"xml\" ContentType=\"application/xml\"/>",
            "<Default Extension=\"xml\" ContentType=\"application/xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/>",
        ).replace(
            "</Types>",
            "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>",
        ));
        // Removing what was added gives back the original text.
        assert!(types.remove_override(&name("/word/styles.xml")));
        assert!(types.remove_default("PNG"));
        assert!(types.is_modified());
        assert_eq!(types.to_xml(), WORD.as_bytes());
    }

    #[test]
    fn indents_new_entries_like_their_neighbours() {
        let text = "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n  <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n</Types>";
        let mut types = parse(text).unwrap();
        types
            .set_override(&name("/a.bin"), "application/octet-stream")
            .unwrap();
        assert_eq!(
            String::from_utf8(types.to_xml()).unwrap(),
            "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n  <Default Extension=\"xml\" ContentType=\"application/xml\"/>\n  <Override PartName=\"/a.bin\" ContentType=\"application/octet-stream\"/>\n</Types>"
        );
        let empty =
            "<p:Types xmlns:p=\"http://schemas.openxmlformats.org/package/2006/content-types\"/>";
        let mut types = parse(empty).unwrap();
        assert_eq!(types.to_xml(), empty.as_bytes());
        types.set_default("xml", "application/xml").unwrap();
        assert_eq!(
            String::from_utf8(types.to_xml()).unwrap(),
            "<p:Types xmlns:p=\"http://schemas.openxmlformats.org/package/2006/content-types\"><p:Default Extension=\"xml\" ContentType=\"application/xml\"/></p:Types>"
        );
    }

    #[test]
    fn writes_a_new_stream_like_word() {
        let mut types = ContentTypes::new();
        types
            .register(
                &name("/_rels/.rels"),
                "application/vnd.openxmlformats-package.relationships+xml",
            )
            .unwrap();
        types
            .register(
                &name("/word/document.xml"),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            )
            .unwrap();
        types
            .register(
                &name("/word/styles.xml"),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml",
            )
            .unwrap();
        types
            .register(&name("/word/media/é.png"), "image/png")
            .unwrap();
        types
            .register(&name("/word/media/b.PNG"), "image/PNG")
            .unwrap();
        types.register(&name("/LICENSE"), "text/plain").unwrap();
        assert_eq!(
            String::from_utf8(types.to_xml()).unwrap(),
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Default Extension=\"png\" ContentType=\"image/png\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/LICENSE\" ContentType=\"text/plain\"/></Types>"
        );
        let read = ContentTypes::parse(&types.to_xml(), &Limits::default()).unwrap();
        assert_eq!(
            read.content_type(&name("/word/media/é.png")),
            Some("image/png")
        );
        assert_eq!(
            read.content_type(&name("/word/styles.xml")),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml")
        );
    }

    #[test]
    fn refuses_streams_that_break_the_rules() {
        let types = |body: &str| format!("<Types xmlns=\"{NAMESPACE}\">{body}</Types>");
        assert_eq!(error("<Types/>"), ContentTypesError::UnexpectedRoot);
        assert_eq!(
            error(&format!("<Typez xmlns=\"{NAMESPACE}\"/>")),
            ContentTypesError::UnexpectedRoot
        );
        assert_eq!(
            error(&format!("<Types xmlns=\"{NAMESPACE}\" a=\"1\"/>")),
            ContentTypesError::UnexpectedAttribute
        );
        assert_eq!(
            error(&types("<Other/>")),
            ContentTypesError::UnexpectedElement
        );
        assert_eq!(
            error(&types(
                "<Default xmlns=\"urn:x\" Extension=\"a\" ContentType=\"a/b\"/>"
            )),
            ContentTypesError::UnexpectedElement
        );
        assert_eq!(error(&types("text")), ContentTypesError::UnexpectedText);
        assert_eq!(
            error(&types("<![CDATA[ ]]>")),
            ContentTypesError::UnexpectedText
        );
        assert_eq!(
            error(&types(
                "<Default Extension=\"a\" ContentType=\"a/b\"><x/></Default>"
            )),
            ContentTypesError::UnexpectedElement
        );
        assert_eq!(
            error(&types(
                "<Default Extension=\"a\" ContentType=\"a/b\" Other=\"1\"/>"
            )),
            ContentTypesError::UnexpectedAttribute
        );
        assert_eq!(
            error(&types(
                "<Default Extension=\"a\" xmlns:m=\"urn:m\" m:x=\"1\" ContentType=\"a/b\"/>"
            )),
            ContentTypesError::UnexpectedAttribute
        );
        assert_eq!(
            error(&types("<Default Extension=\"a\"/>")),
            ContentTypesError::MissingAttribute
        );
        assert_eq!(
            error(&types("<Override ContentType=\"a/b\"/>")),
            ContentTypesError::MissingAttribute
        );
        assert_eq!(
            error(&types(
                "<Default Extension=\"tar.gz\" ContentType=\"a/b\"/>"
            )),
            ContentTypesError::InvalidExtension
        );
        assert_eq!(
            error(&types("<Default Extension=\"\" ContentType=\"a/b\"/>")),
            ContentTypesError::InvalidExtension
        );
        assert_eq!(
            error(&types("<Default Extension=\"a\" ContentType=\"text\"/>")),
            ContentTypesError::InvalidContentType
        );
        assert_eq!(
            error(&types("<Override PartName=\"a.xml\" ContentType=\"a/b\"/>")),
            ContentTypesError::InvalidPartName(crate::PartNameError::MissingLeadingSlash)
        );
        assert_eq!(
            error(&types(
                "<Default Extension=\"xml\" ContentType=\"a/b\"/><Default Extension=\"XML\" ContentType=\"a/c\"/>"
            )),
            ContentTypesError::DuplicateDefault
        );
        assert_eq!(
            error(&types(
                "<Override PartName=\"/a\" ContentType=\"a/b\"/><Override PartName=\"/A\" ContentType=\"a/b\"/>"
            )),
            ContentTypesError::DuplicateOverride
        );
    }

    #[test]
    fn checks_media_types_and_extensions() {
        for valid in [
            "text/plain",
            "application/vnd.openxmlformats-package.relationships+xml",
            "text/plain;charset=utf-8",
            "text/plain ; charset=\"utf-8\"",
            "a/b;x=y;z=\"q\\\"t\"",
            "image/svg+xml",
        ] {
            assert!(is_media_type(valid), "{valid}");
        }
        for invalid in [
            "",
            "text",
            "/plain",
            "text/",
            "text/pl ain",
            "text/plain;",
            "text/plain;x",
            "text/plain;x=",
            "text/plain;x=\"open",
            "text/plain x=y",
            "a/b/c",
            "té/xt",
        ] {
            assert!(!is_media_type(invalid), "{invalid}");
        }
        for valid in ["xml", "rels", "png", "x-y_z~", "%41", "a:b", "a@b"] {
            assert!(is_extension(valid), "{valid}");
        }
        for invalid in ["", "a.b", "a b", "é", "%4", "a/b", "a?b"] {
            assert!(!is_extension(invalid), "{invalid}");
        }
    }

    #[test]
    fn keeps_utf16_streams_in_utf16() {
        let text = WORD.replace("UTF-8", "UTF-16");
        let bytes = xml::encode(&text, Encoding::Utf16Le);
        let mut types = ContentTypes::parse(&bytes, &Limits::default()).unwrap();
        assert_eq!(types.to_xml(), bytes);
        types.set_default("png", "image/png").unwrap();
        let changed = types.to_xml();
        assert_eq!(changed.get(..2), Some(&[0xFF, 0xFE][..]));
        let read = ContentTypes::parse(&changed, &Limits::default()).unwrap();
        assert_eq!(read.content_type(&name("/a.png")), Some("image/png"));
    }
}
