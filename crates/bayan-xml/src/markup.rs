//! The grammar inside the pieces that quick-xml finds: start and end tags with their attributes, references, and line ends. quick-xml finds where each piece starts and ends; everything inside is checked here, strictly, against XML 1.0 (Fifth Edition) and Namespaces in XML 1.0 (Third Edition).

use std::borrow::Cow;
use std::ops::Range;

use crate::chars::{is_char, is_name, is_name_char, is_space, split_qname};
use crate::event::AttributeData;
use crate::{Error, ErrorKind, Limits};

/// The name of a start tag.
pub(crate) struct TagName<'a> {
    pub(crate) qname: &'a str,
    pub(crate) prefix: &'a str,
    pub(crate) local: &'a str,
    pub(crate) offset: usize,
}

/// The end of the run of name characters that starts at `start`, at most at `end`.
fn scan_name(text: &str, start: usize, end: usize) -> usize {
    text.get(start..end).map_or(start, |rest| {
        start
            + rest
                .char_indices()
                .find(|&(_, character)| !is_name_char(character))
                .map_or(rest.len(), |(offset, _)| offset)
    })
}

/// Checks a qualified name at `offset`: not too long, and a valid `QName`.
fn qualified_name<'a>(
    name: &'a str,
    offset: usize,
    limits: &Limits,
) -> Result<(&'a str, &'a str), Error> {
    if name.len() > limits.max_name_length {
        return Err(Error::new(offset, ErrorKind::NameTooLong));
    }
    split_qname(name).ok_or(Error::new(offset, ErrorKind::InvalidName))
}

/// Skips white space from `position` up to `end`.
fn skip_space(bytes: &[u8], mut position: usize, end: usize) -> usize {
    while position < end && bytes.get(position).copied().is_some_and(is_space) {
        position += 1;
    }
    position
}

/// Parses the start tag or empty-element tag at `span` (from `<` to `>`), putting its attributes into `attributes`. Returns its name and whether it is an empty-element tag.
pub(crate) fn start_tag<'a>(
    text: &'a str,
    span: &Range<usize>,
    limits: &Limits,
    attributes: &mut Vec<AttributeData<'a>>,
) -> Result<(TagName<'a>, bool), Error> {
    attributes.clear();
    let bytes = text.as_bytes();
    // quick-xml hands over only tags that start with `<` and end with `>`.
    let close = span.end.saturating_sub(1);
    let name_start = span.start + 1;
    let name_end = scan_name(text, name_start, close);
    let qname = text.get(name_start..name_end).unwrap_or_default();
    let (prefix, local) = qualified_name(qname, name_start, limits)?;
    let name = TagName {
        qname,
        prefix,
        local,
        offset: name_start,
    };
    let mut position = name_end;
    loop {
        let space_start = position;
        position = skip_space(bytes, position, close);
        if position >= close {
            return Ok((name, false));
        }
        if bytes.get(position) == Some(&b'/') {
            return if position + 1 == close {
                Ok((name, true))
            } else {
                Err(Error::new(position, ErrorKind::MalformedTag))
            };
        }
        if position == space_start {
            return Err(Error::new(position, ErrorKind::MalformedTag));
        }
        let attribute_start = position;
        let attribute_end = scan_name(text, position, close);
        let attribute_name = text.get(position..attribute_end).unwrap_or_default();
        let (prefix, local) = qualified_name(attribute_name, attribute_start, limits)?;
        if attributes.len() >= limits.max_attributes {
            return Err(Error::new(attribute_start, ErrorKind::TooManyAttributes));
        }
        position = skip_space(bytes, attribute_end, close);
        if position >= close || bytes.get(position) != Some(&b'=') {
            return Err(Error::new(position, ErrorKind::MalformedTag));
        }
        position = skip_space(bytes, position + 1, close);
        let quote = match bytes.get(position) {
            Some(&quote @ (b'"' | b'\'')) if position < close => quote,
            _ => return Err(Error::new(position, ErrorKind::MalformedTag)),
        };
        let value_start = position + 1;
        let Some(length) = bytes
            .get(value_start..close)
            .and_then(|rest| rest.iter().position(|&byte| byte == quote))
        else {
            return Err(Error::new(position, ErrorKind::MalformedTag));
        };
        let value_end = value_start + length;
        let value = attribute_value(text, value_start..value_end, limits)?;
        attributes.push(AttributeData {
            prefix,
            local,
            namespace: None,
            raw: text.get(value_start..value_end).unwrap_or_default(),
            value,
            span: attribute_start..value_end + 1,
            value_span: value_start..value_end,
            declaration: attribute_name == "xmlns" || prefix == "xmlns",
        });
        position = value_end + 1;
    }
}

/// Parses the end tag at `span` (from `</` to `>`) and returns its name as written.
pub(crate) fn end_tag<'a>(text: &'a str, span: &Range<usize>) -> Result<&'a str, Error> {
    let close = span.end.saturating_sub(1);
    let name_start = span.start + 2;
    let name_end = scan_name(text, name_start, close);
    let name = text.get(name_start..name_end).unwrap_or_default();
    if name.is_empty() {
        return Err(Error::new(name_start, ErrorKind::InvalidName));
    }
    if skip_space(text.as_bytes(), name_end, close) != close {
        return Err(Error::new(span.start, ErrorKind::MalformedTag));
    }
    Ok(name)
}

