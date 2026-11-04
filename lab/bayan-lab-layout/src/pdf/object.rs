//! PDF objects (ISO 32000-2 §7.3) and their parser.

use std::collections::{BTreeMap, BTreeSet};

use super::lexer::{Lexer, Token};
use super::{Error, Limit};

/// A reference to an indirect object: its object number and generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Ref {
    /// The object number.
    pub(crate) number: u32,
    /// The generation number.
    pub(crate) generation: u16,
}

/// A stream's dictionary and where its data lies in the file.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stream {
    /// The stream dictionary.
    pub(crate) dict: Dict,
    /// The offset of the first byte of its data in the file.
    pub(crate) start: usize,
    /// The number of bytes of its data, from its `Length`.
    pub(crate) length: usize,
}

/// A dictionary: names to values. A `null` value is the same as no entry (§7.3.7), so none is kept.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dict {
    entries: BTreeMap<Vec<u8>, Object>,
}

impl Dict {
    /// The value of `key`, if it has one.
    pub(crate) fn get(&self, key: &[u8]) -> Option<&Object> {
        self.entries.get(key)
    }

    /// Whether the dictionary has `key`.
    pub(crate) fn contains(&self, key: &[u8]) -> bool {
        self.entries.contains_key(key)
    }

    /// Whether the dictionary's `/Type` (or another name-valued key) is `name`.
    pub(crate) fn has_name(&self, key: &[u8], name: &[u8]) -> bool {
        matches!(self.get(key), Some(Object::Name(value)) if value == name)
    }

    /// Sets `key` to `value`; a null value removes the entry (§7.3.7).
    pub(crate) fn set(&mut self, key: Vec<u8>, value: Object) {
        if matches!(value, Object::Null) {
            self.entries.remove(&key);
        } else {
            self.entries.insert(key, value);
        }
    }
}

/// A PDF object.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Object {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Integer(i64),
    /// A real number.
    Real(f64),
    /// A string, decoded from its literal or hexadecimal form.
    String(Vec<u8>),
    /// A name, without its `/`.
    Name(Vec<u8>),
    /// An array.
    Array(Vec<Object>),
    /// A dictionary.
    Dict(Dict),
    /// A reference to an indirect object.
    Ref(Ref),
    /// A stream (only ever an indirect object).
    Stream(Stream),
}

impl Object {
    /// The integer, if this is one.
    pub(crate) const fn as_integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            _ => None,
        }
    }

    /// The number, integer or real, as an `f64`; `None` for other objects and for integers outside PDF's 32-bit range.
    pub(crate) fn as_number(&self) -> Option<f64> {
        match self {
            Self::Integer(value) => i32::try_from(*value).ok().map(f64::from),
            Self::Real(value) => Some(*value),
            _ => None,
        }
    }

    /// The name, if this is one.
    pub(crate) fn as_name(&self) -> Option<&[u8]> {
        match self {
            Self::Name(name) => Some(name),
            _ => None,
        }
    }

    /// The array, if this is one.
    pub(crate) fn as_array(&self) -> Option<&[Self]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The dictionary, if this is one, or the dictionary of a stream.
    pub(crate) const fn as_dict(&self) -> Option<&Dict> {
        match self {
            Self::Dict(dict) => Some(dict),
            Self::Stream(stream) => Some(&stream.dict),
            _ => None,
        }
    }

    /// The stream, if this is one.
    pub(crate) const fn as_stream(&self) -> Option<&Stream> {
        match self {
            Self::Stream(stream) => Some(stream),
            _ => None,
        }
    }
}

/// Parses one object from `lexer`, which must not be at the end. With `references`, `n g R` is a reference; content streams have none.
///
/// # Errors
///
/// [`Error::Malformed`] if the tokens form no object, and [`Error::Limit`] with [`Limit::Nesting`] if arrays and dictionaries nest deeper than `max_nesting`.
pub(crate) fn parse_object(
    lexer: &mut Lexer<'_>,
    references: bool,
    max_nesting: usize,
) -> Result<Object, Error> {
    let start = lexer.position();
    let Some(token) = lexer.next_token()? else {
        return Err(Error::malformed(format!(
            "an object is missing at byte {start}"
        )));
    };
    parse_from(token, lexer, references, max_nesting, 0)
}

/// Parses the object that starts with `token`.
pub(crate) fn parse_from(
    token: Token<'_>,
    lexer: &mut Lexer<'_>,
    references: bool,
    max_nesting: usize,
    depth: usize,
) -> Result<Object, Error> {
    match token {
        Token::Integer(value) => {
            if references && let Some(reference) = reference_after(value, lexer)? {
                return Ok(Object::Ref(reference));
            }
            Ok(Object::Integer(value))
        }
        Token::Real(value) => Ok(Object::Real(value)),
        Token::Name(name) => Ok(Object::Name(name)),
        Token::String(bytes) => Ok(Object::String(bytes)),
        Token::Keyword(b"true") => Ok(Object::Bool(true)),
        Token::Keyword(b"false") => Ok(Object::Bool(false)),
        Token::Keyword(b"null") => Ok(Object::Null),
        Token::ArrayStart => {
            if depth >= max_nesting {
                return Err(Error::Limit(Limit::Nesting));
            }
            let mut items = Vec::new();
            loop {
                let at = lexer.position();
                match lexer.next_token()? {
                    Some(Token::ArrayEnd) => break,
                    Some(token) => {
                        items.push(parse_from(
                            token,
                            lexer,
                            references,
                            max_nesting,
                            depth + 1,
                        )?);
                    }
                    None => {
                        return Err(Error::malformed(format!(
                            "an array does not end (at byte {at})"
                        )));
                    }
                }
            }
            Ok(Object::Array(items))
        }
        Token::DictStart => {
            if depth >= max_nesting {
                return Err(Error::Limit(Limit::Nesting));
            }
            parse_dict(lexer, references, max_nesting, depth).map(Object::Dict)
        }
        Token::ArrayEnd | Token::DictEnd | Token::Keyword(_) => Err(Error::malformed(format!(
            "an unexpected token before byte {}",
            lexer.position()
        ))),
    }
}

