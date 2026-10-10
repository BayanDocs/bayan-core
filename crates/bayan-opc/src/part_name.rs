//! Part names: validation, equivalence, the mapping to ZIP entry names, relationships parts, and the resolution of relationship targets (ECMA-376 Part 2 §6.2.2, §6.4, §6.5.2, §7.3.4 and §7.3.5).

use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};

use crate::percent;
use crate::{PartNameError, TargetError};

/// The name of the package relationships part.
const PACKAGE_RELATIONSHIPS: &str = "/_rels/.rels";

/// The name of a part, such as `/word/document.xml`: a slash followed by non-empty segments separated by slashes (ECMA-376 Part 2 §6.2.2).
///
/// A part name is checked when it is made, so every `PartName` is valid. Two part names are equal when they are equivalent, that is when they differ at most in the case of ASCII letters (`/Word/Document.xml` equals `/word/document.xml`), but a part name keeps the spelling it was made with, which is the spelling written back. Non-ASCII characters are written as themselves (the 2021 edition of the standard allows them); in a ZIP archive they are percent-encoded ([`PartName::zip_name`]).
#[derive(Clone)]
pub struct PartName {
    name: String,
    /// The name with ASCII letters in lower case, which decides equivalence.
    key: String,
}

impl PartName {
    /// Checks `name` and makes it a part name.
    ///
    /// # Errors
    ///
    /// A [`PartNameError`] if `name` breaks the part name grammar: it must start with `/`, have no empty segment and no segment that ends with a dot, use only the characters of IRI path segments (RFC 3987), and percent-encode no character that it could write as itself, nor a slash or backslash.
    pub fn new(name: &str) -> Result<PartName, PartNameError> {
        validate(name)?;
        Ok(PartName::trusted(name.to_owned()))
    }

    /// A part name from a string that is known to be valid.
    fn trusted(name: String) -> PartName {
        let key = name.to_ascii_lowercase();
        PartName { name, key }
    }

    /// The part name of a ZIP entry name: the characters that part names hold as themselves percent-decoded (`é` for `%C3%A9`, but `%C2%80` stays as it is) and a slash added in front (§7.3.5).
    ///
    /// # Errors
    ///
    /// A [`PartNameError`] if the result is not a valid part name; such an entry is not a part (§7.2.5.5).
    pub fn from_zip_name(zip_name: &str) -> Result<PartName, PartNameError> {
        let mut name = String::with_capacity(zip_name.len() + 1);
        name.push('/');
        name.push_str(&percent::decode_ucschar(zip_name));
        PartName::new(&name)
    }

    /// The part name as written.
    pub fn as_str(&self) -> &str {
        &self.name
    }

    /// The name of the ZIP entry that holds the part: without the leading slash, and with non-ASCII characters percent-encoded, because ZIP entry names in a package are ASCII (§7.3.3, §7.3.4).
    pub fn zip_name(&self) -> String {
        let without_slash = self.name.get(1..).unwrap_or_default();
        percent::encode_non_ascii(without_slash).into_owned()
    }

    /// The extension: the text after the last dot of the last segment, if there is a dot (§7.2.3.4).
    pub fn extension(&self) -> Option<&str> {
        let last = self.last_segment();
        last.rfind('.').and_then(|dot| last.get(dot + 1..))
    }

    /// The last segment.
    fn last_segment(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or_default()
    }

    /// Whether this is the name of a relationships part: its second-to-last segment is `_rels` and its last ends with `.rels`, compared without regard to the case of ASCII letters (§6.2.2.2).
    pub fn is_relationships_part(&self) -> bool {
        let mut segments = self.key.rsplit('/');
        let last = segments.next().unwrap_or_default();
        let parent = segments.next().unwrap_or_default();
        parent == "_rels" && last.ends_with(".rels") && segments.next().is_some()
    }

    /// The source of the relationships that this relationships part holds: the package for `/_rels/.rels`, otherwise the part whose name this one is made from (`/word/_rels/document.xml.rels` holds the relationships of `/word/document.xml`). `None` if this is not a relationships part.
    ///
    /// # Errors
    ///
    /// [`PartNameError`] if the name does not make a valid source part name (such as `/word/_rels/.rels`).
    pub fn relationships_source(&self) -> Option<Result<RelationshipSource, PartNameError>> {
        if !self.is_relationships_part() {
            return None;
        }
        if self.key == PACKAGE_RELATIONSHIPS {
            return Some(Ok(RelationshipSource::Package));
        }
        // Remove the `_rels` segment and the `.rels` ending, keeping the spelling of the rest.
        let last_slash = self.name.rfind('/')?;
        let parent_slash = self.name.get(..last_slash)?.rfind('/')?;
        let folder = self.name.get(..=parent_slash)?;
        let last = self.name.get(last_slash + 1..)?;
        let source = last.get(..last.len().checked_sub(".rels".len())?)?;
        let mut name = String::with_capacity(folder.len() + source.len());
        name.push_str(folder);
        name.push_str(source);
        Some(PartName::new(&name).map(RelationshipSource::Part))
    }

