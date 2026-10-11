//! The Open Packaging Conventions as far as tagging needs them (ECMA-376 Part 2): content types, relationships and part names.
//!
//! **Marked for replacement** by `bayan-opc` (CORE-005).

use std::collections::BTreeMap;

use super::vocabulary::{Ns, namespace};
use super::xml::{self, Handler, Name, Start, Text, XmlError, XmlLimits};

/// The most entries `[Content_Types].xml` or one relationships part may have.
pub const MAX_PACKAGE_ITEMS: usize = 10_000;

/// Why a package's structure cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpcError {
    /// A part is not XML the scanner accepts.
    Xml(XmlError),
    /// More content types or relationships than [`MAX_PACKAGE_ITEMS`].
    TooManyItems,
    /// A required attribute is missing; the text names it.
    Missing(&'static str),
}

impl From<XmlError> for OpcError {
    fn from(error: XmlError) -> Self {
        Self::Xml(error)
    }
}

impl std::fmt::Display for OpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Xml(error) => error.fmt(formatter),
            Self::TooManyItems => {
                formatter.write_str("more content types or relationships than the limit")
            }
            Self::Missing(what) => write!(formatter, "missing {what}"),
        }
    }
}

/// The content types of a package (`[Content_Types].xml`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ContentTypes {
    /// Content type by file extension, in ASCII lower case.
    defaults: BTreeMap<String, String>,
    /// Content type by part name, normalized with [`normalize`].
    overrides: BTreeMap<String, String>,
}

impl ContentTypes {
    /// Reads `[Content_Types].xml`.
    ///
    /// # Errors
    ///
    /// [`OpcError`] when the part is not well-formed, lacks required attributes, or has more than [`MAX_PACKAGE_ITEMS`] entries.
    pub fn parse(bytes: &[u8], limits: &XmlLimits) -> Result<Self, OpcError> {
        struct Reader {
            types: ContentTypes,
            items: usize,
            problem: Option<OpcError>,
        }
        impl Handler for Reader {
            fn start(&mut self, start: &Start<'_, '_>) {
                let name = start.name();
                if self.problem.is_some()
                    || namespace(name.namespace).map(|(ns, _)| ns) != Some(Ns::ContentTypes)
                {
                    return;
                }
                let (key, value) = match name.local {
                    "Default" => ("Extension", "ContentType"),
                    "Override" => ("PartName", "ContentType"),
                    _ => return,
                };
                self.items += 1;
                if self.items > MAX_PACKAGE_ITEMS {
                    self.problem = Some(OpcError::TooManyItems);
                    return;
                }
                let (Some(key), Some(content_type)) =
                    (start.attribute("", key), start.attribute("", value))
                else {
                    self.problem = Some(OpcError::Missing("an attribute of a content type"));
                    return;
                };
                let content_type = content_type.decoded().trim().to_ascii_lowercase();
                if name.local == "Default" {
                    self.types
                        .defaults
                        .insert(key.decoded().to_ascii_lowercase(), content_type);
                } else {
                    self.types
                        .overrides
                        .insert(normalize(&key.decoded()), content_type);
                }
            }
            fn end(&mut self, _: Name<'_>) {}
            fn text(&mut self, _: Text<'_>) {}
        }
        let mut reader = Reader {
            types: Self::default(),
            items: 0,
            problem: None,
        };
        xml::scan(bytes, limits, &mut reader)?;
        match reader.problem {
            Some(problem) => Err(problem),
            None => Ok(reader.types),
        }
    }

    /// The content type of the part named `part` (with a leading `/`), in ASCII lower case.
    #[must_use]
    pub fn of(&self, part: &str) -> Option<&str> {
        self.overrides
            .get(&normalize(part))
            .or_else(|| {
                let file = part.rsplit('/').next().unwrap_or(part);
                let (_, extension) = file.rsplit_once('.')?;
                self.defaults.get(&extension.to_ascii_lowercase())
            })
            .map(String::as_str)
    }
}

/// Where a relationship points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A part of the package, by its normalized name (see [`normalize`]).
    Internal(String),
    /// A resource outside the package. It is never fetched.
    External,
    /// A target that cannot name a part, such as one that climbs above the package root.
    Invalid,
}

/// A relationship of a part or of the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    /// The relationship type URI.
    pub rel_type: String,
    /// Where it points.
    pub target: Target,
}

/// Reads a relationships part whose source is the part named `source` (`/` for the package itself).
///
/// # Errors
///
/// [`OpcError`] when the part is not well-formed, lacks required attributes, or has more than [`MAX_PACKAGE_ITEMS`] relationships.
pub fn relationships(
    bytes: &[u8],
    source: &str,
    limits: &XmlLimits,
) -> Result<Vec<Relationship>, OpcError> {
    struct Reader<'a> {
        source: &'a str,
        found: Vec<Relationship>,
        problem: Option<OpcError>,
    }
    impl Handler for Reader<'_> {
        fn start(&mut self, start: &Start<'_, '_>) {
            let name = start.name();
            if self.problem.is_some()
                || name.local != "Relationship"
                || namespace(name.namespace).map(|(ns, _)| ns) != Some(Ns::Relationships)
            {
                return;
            }
            if self.found.len() >= MAX_PACKAGE_ITEMS {
                self.problem = Some(OpcError::TooManyItems);
                return;
            }
            let (Some(rel_type), Some(target)) =
                (start.attribute("", "Type"), start.attribute("", "Target"))
            else {
                self.problem = Some(OpcError::Missing("the type or target of a relationship"));
                return;
            };
            let external = start
                .attribute("", "TargetMode")
                .is_some_and(|mode| mode.decoded().trim() == "External");
            let target = if external {
                Target::External
            } else {
                resolve(self.source, target.decoded().trim())
                    .map_or(Target::Invalid, Target::Internal)
            };
            self.found.push(Relationship {
                rel_type: rel_type.decoded().trim().to_owned(),
                target,
            });
        }
        fn end(&mut self, _: Name<'_>) {}
        fn text(&mut self, _: Text<'_>) {}
    }
    let mut reader = Reader {
        source,
        found: Vec::new(),
        problem: None,
    };
    xml::scan(bytes, limits, &mut reader)?;
    match reader.problem {
        Some(problem) => Err(problem),
        None => Ok(reader.found),
    }
}

