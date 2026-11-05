//! PDF objects (ISO 32000-2 §7.3) and their parser.

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
///
/// The entries are a vector sorted by name, found by binary search: an entry costs its key and its value and nothing more, which matters because the file's objects stay in memory until extraction ends (a tree would give every small dictionary a node for eleven entries).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Dict {
    entries: Vec<(Vec<u8>, Object)>,
}

impl Dict {
    /// The dictionary of `entries`, in any order; `None` if a key appears twice, which has no defined meaning, even when one of its values is null.
    pub(crate) fn from_entries(mut entries: Vec<(Vec<u8>, Object)>) -> Option<Self> {
        entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        if entries
            .windows(2)
            .any(|pair| matches!(pair, [(left, _), (right, _)] if left == right))
        {
            return None;
        }
        entries.retain(|(_, value)| !matches!(value, Object::Null));
        entries.shrink_to_fit();
        Some(Self { entries })
    }

    /// The value of `key`, if it has one.
    pub(crate) fn get(&self, key: &[u8]) -> Option<&Object> {
        self.entries
            .binary_search_by(|(name, _)| name.as_slice().cmp(key))
            .ok()
            .and_then(|index| self.entries.get(index))
            .map(|(_, value)| value)
    }

    /// Whether the dictionary has `key`.
    pub(crate) fn contains(&self, key: &[u8]) -> bool {
        self.get(key).is_some()
    }