/// The value of the attribute written at `range` (between the quotes): references resolved and white space normalized (XML 1.0 §3.3.3), borrowed from `text` when nothing changes.
pub(crate) fn attribute_value<'a>(
    text: &'a str,
    range: Range<usize>,
    limits: &Limits,
) -> Result<Cow<'a, str>, Error> {
    let raw = text.get(range.clone()).unwrap_or_default();
    let special = |byte: u8| matches!(byte, b'&' | b'<' | b'\t' | b'\n' | b'\r');
    if !raw.bytes().any(special) {
        return Ok(Cow::Borrowed(raw));
    }
    let bytes = raw.as_bytes();
    let mut value = String::with_capacity(raw.len());
    let mut position = 0;
    while let Some(&byte) = bytes.get(position) {
        match byte {
            b'<' => {
                return Err(Error::new(
                    range.start + position,
                    ErrorKind::LessThanInAttribute,
                ));
            }
            b'&' => {
                let offset = range.start + position;
                let Some(length) = bytes
                    .get(position + 1..)
                    .and_then(|rest| rest.iter().position(|&byte| byte == b';'))
                else {
                    return Err(Error::new(offset, ErrorKind::MalformedReference));
                };
                let name = raw
                    .get(position + 1..position + 1 + length)
                    .unwrap_or_default();
                value.push(reference(name, offset, limits)?);
                position += length + 2;
            }
            b'\r' => {
                value.push(' ');
                position += if bytes.get(position + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
            }
            b'\t' | b'\n' => {
                value.push(' ');
                position += 1;
            }
            _ => {
                let run = bytes
                    .get(position..)
                    .and_then(|rest| rest.iter().position(|&byte| special(byte)))
                    .map_or(raw.len(), |length| position + length);
                value.push_str(raw.get(position..run).unwrap_or_default());
                position = run;
            }
        }
    }
    Ok(Cow::Owned(value))
}

/// The character that the reference `&name;` at `offset` stands for: one of the five predefined entities or a character reference. Every reference stands for exactly one character, which is never longer in UTF-8 than the reference itself, so resolving references never makes a text longer: entity expansion is impossible by construction.
pub(crate) fn reference(name: &str, offset: usize, limits: &Limits) -> Result<char, Error> {
    let malformed = Error::new(offset, ErrorKind::MalformedReference);
    if let Some(number) = name.strip_prefix('#') {
        let (digits, radix) = match number.strip_prefix('x') {
            Some(hex) => (hex, 16),
            None => (number, 10),
        };
        if digits.is_empty() || !digits.bytes().all(|byte| char::from(byte).is_digit(radix)) {
            return Err(malformed);
        }
        let invalid = Error::new(offset, ErrorKind::InvalidCharacterReference);
        let mut value: u32 = 0;
        for byte in digits.bytes() {
            let digit = char::from(byte).to_digit(radix).ok_or(malformed)?;
            value = value
                .checked_mul(radix)
                .and_then(|value| value.checked_add(digit))
                .ok_or(invalid)?;
        }
        return char::from_u32(value)
            .filter(|&character| is_char(character))
            .ok_or(invalid);
    }
    if name.len() > limits.max_name_length {
        return Err(Error::new(offset, ErrorKind::NameTooLong));
    }
    match name {
        "amp" => Ok('&'),
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "apos" => Ok('\''),
        "quot" => Ok('"'),
        _ if is_name(name) => Err(Error::new(offset, ErrorKind::UndefinedEntity)),
        _ => Err(malformed),
    }
}

/// Appends `raw` to `out` with its line ends normalized (XML 1.0 §2.11): CR LF and a lone CR become LF.
pub(crate) fn push_normalized_line_ends(raw: &str, out: &mut String) {
    let mut rest = raw;
    while let Some(position) = rest.find('\r') {
        out.push_str(rest.get(..position).unwrap_or_default());
        out.push('\n');
        let skip = if rest.as_bytes().get(position + 1) == Some(&b'\n') {
            2
        } else {
            1
        };
        rest = rest.get(position + skip..).unwrap_or_default();
    }
    out.push_str(rest);
}

/// `raw` with its line ends normalized, borrowed when it has no carriage return.
pub(crate) fn normalized_line_ends(raw: &str) -> Cow<'_, str> {
    if raw.contains('\r') {
        let mut out = String::with_capacity(raw.len());
        push_normalized_line_ends(raw, &mut out);
        Cow::Owned(out)
    } else {
        Cow::Borrowed(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_references() {
        let limits = Limits::default();
        assert_eq!(reference("amp", 0, &limits), Ok('&'));
        assert_eq!(reference("#65", 0, &limits), Ok('A'));
        assert_eq!(reference("#x1F600", 0, &limits), Ok('\u{1F600}'));
        assert_eq!(
            reference("#X41", 7, &limits),
            Err(Error::new(7, ErrorKind::MalformedReference))
        );
        assert_eq!(
            reference("#4294967296", 7, &limits),
            Err(Error::new(7, ErrorKind::InvalidCharacterReference))
        );
        assert_eq!(
            reference("nbsp", 7, &limits),
            Err(Error::new(7, ErrorKind::UndefinedEntity))
        );
        assert_eq!(
            reference("a b", 7, &limits),
            Err(Error::new(7, ErrorKind::MalformedReference))
        );
    }

    #[test]
    fn normalizes_line_ends() {
        assert_eq!(normalized_line_ends("a\r\nb\rc\n\r\r\nd"), "a\nb\nc\n\n\nd");
        assert!(matches!(normalized_line_ends("a\nb"), Cow::Borrowed(_)));
    }
}