/// The entries of a dictionary after its `<<`.
fn parse_dict(
    lexer: &mut Lexer<'_>,
    references: bool,
    max_nesting: usize,
    depth: usize,
) -> Result<Dict, Error> {
    let mut entries = BTreeMap::new();
    let mut keys = BTreeSet::new();
    loop {
        let at = lexer.position();
        let key = match lexer.next_token()? {
            Some(Token::DictEnd) => break,
            Some(Token::Name(key)) => key,
            Some(_) => {
                return Err(Error::malformed(format!(
                    "a dictionary key is not a name (before byte {})",
                    lexer.position()
                )));
            }
            None => {
                return Err(Error::malformed(format!(
                    "a dictionary does not end (at byte {at})"
                )));
            }
        };
        let Some(token) = lexer.next_token()? else {
            return Err(Error::malformed(format!(
                "a dictionary entry has no value (at byte {at})"
            )));
        };
        let value = parse_from(token, lexer, references, max_nesting, depth + 1)?;
        // A key given twice has no defined meaning; refuse it rather than pick one.
        if !keys.insert(key.clone()) {
            return Err(Error::malformed(format!(
                "a dictionary has the same key twice (before byte {})",
                lexer.position()
            )));
        }
        if !matches!(value, Object::Null) {
            entries.insert(key, value);
        }
    }
    Ok(Dict { entries })
}

/// If the tokens after the integer `number` are `generation R`, the reference; otherwise the lexer stays where it was.
fn reference_after(number: i64, lexer: &mut Lexer<'_>) -> Result<Option<Ref>, Error> {
    let saved = lexer.position();
    let generation = match lexer.next_token() {
        Ok(Some(Token::Integer(generation))) => generation,
        _ => {
            lexer.seek(saved);
            return Ok(None);
        }
    };
    if !lexer.eat_keyword(b"R") {
        lexer.seek(saved);
        return Ok(None);
    }
    match (u32::try_from(number), u16::try_from(generation)) {
        (Ok(number), Ok(generation)) if number > 0 => Ok(Some(Ref { number, generation })),
        _ => Err(Error::malformed(format!(
            "a reference before byte {} has an object or generation number out of range",
            lexer.position()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &[u8]) -> Result<Object, Error> {
        parse_object(&mut Lexer::new(text, 0), true, 8)
    }

    #[test]
    fn parses_nested_objects_and_references() {
        let object = parse(b"<< /Type /Page /Kids [1 0 R 2 0 R] /Count 2 /Box [0 0 612.5 792] /Empty null /Flag true >>").unwrap();
        let dict = object.as_dict().unwrap();
        assert!(dict.has_name(b"Type", b"Page"));
        assert_eq!(
            dict.get(b"Kids").unwrap().as_array().unwrap(),
            [
                Object::Ref(Ref {
                    number: 1,
                    generation: 0
                }),
                Object::Ref(Ref {
                    number: 2,
                    generation: 0
                })
            ]
        );
        assert_eq!(dict.get(b"Count").unwrap().as_integer(), Some(2));
        assert_eq!(
            dict.get(b"Box").unwrap().as_array().unwrap()[2].as_number(),
            Some(612.5)
        );
        assert!(!dict.contains(b"Empty"));
        assert_eq!(dict.get(b"Flag"), Some(&Object::Bool(true)));
        // Without references, `1 0 R` is not one.
        assert_eq!(
            parse_object(&mut Lexer::new(b"[1 0 R]", 0), false, 8).unwrap_err(),
            Error::malformed("an unexpected token before byte 6".to_owned())
        );
        // An integer followed by another integer is not a reference.
        assert_eq!(
            parse(b"[1 2 3]").unwrap().as_array().unwrap(),
            [Object::Integer(1), Object::Integer(2), Object::Integer(3)]
        );
    }

    #[test]
    fn refuses_duplicate_keys_and_bad_dictionaries() {
        for text in [
            &b"<< /A 1 /A 2 >>"[..],
            b"<< /A null /A 2 >>",
            b"<< /A 1 /B >>",
            b"<< 1 2 >>",
            b"<< /A 1",
            b"[1 2",
            b"0 0 R",
            b"1 70000 R",
            b"]",
        ] {
            assert!(parse(text).is_err(), "{}", String::from_utf8_lossy(text));
        }
    }

    #[test]
    fn limits_nesting() {
        let fits = [b"[".repeat(8), b"]".repeat(8)].concat();
        assert!(parse(&fits).is_ok());
        let too_deep = [b"[".repeat(9), b"]".repeat(9)].concat();
        assert_eq!(parse(&too_deep).unwrap_err(), Error::Limit(Limit::Nesting));
        let dicts = [b"<</A ".repeat(9), b">>".repeat(9)].concat();
        assert_eq!(parse(&dicts).unwrap_err(), Error::Limit(Limit::Nesting));
    }
}