    /// Whether the dictionary's `/Type` (or another name-valued key) is `name`.
    pub(crate) fn has_name(&self, key: &[u8], name: &[u8]) -> bool {
        matches!(self.get(key), Some(Object::Name(value)) if value == name)
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

/// How many more values the parser may make, and the limit that stops it: every object counts one, and so does, at any depth, every item of an array and every value of a dictionary.
///
/// The file's objects ([`Values::kept`]) draw on one count for the whole extraction, [`super::Limits::max_object_values`], because they stay in memory; the operands that wait for one content-stream operator ([`Values::operands`]) draw on a fresh count of [`super::Limits::max_operand_values`] each.
#[derive(Debug)]
pub(crate) struct Values {
    remaining: usize,
    limit: Limit,
    /// Whether the parsed vectors are trimmed to their length, since the objects stay in memory.
    kept: bool,
}

impl Values {
    /// The count for the file's objects, which stay in memory until extraction ends.
    pub(crate) const fn kept(count: usize) -> Self {
        Self {
            remaining: count,
            limit: Limit::ObjectValues,
            kept: true,
        }
    }

    /// The count for the operands of one content-stream operator, or the entries of one inline image's dictionary.
    pub(crate) const fn operands(count: usize) -> Self {
        Self {
            remaining: count,
            limit: Limit::OperandValues,
            kept: false,
        }
    }

    /// Takes one value from the count.
    fn take(&mut self) -> Result<(), Error> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(Error::Limit(self.limit))?;
        Ok(())
    }

    /// Trims a vector that stays in memory to its length.
    fn trim<T>(&self, items: &mut Vec<T>) {
        if self.kept {
            items.shrink_to_fit();
        }
    }
}

/// Parses one object from `lexer`, which must not be at the end. With `references`, `n g R` is a reference; content streams have none.
///
/// # Errors
///
/// [`Error::Malformed`] if the tokens form no object, and [`Error::Limit`] with [`Limit::Nesting`] if arrays and dictionaries nest deeper than `max_nesting`, or with the limit of `values` when the object holds more values than it allows.
pub(crate) fn parse_object(
    lexer: &mut Lexer<'_>,
    references: bool,
    max_nesting: usize,
    values: &mut Values,
) -> Result<Object, Error> {
    let start = lexer.position();
    let Some(token) = lexer.next_token()? else {
        return Err(Error::malformed(format!(
            "an object is missing at byte {start}"
        )));
    };
    parse_from(token, lexer, references, max_nesting, 0, values)
}

/// Parses the object that starts with `token`.
pub(crate) fn parse_from(
    token: Token<'_>,
    lexer: &mut Lexer<'_>,
    references: bool,
    max_nesting: usize,
    depth: usize,
    values: &mut Values,
) -> Result<Object, Error> {
    values.take()?;
    match token {
        Token::Integer(value) => {
            if references && let Some(reference) = reference_after(value, lexer)? {
                return Ok(Object::Ref(reference));
            }
            Ok(Object::Integer(value))
        }
        Token::Real(value) => Ok(Object::Real(value)),
        Token::Name(mut name) => {
            values.trim(&mut name);
            Ok(Object::Name(name))
        }
        Token::String(mut bytes) => {
            values.trim(&mut bytes);
            Ok(Object::String(bytes))
        }
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
                            values,
                        )?);
                    }
                    None => {
                        return Err(Error::malformed(format!(
                            "an array does not end (at byte {at})"
                        )));
                    }
                }
            }
            values.trim(&mut items);
            Ok(Object::Array(items))
        }
        Token::DictStart => {
            if depth >= max_nesting {
                return Err(Error::Limit(Limit::Nesting));
            }
            parse_dict(lexer, references, max_nesting, depth, values).map(Object::Dict)
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
    values: &mut Values,
) -> Result<Dict, Error> {
    let mut entries = Vec::new();
    loop {
        let at = lexer.position();
        let mut key = match lexer.next_token()? {
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
        let value = parse_from(token, lexer, references, max_nesting, depth + 1, values)?;
        values.trim(&mut key);
        entries.push((key, value));
    }
    // A key given twice has no defined meaning; refuse it rather than pick one.
    Dict::from_entries(entries).ok_or_else(|| {
        Error::malformed(format!(
            "a dictionary has the same key twice (before byte {})",
            lexer.position()
        ))
    })
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
        parse_object(&mut Lexer::new(text, 0), true, 8, &mut Values::kept(1000))
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
            parse_object(
                &mut Lexer::new(b"[1 0 R]", 0),
                false,
                8,
                &mut Values::operands(10)
            )
            .unwrap_err(),
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
    fn counts_every_value_against_its_limit() {
        // The array, its two items, the dictionary's one value and that array's two items: six values.
        let text = b"[1 << /A [2 3] >>]";
        let parse_with =
            |values: &mut Values| parse_object(&mut Lexer::new(text, 0), true, 8, values);
        let mut values = Values::kept(6);
        assert!(parse_with(&mut values).is_ok());
        assert_eq!(values.remaining, 0);
        assert_eq!(
            parse_with(&mut Values::kept(5)).unwrap_err(),
            Error::Limit(Limit::ObjectValues)
        );
        assert_eq!(
            parse_with(&mut Values::operands(5)).unwrap_err(),
            Error::Limit(Limit::OperandValues)
        );
        // Objects that stay in memory keep no spare capacity.
        let Object::Array(items) = parse(b"[1 2 3 4 5]").unwrap() else {
            panic!("not an array");
        };
        assert_eq!(items.capacity(), items.len());
    }

    #[test]
    fn dictionaries_are_found_by_name_whatever_the_order() {
        let object = parse(b"<< /Zebra 1 /Apple 2 /Mango null /Kiwi 3 >>").unwrap();
        let dict = object.as_dict().unwrap();
        assert_eq!(dict.get(b"Apple"), Some(&Object::Integer(2)));
        assert_eq!(dict.get(b"Kiwi"), Some(&Object::Integer(3)));
        assert_eq!(dict.get(b"Zebra"), Some(&Object::Integer(1)));
        assert_eq!(dict.get(b"Mango"), None);
        assert_eq!(dict.get(b"Pear"), None);
        assert_eq!(
            Dict::from_entries(vec![
                (b"B".to_vec(), Object::Null),
                (b"A".to_vec(), Object::Integer(1)),
                (b"B".to_vec(), Object::Integer(2)),
            ]),
            None
        );
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
