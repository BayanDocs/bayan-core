//! The tokens of PDF's syntax (ISO 32000-2 §7.2 and §7.3), shared by objects, content streams and CMaps.

use super::{Error, MAX_TOKEN_BYTES};

/// A token.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Token<'a> {
    /// An integer, such as `-12`.
    Integer(i64),
    /// A real number, such as `0.5` (PDF has no exponents).
    Real(f64),
    /// A name, with its `#xx` escapes decoded, without the `/`.
    Name(Vec<u8>),
    /// A literal or hexadecimal string, decoded.
    String(Vec<u8>),
    /// `[`.
    ArrayStart,
    /// `]`.
    ArrayEnd,
    /// `<<`.
    DictStart,
    /// `>>`.
    DictEnd,
    /// Any other run of regular characters: `true`, `obj`, `R`, `stream`, an operator such as `Tj` or `T*`.
    Keyword(&'a [u8]),
}

/// PDF's white-space characters (§7.2.3).
pub(crate) const fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

/// PDF's delimiter characters (§7.2.3).
pub(crate) const fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Characters that are neither white space nor delimiters.
pub(crate) const fn is_regular(byte: u8) -> bool {
    !is_whitespace(byte) && !is_delimiter(byte)
}

/// The value of a hexadecimal digit.
pub(crate) const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Reads tokens from a slice of bytes.
#[derive(Clone, Debug)]
pub(crate) struct Lexer<'a> {
    data: &'a [u8],
    position: usize,
    /// The furthest position the lexer has been at before it last moved back.
    furthest: usize,
}

impl<'a> Lexer<'a> {
    /// A lexer at `position` of `data`.
    pub(crate) const fn new(data: &'a [u8], position: usize) -> Self {
        Self {
            data,
            position,
            furthest: position,
        }
    }

    /// The position of the next byte.
    pub(crate) const fn position(&self) -> usize {
        self.position
    }

    /// The furthest position the lexer has reached, also when it has moved back since, as it does after looking ahead for what was not there: everything before it has been read, some bytes more than once (a few times at most), and the reader counts it as read.
    pub(crate) const fn furthest(&self) -> usize {
        if self.position > self.furthest {
            self.position
        } else {
            self.furthest
        }
    }

    /// Moves to `position`.
    pub(crate) const fn seek(&mut self, position: usize) {
        self.furthest = self.furthest();
        self.position = position;
    }

    /// The bytes the lexer reads.
    pub(crate) const fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Skips white space and comments.
    pub(crate) fn skip_whitespace(&mut self) {
        while let Some(&byte) = self.data.get(self.position) {
            if is_whitespace(byte) {
                self.position += 1;
            } else if byte == b'%' {
                while let Some(&byte) = self.data.get(self.position) {
                    if byte == b'\n' || byte == b'\r' {
                        break;
                    }
                    self.position += 1;
                }
            } else {
                break;
            }
        }
    }