    /// The name of the relationships part that holds this part's relationships: `_rels` inserted before the last segment and `.rels` added to it (§6.5.2.3).
    pub fn relationships_part(&self) -> PartName {
        let last_slash = self.name.rfind('/').unwrap_or_default();
        let (folder, last) = self.name.split_at(last_slash + 1);
        let mut name = String::with_capacity(self.name.len() + "_rels/.rels".len());
        name.push_str(folder);
        name.push_str("_rels/");
        name.push_str(last);
        name.push_str(".rels");
        // Valid: `_rels` and a valid last segment followed by `.rels` are valid segments.
        PartName::trusted(name)
    }

    /// Whether this name is derivable from `other`: equivalent to `other` followed by a slash and more (`/a/b` is derivable from `/a`, §6.2.2.3).
    pub fn is_derivable_from(&self, other: &PartName) -> bool {
        self.key.len() > other.key.len()
            && self.key.starts_with(&other.key)
            && self.key.as_bytes().get(other.key.len()) == Some(&b'/')
    }

    /// The key that decides equivalence: the name with ASCII letters in lower case.
    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

impl PartialEq for PartName {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for PartName {}

impl PartialOrd for PartName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PartName {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl Hash for PartName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.key.hash(state);
    }
}

impl fmt::Debug for PartName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("PartName").field(&self.name).finish()
    }
}

impl fmt::Display for PartName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

/// What a set of relationships belongs to: the package as a whole, or one part.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RelationshipSource {
    /// The package; its relationships are in `/_rels/.rels`.
    Package,
    /// A part; its relationships are in its relationships part ([`PartName::relationships_part`]).
    Part(PartName),
}

impl RelationshipSource {
    /// The name of the relationships part that holds this source's relationships.
    pub fn relationships_part(&self) -> PartName {
        match self {
            RelationshipSource::Package => PartName::trusted(PACKAGE_RELATIONSHIPS.to_owned()),
            RelationshipSource::Part(part) => part.relationships_part(),
        }
    }

    /// Resolves the target of an internal relationship from this source to a part name, as RFC 3986 resolves a relative reference against a base (§6.4, §6.5.2): against `/` for the package, against the source part's name for a part. The query and fragment of the target are not part of the name; `.` and `..` segments are removed and can never climb above the root, and percent-encoded non-ASCII characters are decoded, as in ZIP entry names.
    ///
    /// # Errors
    ///
    /// [`TargetError::NotRelative`] if the target has a scheme (`http:`, `file:`, `C:`) or an authority (`//server`), [`TargetError::Empty`] if it refers to the source itself, and [`TargetError::InvalidPartName`] if the result is not a valid part name.
    pub fn resolve(&self, target: &str) -> Result<PartName, TargetError> {
        let base = match self {
            RelationshipSource::Package => "/",
            RelationshipSource::Part(part) => part.as_str(),
        };
        let path = resolve_reference(base, target)?;
        PartName::new(&percent::decode_ucschar(&path)).map_err(TargetError::InvalidPartName)
    }
}

/// Resolves the path of the relative reference `reference` against the absolute path `base` (RFC 3986 §5.2.2), ignoring its query and fragment.
fn resolve_reference(base: &str, reference: &str) -> Result<String, TargetError> {
    if reference.starts_with("//") {
        return Err(TargetError::NotRelative);
    }
    let end_of_path = reference.find(['?', '#']).unwrap_or(reference.len());
    let path = reference.get(..end_of_path).unwrap_or_default();
    // A colon in the first segment means a scheme (or a Windows drive letter): RFC 3986 does not allow it in a relative path.
    let first_segment = path.split('/').next().unwrap_or_default();
    if first_segment.contains(':') {
        return Err(TargetError::NotRelative);
    }
    if path.is_empty() {
        return Err(TargetError::Empty);
    }
    let merged = if path.starts_with('/') {
        path.to_owned()
    } else {
        let folder = base
            .rfind('/')
            .and_then(|slash| base.get(..=slash))
            .unwrap_or("/");
        let mut merged = String::with_capacity(folder.len() + path.len());
        merged.push_str(folder);
        merged.push_str(path);
        merged
    };
    Ok(remove_dot_segments(&merged))
}

