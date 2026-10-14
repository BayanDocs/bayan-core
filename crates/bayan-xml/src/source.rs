//! Decoding a part's bytes into the text that the reader reads and that every offset refers to.

use std::borrow::Cow;
use std::ops::Range;

use crate::chars::{find_invalid_char, is_space};
use crate::{Error, ErrorKind, Limits};

/// The character encoding of a part, kept so that a caller can write a changed part back in the same encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// UTF-8, with or without a byte order mark.
    Utf8 {
        /// Whether the part starts with a byte order mark.
        bom: bool,
    },
    /// UTF-16, little-endian, with a byte order mark.
    Utf16Le,
    /// UTF-16, big-endian, with a byte order mark.
    Utf16Be,
}

/// The XML declaration at the start of a part (`<?xml version="1.0" encoding="UTF-8" standalone="yes"?>`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    span: Range<usize>,
    declares_encoding: bool,
    standalone: Option<bool>,
}

impl Declaration {
    /// Where the declaration is in the text, from `<?xml` to `?>`.
    pub fn span(&self) -> Range<usize> {
        self.span.clone()
    }

    /// Whether the declaration names an encoding.
    pub fn declares_encoding(&self) -> bool {
        self.declares_encoding
    }

    /// The `standalone` pseudo-attribute: `Some(true)` for `yes`, `Some(false)` for `no`, `None` without one.
    pub fn standalone(&self) -> Option<bool> {
        self.standalone
    }
}

/// A part's XML, decoded and checked: valid UTF-8 or UTF-16, no character that XML does not allow, within [`Limits::max_size`], and with a well-formed XML declaration if it has one.
///
/// Every offset and span that bayan-xml reports refers to [`Source::text`]: the text after the byte order mark, if there is one, and, for UTF-16, the text converted to UTF-8.
#[derive(Clone, Debug)]
pub struct Source<'a> {
    text: Cow<'a, str>,
    encoding: Encoding,
    declaration: Option<Declaration>,
}

impl<'a> Source<'a> {
    /// Decodes and checks `bytes`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TooLarge`] if there are more than [`Limits::max_size`] bytes; [`ErrorKind::InvalidEncoding`] if they are not UTF-8, or not UTF-16 after a UTF-16 byte order mark; [`ErrorKind::InvalidCharacter`] for a character that XML does not allow; [`ErrorKind::MalformedDeclaration`], [`ErrorKind::UnexpectedEnd`] or [`ErrorKind::UnsupportedEncoding`] for an XML declaration that is malformed, cut short, or names an encoding other than the bytes'.
    pub fn new(bytes: &'a [u8], limits: &Limits) -> Result<Source<'a>, Error> {
        if bytes.len() > limits.max_size {
            return Err(Error::new(0, ErrorKind::TooLarge));
        }
        let (text, encoding) = decode(bytes)?;
        if let Some(offset) = find_invalid_char(&text) {
            return Err(Error::new(offset, ErrorKind::InvalidCharacter));
        }
        let declaration = declaration(&text, encoding)?;
        Ok(Source {
            text,
            encoding,
            declaration,
        })
    }

    /// The decoded text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The encoding of the bytes.
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// The XML declaration, if the text starts with one.
    pub fn declaration(&self) -> Option<&Declaration> {
        self.declaration.as_ref()
    }
}

/// Decodes the bytes, after a byte order mark if there is one: UTF-8 is borrowed as it is, UTF-16 converted to UTF-8.
fn decode(bytes: &[u8]) -> Result<(Cow<'_, str>, Encoding), Error> {
    let utf16 = |rest: &[u8], encoding, unit: fn([u8; 2]) -> u16| {
        let (pairs, odd) = rest.as_chunks::<2>();
        if !odd.is_empty() {
            return Err(Error::new(0, ErrorKind::InvalidEncoding));
        }
        let mut text = String::with_capacity(rest.len());
        for character in char::decode_utf16(pairs.iter().map(|&pair| unit(pair))) {
            // The offset of a decoding error is that of the text decoded so far, where the bad character would have gone.
            let character =
                character.map_err(|_| Error::new(text.len(), ErrorKind::InvalidEncoding))?;
            text.push(character);
        }
        Ok((Cow::Owned(text), encoding))
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, Encoding::Utf16Le, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, Encoding::Utf16Be, u16::from_be_bytes);
    }
    let (rest, bom) = match bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    let text = core::str::from_utf8(rest)
        .map_err(|error| Error::new(error.valid_up_to(), ErrorKind::InvalidEncoding))?;
    Ok((Cow::Borrowed(text), Encoding::Utf8 { bom }))
}