    /// The next token, or `None` at the end of the data.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for bytes that form no token, an unterminated string, or a token longer than [`MAX_TOKEN_BYTES`] other than a string.
    pub(crate) fn next_token(&mut self) -> Result<Option<Token<'a>>, Error> {
        self.skip_whitespace();
        let Some(&byte) = self.data.get(self.position) else {
            return Ok(None);
        };
        let token = match byte {
            b'[' => {
                self.position += 1;
                Token::ArrayStart
            }
            b']' => {
                self.position += 1;
                Token::ArrayEnd
            }
            b'<' if self.data.get(self.position + 1) == Some(&b'<') => {
                self.position += 2;
                Token::DictStart
            }
            b'>' if self.data.get(self.position + 1) == Some(&b'>') => {
                self.position += 2;
                Token::DictEnd
            }
            b'<' => Token::String(self.hex_string()?),
            b'(' => Token::String(self.literal_string()?),
            b'/' => Token::Name(self.name()?),
            b')' | b'>' | b'{' | b'}' => {
                return Err(Error::malformed(format!(
                    "an unexpected delimiter at byte {}",
                    self.position
                )));
            }
            _ => self.regular()?,
        };
        Ok(Some(token))
    }

    /// A run of regular characters: a number or a keyword.
    fn regular(&mut self) -> Result<Token<'a>, Error> {
        let start = self.position;
        while self
            .data
            .get(self.position)
            .is_some_and(|&byte| is_regular(byte))
        {
            self.position += 1;
            if self.position - start > MAX_TOKEN_BYTES {
                return Err(Error::malformed(format!(
                    "a token at byte {start} is longer than {MAX_TOKEN_BYTES} bytes"
                )));
            }
        }
        let run = self.data.get(start..self.position).unwrap_or_default();
        match run.first() {
            Some(b'0'..=b'9' | b'+' | b'-' | b'.') => parse_number(run)
                .ok_or_else(|| Error::malformed(format!("a malformed number at byte {start}"))),
            _ => Ok(Token::Keyword(run)),
        }
    }

    /// A name after its `/` (§7.3.5).
    fn name(&mut self) -> Result<Vec<u8>, Error> {
        let start = self.position;
        self.position += 1;
        let mut name = Vec::new();
        while let Some(&byte) = self.data.get(self.position) {
            if !is_regular(byte) {
                break;
            }
            if byte == b'#' {
                let high = self
                    .data
                    .get(self.position + 1)
                    .copied()
                    .and_then(hex_value);
                let low = self
                    .data
                    .get(self.position + 2)
                    .copied()
                    .and_then(hex_value);
                let (Some(high), Some(low)) = (high, low) else {
                    return Err(Error::malformed(format!(
                        "a name at byte {start} has a `#` without two hexadecimal digits"
                    )));
                };
                name.push((high << 4) | low);
                self.position += 3;
            } else {
                name.push(byte);
                self.position += 1;
            }
            if name.len() > MAX_TOKEN_BYTES {
                return Err(Error::malformed(format!(
                    "a name at byte {start} is longer than {MAX_TOKEN_BYTES} bytes"
                )));
            }
        }
        Ok(name)
    }

    /// A hexadecimal string after its `<` (§7.3.4.3).
    fn hex_string(&mut self) -> Result<Vec<u8>, Error> {
        let start = self.position;
        self.position += 1;
        let mut bytes = Vec::new();
        let mut high: Option<u8> = None;
        loop {
            let Some(&byte) = self.data.get(self.position) else {
                return Err(Error::malformed(format!(
                    "the hexadecimal string at byte {start} does not end"
                )));
            };
            self.position += 1;
            if byte == b'>' {
                break;
            }
            if is_whitespace(byte) {
                continue;
            }
            let Some(value) = hex_value(byte) else {
                return Err(Error::malformed(format!(
                    "the hexadecimal string at byte {start} holds a character that is not a hexadecimal digit"
                )));
            };
            match high.take() {
                Some(first) => bytes.push((first << 4) | value),
                None => high = Some(value),
            }
        }
        // An odd number of digits: the last one is followed by an implied 0.
        if let Some(first) = high {
            bytes.push(first << 4);
        }
        Ok(bytes)
    }

    /// A literal string after its `(` (§7.3.4.2).
    fn literal_string(&mut self) -> Result<Vec<u8>, Error> {
        let start = self.position;
        self.position += 1;
        let mut bytes = Vec::new();
        let mut depth = 0_usize;
        loop {
            let Some(&byte) = self.data.get(self.position) else {
                return Err(Error::malformed(format!(
                    "the string at byte {start} does not end"
                )));
            };
            self.position += 1;
            match byte {
                b'(' => {
                    depth += 1;
                    bytes.push(byte);
                }
                b')' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                    bytes.push(byte);
                }
                b'\\' => self.escape(&mut bytes, start)?,
                b'\r' => {
                    // An end of line in a string is a line feed, whichever way it is written.
                    if self.data.get(self.position) == Some(&b'\n') {
                        self.position += 1;
                    }
                    bytes.push(b'\n');
                }
                _ => bytes.push(byte),
            }
        }
        Ok(bytes)
    }

    /// The escape sequence after a backslash in a literal string.
    fn escape(&mut self, bytes: &mut Vec<u8>, start: usize) -> Result<(), Error> {
        let Some(&byte) = self.data.get(self.position) else {
            return Err(Error::malformed(format!(
                "the string at byte {start} does not end"
            )));
        };
        self.position += 1;
        match byte {
            b'n' => bytes.push(b'\n'),
            b'r' => bytes.push(b'\r'),
            b't' => bytes.push(b'\t'),
            b'b' => bytes.push(0x08),
            b'f' => bytes.push(0x0C),
            b'(' | b')' | b'\\' => bytes.push(byte),
            b'0'..=b'7' => {
                let mut value = u32::from(byte - b'0');
                for _ in 0..2 {
                    match self.data.get(self.position) {
                        Some(&digit @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(digit - b'0');
                            self.position += 1;
                        }
                        _ => break,
                    }
                }
                // A value above 255 keeps its low eight bits (§7.3.4.2: high-order overflow is ignored).
                bytes.push(value.to_le_bytes()[0]);
            }
            // A backslash at the end of a line continues the string on the next line.
            b'\r' => {
                if self.data.get(self.position) == Some(&b'\n') {
                    self.position += 1;
                }
            }
            b'\n' => {}
            // Any other character after a backslash stands for itself; the backslash is ignored.
            other => bytes.push(other),
        }
        Ok(())
    }

    /// Whether the next bytes, after white space and comments, are `keyword` followed by a delimiter, white space or the end; if so, the lexer moves past it.
    pub(crate) fn eat_keyword(&mut self, keyword: &[u8]) -> bool {
        let saved = self.position;
        self.skip_whitespace();
        let end = self.position + keyword.len();
        let matches = self.data.get(self.position..end) == Some(keyword)
            && self.data.get(end).is_none_or(|&byte| !is_regular(byte));
        if matches {
            self.position = end;
        } else {
            self.seek(saved);
        }
        matches
    }
}