/// RFC 3986 §5.2.4: removes `.` and `..` segments from an absolute path; a `..` at the root stays at the root.
fn remove_dot_segments(path: &str) -> String {
    let mut input = path;
    let mut output = String::with_capacity(path.len());
    while !input.is_empty() {
        if let Some(rest) = input.strip_prefix("../") {
            input = rest;
        } else if let Some(rest) = input.strip_prefix("./") {
            input = rest;
        } else if input.starts_with("/./") {
            input = &input[2..];
        } else if input == "/." {
            input = "/";
        } else if input.starts_with("/../") || input == "/.." {
            input = if input == "/.." { "/" } else { &input[3..] };
            let cut = output.rfind('/').unwrap_or_default();
            output.truncate(cut);
        } else if input == "." || input == ".." {
            input = "";
        } else {
            // Move the first segment, with its leading slash if it has one, to the output.
            let start = usize::from(input.starts_with('/'));
            let end = input
                .get(start..)
                .and_then(|rest| rest.find('/'))
                .map_or(input.len(), |slash| start + slash);
            output.push_str(&input[..end]);
            input = &input[end..];
        }
    }
    output
}

/// Checks `name` against the part name grammar of §6.2.2.2.
fn validate(name: &str) -> Result<(), PartNameError> {
    let rest = name
        .strip_prefix('/')
        .ok_or(PartNameError::MissingLeadingSlash)?;
    for segment in rest.split('/') {
        if segment.is_empty() {
            return Err(PartNameError::EmptySegment);
        }
        if segment.ends_with('.') {
            return Err(PartNameError::TrailingDot);
        }
        validate_segment(segment)?;
    }
    Ok(())
}

/// Checks the characters of one segment: IRI path characters (`ipchar` of RFC 3987) and percent-encoding that encodes neither an unreserved character nor a slash or backslash.
fn validate_segment(segment: &str) -> Result<(), PartNameError> {
    let bytes = segment.as_bytes();
    let mut at = 0;
    while let Some(character) = segment.get(at..).and_then(|rest| rest.chars().next()) {
        if character == '%' {
            let high = bytes.get(at + 1).copied().and_then(percent::hex_digit);
            let low = bytes.get(at + 2).copied().and_then(percent::hex_digit);
            let (Some(high), Some(low)) = (high, low) else {
                return Err(PartNameError::InvalidPercentEncoding);
            };
            let byte = high << 4 | low;
            let forbidden = if byte.is_ascii() {
                is_ascii_unreserved(char::from(byte)) || byte == b'/' || byte == b'\\'
            } else {
                // A non-ASCII character, percent-encoded, is an unreserved character written in a form part names do not allow.
                percent::decode_character(bytes, at)
                    .is_some_and(|(decoded, _)| percent::is_ucschar(decoded))
            };
            if forbidden {
                return Err(PartNameError::ForbiddenPercentEncoding);
            }
            at += 3;
        } else if is_path_character(character) {
            at += character.len_utf8();
        } else {
            return Err(PartNameError::InvalidCharacter);
        }
    }
    Ok(())
}

/// The characters an IRI path segment may contain as themselves: unreserved characters (including the non-ASCII `ucschar` of RFC 3987), sub-delimiters, `:` and `@`.
fn is_path_character(character: char) -> bool {
    is_ascii_unreserved(character)
        || matches!(
            character,
            '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=' | ':' | '@'
        )
        || percent::is_ucschar(character)
}