/// The name of the relationships part of `part` (`/` for the package): `/word/document.xml` → `/word/_rels/document.xml.rels`.
#[must_use]
pub fn relationships_part(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((folder, file)) if !file.is_empty() => format!("{folder}/_rels/{file}.rels"),
        _ => "/_rels/.rels".to_owned(),
    }
}

/// Resolves a relative reference `target` against the part named `source` (RFC 3986 §5.2, as OPC uses it), and normalizes the result. `None` when it climbs above the root or names no part.
#[must_use]
pub fn resolve(source: &str, target: &str) -> Option<String> {
    // A fragment or query never belongs to a part name.
    let target = target.split(['#', '?']).next().unwrap_or("");
    if target.is_empty() || target.contains("://") {
        return None;
    }
    let mut segments: Vec<&str> = Vec::new();
    if !target.starts_with('/') {
        let folder = source.rsplit_once('/').map_or("", |(folder, _)| folder);
        segments.extend(folder.split('/').filter(|segment| !segment.is_empty()));
    }
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return None;
    }
    Some(normalize(&format!("/{}", segments.join("/"))))
}

/// The form in which part names are compared: percent-encoded octets decoded, and ASCII letters in lower case (ECMA-376 Part 2 §6.2.2.3 compares part names ASCII case-insensitively).
#[must_use]
pub fn normalize(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        let hex = |at: usize| {
            bytes
                .get(at)
                .and_then(|&digit| char::from(digit).to_digit(16))
        };
        if byte == b'%'
            && let (Some(high), Some(low)) = (hex(index + 1), hex(index + 2))
            && let Ok(value) = u8::try_from(high * 16 + low)
        {
            decoded.push(value);
            index += 3;
            continue;
        }
        decoded.push(byte);
        index += 1;
    }
    let mut text = String::from_utf8_lossy(&decoded).into_owned();
    if !text.starts_with('/') {
        text.insert(0, '/');
    }
    text.make_ascii_lowercase();
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMITS: XmlLimits = XmlLimits {
        max_depth: 16,
        max_attributes: 16,
        max_name_len: 64,
        max_namespaces: 16,
    };

    #[test]
    fn resolves_relative_targets_like_opc() {
        assert_eq!(
            resolve("/word/document.xml", "styles.xml"),
            Some("/word/styles.xml".into())
        );
        assert_eq!(
            resolve("/word/document.xml", "../customXml/item1.xml"),
            Some("/customxml/item1.xml".into())
        );
        assert_eq!(
            resolve("/", "word/document.xml"),
            Some("/word/document.xml".into())
        );
        assert_eq!(
            resolve("/word/document.xml", "/word/Media/Image%201.png"),
            Some("/word/media/image 1.png".into())
        );
        assert_eq!(resolve("/word/document.xml", "../../escape.xml"), None);
        assert_eq!(resolve("/word/document.xml", "#bookmark"), None);
        assert_eq!(
            resolve("/word/document.xml", "https://example.invalid/x"),
            None
        );
        assert_eq!(
            relationships_part("/word/document.xml"),
            "/word/_rels/document.xml.rels"
        );
        assert_eq!(relationships_part("/"), "/_rels/.rels");
    }

    #[test]
    fn reads_content_types_and_relationships() {
        let types = ContentTypes::parse(
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="XML" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
            &LIMITS,
        )
        .unwrap();
        assert_eq!(
            types.of("/WORD/Document.xml"),
            Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"
            )
        );
        assert_eq!(types.of("/word/styles.xml"), Some("application/xml"));
        assert_eq!(types.of("/word/media/image1.png"), None);

        let found = relationships(
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="t1" Target="styles.xml"/><Relationship Id="rId2" Type="t2" Target="https://example.invalid/" TargetMode="External"/><Relationship Id="rId3" Type="t3" Target="../../x"/></Relationships>"#,
            "/word/document.xml",
            &LIMITS,
        )
        .unwrap();
        assert_eq!(
            found,
            [
                Relationship {
                    rel_type: "t1".into(),
                    target: Target::Internal("/word/styles.xml".into())
                },
                Relationship {
                    rel_type: "t2".into(),
                    target: Target::External
                },
                Relationship {
                    rel_type: "t3".into(),
                    target: Target::Invalid
                },
            ]
        );
        assert_eq!(
            relationships(br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="x"/></Relationships>"#, "/", &LIMITS),
            Err(OpcError::Missing("the type or target of a relationship"))
        );
    }
}