/// A number token (§7.3.3): an integer, or a real with a decimal point and no exponent.
fn parse_number(run: &[u8]) -> Option<Token<'static>> {
    let (negative, digits) = match run.split_first() {
        Some((&b'-', rest)) => (true, rest),
        Some((&b'+', rest)) => (false, rest),
        _ => (false, run),
    };
    if digits.is_empty()
        || !digits
            .iter()
            .all(|&byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let points = digits.iter().filter(|&&byte| byte == b'.').count();
    if points > 1 || digits == b"." {
        return None;
    }
    let text = core::str::from_utf8(run).ok()?;
    if points == 0 {
        // Integers beyond an i64 are not PDF integers (whose limit is about 2^31) and are refused.
        return text.parse::<i64>().ok().map(Token::Integer);
    }
    let mut normalized = String::with_capacity(run.len() + 2);
    if negative {
        normalized.push('-');
    }
    let digits = core::str::from_utf8(digits).ok()?;
    if digits.starts_with('.') {
        normalized.push('0');
    }
    normalized.push_str(digits);
    if digits.ends_with('.') {
        normalized.push('0');
    }
    let value: f64 = normalized.parse().ok()?;
    value.is_finite().then_some(Token::Real(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(data: &[u8]) -> Vec<Token<'_>> {
        let mut lexer = Lexer::new(data, 0);
        let mut tokens = Vec::new();
        while let Some(token) = lexer.next_token().unwrap() {
            tokens.push(token);
        }
        tokens
    }

    #[test]
    fn reads_every_kind_of_token() {
        assert_eq!(
            tokens(
                b"[1 -2 +3 0.5 -.25 4. /Name#20x (a(b)c) <414> << >> R Tj T* ' \"] % comment\n/A"
            ),
            vec![
                Token::ArrayStart,
                Token::Integer(1),
                Token::Integer(-2),
                Token::Integer(3),
                Token::Real(0.5),
                Token::Real(-0.25),
                Token::Real(4.0),
                Token::Name(b"Name x".to_vec()),
                Token::String(b"a(b)c".to_vec()),
                Token::String(vec![0x41, 0x40]),
                Token::DictStart,
                Token::DictEnd,
                Token::Keyword(b"R"),
                Token::Keyword(b"Tj"),
                Token::Keyword(b"T*"),
                Token::Keyword(b"'"),
                Token::Keyword(b"\""),
                Token::ArrayEnd,
                Token::Name(b"A".to_vec()),
            ]
        );
    }

    #[test]
    fn decodes_string_escapes_and_line_ends() {
        assert_eq!(
            tokens(b"(a\\n\\r\\t\\b\\f\\(\\)\\\\\\101\\7\\0612\\q)"),
            vec![Token::String(b"a\n\r\t\x08\x0C()\\A\x071\x32q".to_vec())]
        );
        assert_eq!(
            tokens(b"(one\r\ntwo\rthree\\\r\nfour)"),
            vec![Token::String(b"one\ntwo\nthreefour".to_vec())]
        );
        assert_eq!(tokens(b"<61 6 >"), vec![Token::String(b"a`".to_vec())]);
    }

    #[test]
    fn refuses_what_is_not_a_token() {
        for data in [
            &b"(unterminated"[..],
            b"<41",
            b"<4G>",
            b"1.2.3",
            b"12abc",
            b"-",
            b".",
            b"99999999999999999999",
            b"/A#4",
            b")",
            b"{",
        ] {
            let mut lexer = Lexer::new(data, 0);
            assert!(lexer.next_token().is_err(), "{data:?}");
        }
        let long = vec![b'a'; MAX_TOKEN_BYTES + 1];
        assert!(Lexer::new(&long, 0).next_token().is_err());
        let mut long_name = vec![b'/'];
        long_name.extend(vec![b'a'; MAX_TOKEN_BYTES + 1]);
        assert!(Lexer::new(&long_name, 0).next_token().is_err());
        let fits = vec![b'a'; MAX_TOKEN_BYTES];
        assert!(Lexer::new(&fits, 0).next_token().is_ok());
    }

    #[test]
    fn finds_keywords_only_as_whole_tokens() {
        let mut lexer = Lexer::new(b"  stream\nabc", 0);
        assert!(!lexer.eat_keyword(b"str"));
        assert!(lexer.eat_keyword(b"stream"));
        assert_eq!(lexer.position(), 8);
        let mut lexer = Lexer::new(b"endobjx", 0);
        assert!(!lexer.eat_keyword(b"endobj"));
        assert_eq!(lexer.position(), 0);
    }

    #[test]
    fn remembers_how_far_it_has_read() {
        // Looking for a keyword that is not there reads the white space and the comment before it, and then goes back.
        let mut lexer = Lexer::new(b"1 % a comment\n2", 1);
        assert!(!lexer.eat_keyword(b"R"));
        assert_eq!(lexer.position(), 1);
        assert_eq!(lexer.furthest(), 14);
        // Going back by seeking keeps the mark too, and reading on moves it.
        lexer.seek(0);
        assert_eq!(lexer.furthest(), 14);
        assert_eq!(lexer.next_token().unwrap(), Some(Token::Integer(1)));
        assert_eq!(lexer.next_token().unwrap(), Some(Token::Integer(2)));
        assert_eq!(lexer.furthest(), 15);
    }
}