/// The unreserved ASCII characters of RFC 3986: letters, digits, `-`, `.`, `_` and `~`.
fn is_ascii_unreserved(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_' | '~')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(text: &str) -> PartName {
        PartName::new(text).unwrap()
    }

    #[test]
    fn accepts_the_part_names_of_documents() {
        for text in [
            "/word/document.xml",
            "/x",
            "/_rels/.rels",
            "/word/_rels/document.xml.rels",
            "/customXml/item1.xml",
            "/word/media/image%20one.png",
            "/média/é.png",
            "/a/b/c/d",
            "/a~b_c-d!$&'()*+,;=:@",
            "/😀",
            "/%25",
            "/%C3",
        ] {
            assert!(PartName::new(text).is_ok(), "{text}");
        }
    }

    #[test]
    fn refuses_names_that_break_the_grammar() {
        let cases = [
            ("", PartNameError::MissingLeadingSlash),
            ("word/document.xml", PartNameError::MissingLeadingSlash),
            ("/", PartNameError::EmptySegment),
            ("//a", PartNameError::EmptySegment),
            ("/a/", PartNameError::EmptySegment),
            ("/a.", PartNameError::TrailingDot),
            ("/a/./b", PartNameError::TrailingDot),
            ("/a/../b", PartNameError::TrailingDot),
            ("/a b", PartNameError::InvalidCharacter),
            ("/a\\b", PartNameError::InvalidCharacter),
            ("/[Content_Types].xml", PartNameError::InvalidCharacter),
            ("/a#b", PartNameError::InvalidCharacter),
            ("/a?b", PartNameError::InvalidCharacter),
            ("/a\u{7f}", PartNameError::InvalidCharacter),
            ("/a\u{fffe}", PartNameError::InvalidCharacter),
            ("/a\u{e000}", PartNameError::InvalidCharacter),
            ("/a%", PartNameError::InvalidPercentEncoding),
            ("/a%4", PartNameError::InvalidPercentEncoding),
            ("/a%zz", PartNameError::InvalidPercentEncoding),
            ("/a%41", PartNameError::ForbiddenPercentEncoding),
            ("/a%7e", PartNameError::ForbiddenPercentEncoding),
            ("/a%2F", PartNameError::ForbiddenPercentEncoding),
            ("/a%5c", PartNameError::ForbiddenPercentEncoding),
            ("/a%C3%A9", PartNameError::ForbiddenPercentEncoding),
        ];
        for (text, error) in cases {
            assert_eq!(PartName::new(text).unwrap_err(), error, "{text:?}");
        }
    }

    #[test]
    fn compares_ascii_letters_without_case_only() {
        assert_eq!(name("/Word/Document.XML"), name("/word/document.xml"));
        assert_eq!(name("/a%2a"), name("/a%2A"));
        assert_ne!(name("/é"), name("/É"));
        assert_eq!(name("/Word/Document.XML").as_str(), "/Word/Document.XML");
    }

    #[test]
    fn maps_to_and_from_zip_entry_names() {
        assert_eq!(
            PartName::from_zip_name("word/document.xml").unwrap(),
            name("/word/document.xml")
        );
        let unicode = PartName::from_zip_name("m%C3%A9dia/%E2%82%AC.png").unwrap();
        assert_eq!(unicode.as_str(), "/média/€.png");
        assert_eq!(unicode.zip_name(), "m%C3%A9dia/%E2%82%AC.png");
        assert_eq!(name("/a%20b").zip_name(), "a%20b");
        assert!(PartName::from_zip_name("[Content_Types].xml").is_err());
        assert!(PartName::from_zip_name("word/").is_err());
        assert!(PartName::from_zip_name("a%2Fb").is_err());
    }

    #[test]
    fn knows_extensions() {
        assert_eq!(name("/word/document.xml").extension(), Some("xml"));
        assert_eq!(name("/a.b/c").extension(), None);
        assert_eq!(name("/archive.tar.gz").extension(), Some("gz"));
        assert_eq!(name("/.rels").extension(), Some("rels"));
    }

    #[test]
    fn finds_relationships_parts_and_their_sources() {
        let document = name("/word/document.xml");
        assert_eq!(
            document.relationships_part(),
            name("/word/_rels/document.xml.rels")
        );
        assert_eq!(
            RelationshipSource::Package.relationships_part(),
            name("/_rels/.rels")
        );
        assert_eq!(
            name("/word/_rels/document.xml.rels").relationships_source(),
            Some(Ok(RelationshipSource::Part(document)))
        );
        assert_eq!(
            name("/_RELS/.RELS").relationships_source(),
            Some(Ok(RelationshipSource::Package))
        );
        assert_eq!(
            name("/_rels/a.rels").relationships_source(),
            Some(Ok(RelationshipSource::Part(name("/a"))))
        );
        assert_eq!(
            name("/word/_rels/.rels").relationships_source(),
            Some(Err(PartNameError::EmptySegment))
        );
        assert_eq!(name("/word/document.xml").relationships_source(), None);
        assert_eq!(name("/_rels").relationships_source(), None);
        assert_eq!(name("/rels/a.rels").relationships_source(), None);
        assert!(name("/word/_Rels/X.Rels").is_relationships_part());
        let spelled = name("/Word/_rels/Doc.xml.rels").relationships_source();
        assert_eq!(
            spelled.map(|source| source.map(|source| match source {
                RelationshipSource::Part(part) => part.as_str().to_owned(),
                RelationshipSource::Package => String::new(),
            })),
            Some(Ok("/Word/Doc.xml".to_owned()))
        );
    }

    #[test]
    fn knows_derivable_names() {
        assert!(name("/a/b").is_derivable_from(&name("/a")));
        assert!(name("/A/b").is_derivable_from(&name("/a")));
        assert!(!name("/ab").is_derivable_from(&name("/a")));
        assert!(!name("/a").is_derivable_from(&name("/a")));
        assert!(!name("/a").is_derivable_from(&name("/a/b")));
    }

    #[test]
    fn resolves_targets_like_rfc_3986() {
        let package = RelationshipSource::Package;
        let document = RelationshipSource::Part(name("/word/document.xml"));
        let resolve = |source: &RelationshipSource, target: &str| {
            source.resolve(target).map(|part| part.as_str().to_owned())
        };
        assert_eq!(
            resolve(&package, "word/document.xml").unwrap(),
            "/word/document.xml"
        );
        assert_eq!(
            resolve(&package, "/word/document.xml").unwrap(),
            "/word/document.xml"
        );
        assert_eq!(
            resolve(&document, "styles.xml").unwrap(),
            "/word/styles.xml"
        );
        assert_eq!(
            resolve(&document, "./media/image1.png").unwrap(),
            "/word/media/image1.png"
        );
        assert_eq!(
            resolve(&document, "../customXml/item1.xml").unwrap(),
            "/customXml/item1.xml"
        );
        assert_eq!(
            resolve(&document, "../../../../etc/passwd").unwrap(),
            "/etc/passwd"
        );
        assert_eq!(
            resolve(&document, "media/a.png#frag").unwrap(),
            "/word/media/a.png"
        );
        assert_eq!(
            resolve(&document, "media/a.png?x=1").unwrap(),
            "/word/media/a.png"
        );
        assert_eq!(
            resolve(&document, "m%C3%A9dia/x.png").unwrap(),
            "/word/média/x.png"
        );
        assert_eq!(resolve(&document, "a/b/../c").unwrap(), "/word/a/c");
        // The examples of ECMA-376 Part 2 §6.4.3 (case 1, base /a/b/foo.xml).
        let foo = RelationshipSource::Part(name("/a/b/foo.xml"));
        assert_eq!(resolve(&foo, "/b/bar.xml").unwrap(), "/b/bar.xml");
        assert_eq!(resolve(&foo, "bar.xml").unwrap(), "/a/b/bar.xml");
        assert_eq!(resolve(&foo, "./bar.xml").unwrap(), "/a/b/bar.xml");
        assert_eq!(resolve(&foo, "../bar.xml").unwrap(), "/a/bar.xml");
        // Case 2 of §6.4.3: from the package, `..` stays at the root.
        assert_eq!(resolve(&package, "../bar.xml").unwrap(), "/bar.xml");
    }

    #[test]
    fn refuses_targets_that_are_not_parts() {
        let document = RelationshipSource::Part(name("/word/document.xml"));
        for (target, error) in [
            ("http://example.com/a.png", TargetError::NotRelative),
            ("file:///C:/a.png", TargetError::NotRelative),
            ("C:/a.png", TargetError::NotRelative),
            ("c:a.png", TargetError::NotRelative),
            ("//server/share/a.png", TargetError::NotRelative),
            ("", TargetError::Empty),
            ("#bookmark", TargetError::Empty),
            ("?query", TargetError::Empty),
            (
                "a b.png",
                TargetError::InvalidPartName(PartNameError::InvalidCharacter),
            ),
            (
                "media\\a.png",
                TargetError::InvalidPartName(PartNameError::InvalidCharacter),
            ),
            (
                "media/",
                TargetError::InvalidPartName(PartNameError::EmptySegment),
            ),
            (
                "..",
                TargetError::InvalidPartName(PartNameError::EmptySegment),
            ),
        ] {
            assert_eq!(document.resolve(target).unwrap_err(), error, "{target:?}");
        }
    }

    #[test]
    fn removes_dot_segments_like_rfc_3986() {
        // RFC 3986 §5.2.4's own examples.
        assert_eq!(remove_dot_segments("/a/b/c/./../../g"), "/a/g");
        assert_eq!(remove_dot_segments("mid/content=5/../6"), "mid/6");
        assert_eq!(remove_dot_segments("/.."), "/");
        assert_eq!(remove_dot_segments("/a/.."), "/");
        assert_eq!(remove_dot_segments("/a/."), "/a/");
        assert_eq!(remove_dot_segments("/a/b/"), "/a/b/");
    }
}