/// Reads the XML declaration at the start of `text`, if there is one: `version="1.0"`, then optionally `encoding`, which must name the encoding of the bytes, then optionally `standalone`, in this order (XML 1.0 §2.8).
fn declaration(text: &str, encoding: Encoding) -> Result<Option<Declaration>, Error> {
    let Some(rest) = text.strip_prefix("<?xml") else {
        return Ok(None);
    };
    // `<?xml-stylesheet …?>` and the like are processing instructions, not declarations.
    if rest
        .bytes()
        .next()
        .is_some_and(|byte| byte != b'?' && !is_space(byte))
    {
        return Ok(None);
    }
    let malformed = |offset| Error::new(offset, ErrorKind::MalformedDeclaration);
    let end = |offset| Error::new(offset, ErrorKind::UnexpectedEnd);
    let bytes = text.as_bytes();
    let mut position = "<?xml".len();
    let mut fields = Vec::new();
    loop {
        let start = position;
        while bytes.get(position).copied().is_some_and(is_space) {
            position += 1;
        }
        match bytes.get(position..position + 2) {
            Some(b"?>") => {
                position += 2;
                break;
            }
            None => return Err(end(text.len())),
            Some(_) => {}
        }
        if position == start {
            return Err(malformed(position));
        }
        let name_start = position;
        while bytes.get(position).is_some_and(u8::is_ascii_alphabetic) {
            position += 1;
        }
        let name = &text[name_start..position];
        while bytes.get(position).copied().is_some_and(is_space) {
            position += 1;
        }
        match bytes.get(position) {
            Some(b'=') => position += 1,
            Some(_) => return Err(malformed(position)),
            None => return Err(end(text.len())),
        }
        while bytes.get(position).copied().is_some_and(is_space) {
            position += 1;
        }
        let quote = match bytes.get(position) {
            Some(&quote @ (b'"' | b'\'')) => quote,
            Some(_) => return Err(malformed(position)),
            None => return Err(end(text.len())),
        };
        position += 1;
        // Every value the declaration may hold (`1.0`, an encoding name, `yes` or `no`) is made of these characters (XML 1.0 productions [26] and [81]).
        let value_start = position;
        while bytes
            .get(position)
            .is_some_and(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            position += 1;
        }
        match bytes.get(position) {
            Some(&byte) if byte == quote => {}
            Some(_) => return Err(malformed(position)),
            None => return Err(end(text.len())),
        }
        fields.push((name, &text[value_start..position], value_start));
        position += 1;
    }
    let mut fields = fields.into_iter().peekable();
    match fields.next() {
        Some(("version", "1.0", _)) => {}
        Some((_, _, offset)) => return Err(malformed(offset)),
        None => return Err(malformed(0)),
    }
    let mut declares_encoding = false;
    if let Some(&("encoding", name, offset)) = fields.peek() {
        fields.next();
        let agrees = match encoding {
            Encoding::Utf8 { .. } => name.eq_ignore_ascii_case("UTF-8"),
            Encoding::Utf16Le | Encoding::Utf16Be => name.eq_ignore_ascii_case("UTF-16"),
        };
        if !agrees {
            return Err(Error::new(offset, ErrorKind::UnsupportedEncoding));
        }
        declares_encoding = true;
    }
    let mut standalone = None;
    if let Some(&("standalone", value, offset)) = fields.peek() {
        fields.next();
        standalone = Some(match value {
            "yes" => true,
            "no" => false,
            _ => return Err(malformed(offset)),
        });
    }
    if let Some((_, _, offset)) = fields.next() {
        return Err(malformed(offset));
    }
    Ok(Some(Declaration {
        span: 0..position,
        declares_encoding,
        standalone,
    }))
}
