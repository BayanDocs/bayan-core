//! Reads string fields from the JSON that Cargo prints (`cargo metadata`, `--message-format=json`).
//!
//! Two readers, both written here to keep xtask free of dependencies. `string_values` finds every `"key": "string"` pair with a given key and decodes the string, wherever it is; that is enough for most uses. `parse` reads a whole document into a [`Value`], for the few checks that need to know which object a field belongs to, such as which package a build script belongs to.

/// Returns the decoded value of every `"key": "…"` pair in `json`, in order. Pairs whose value is not a string (for example `null`) are skipped.
pub fn string_values(json: &str, key: &str) -> Vec<String> {
    let pattern = format!("\"{key}\"");
    let mut values = Vec::new();
    let mut rest = json;
    while let Some(found) = rest.find(&pattern) {
        rest = &rest[found + pattern.len()..];
        let Some(after_colon) = rest.trim_start().strip_prefix(':') else {
            continue;
        };
        let Some(string) = after_colon.trim_start().strip_prefix('"') else {
            continue;
        };
        if let Some((value, length)) = decode(string) {
            values.push(value);
            rest = &string[length..];
        }
    }
    values
}

/// A JSON value. Numbers are kept as written, because no check needs their value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, as written.
    Number(String),
    /// A string, decoded.
    String(String),
    /// An array.
    Array(Vec<Value>),
    /// An object's members, in order.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value of the member `key`, if this is an object that has one.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    /// The text, if this is a string.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(text) => Some(text),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(elements) => Some(elements),
            _ => None,
        }
    }
}

/// How deeply arrays and objects may nest; Cargo's output nests about five levels.
const MAX_DEPTH: usize = 64;

/// Reads one JSON document, such as the output of `cargo metadata`.
pub fn parse(text: &str) -> Result<Value, String> {
    let mut reader = Reader { text, position: 0 };
    let value = reader.value(0)?;
    reader.skip_whitespace();
    if reader.position == text.len() {
        Ok(value)
    } else {
        Err(reader.error("unexpected text after the JSON value"))
    }
}

struct Reader<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Reader<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.position..]
    }

    fn error(&self, problem: &str) -> String {
        format!("invalid JSON at byte {}: {problem}", self.position)
    }

    fn skip_whitespace(&mut self) {
        let rest = self.rest();
        self.position += rest.len() - rest.trim_start_matches([' ', '\t', '\n', '\r']).len();
    }

    fn eat(&mut self, token: &str) -> bool {
        if self.rest().starts_with(token) {
            self.position += token.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        self.skip_whitespace();
        if self.eat("null") {
            Ok(Value::Null)
        } else if self.eat("true") {
            Ok(Value::Bool(true))
        } else if self.eat("false") {
            Ok(Value::Bool(false))
        } else if self.eat("\"") {
            self.string().map(Value::String)
        } else if self.eat("[") {
            let mut elements = Vec::new();
            self.skip_whitespace();
            if !self.eat("]") {
                loop {
                    elements.push(self.value(depth + 1)?);
                    self.skip_whitespace();
                    if self.eat("]") {
                        break;
                    }
                    if !self.eat(",") {
                        return Err(self.error("expected `,` or `]`"));
                    }
                }
            }
            Ok(Value::Array(elements))
        } else if self.eat("{") {
            let mut members = Vec::new();
            self.skip_whitespace();
            if !self.eat("}") {
                loop {
                    self.skip_whitespace();
                    if !self.eat("\"") {
                        return Err(self.error("expected a member name"));
                    }
                    let name = self.string()?;
                    self.skip_whitespace();
                    if !self.eat(":") {
                        return Err(self.error("expected `:`"));
                    }
                    members.push((name, self.value(depth + 1)?));
                    self.skip_whitespace();
                    if self.eat("}") {
                        break;
                    }
                    if !self.eat(",") {
                        return Err(self.error("expected `,` or `}`"));
                    }
                }
            }
            Ok(Value::Object(members))
        } else {
            let rest = self.rest();
            let length = rest
                .find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')))
                .unwrap_or(rest.len());
            if length == 0 {
                return Err(self.error("expected a value"));
            }
            self.position += length;
            Ok(Value::Number(rest[..length].to_owned()))
        }
    }

    /// Reads a string whose opening quote has been consumed.
    fn string(&mut self) -> Result<String, String> {
        let (value, length) =
            decode(self.rest()).ok_or_else(|| self.error("unterminated string"))?;
        self.position += length;
        Ok(value)
    }
}

/// Decodes a JSON string whose opening quote has already been consumed. Returns the value and the number of bytes up to and including the closing quote.
fn decode(text: &str) -> Option<(String, usize)> {
    let mut value = String::new();
    let mut characters = text.char_indices();
    while let Some((position, character)) = characters.next() {
        match character {
            '"' => return Some((value, position + 1)),
            '\\' => match characters.next()?.1 {
                'b' => value.push('\u{8}'),
                'f' => value.push('\u{c}'),
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'u' => {
                    let code: String = (0..4)
                        .filter_map(|_| characters.next().map(|(_, c)| c))
                        .collect();
                    let unit = u32::from_str_radix(&code, 16).ok()?;
                    // Characters outside the Basic Multilingual Plane arrive as two escapes (a surrogate pair); paths and diagnostics hardly ever contain them, so they are replaced rather than combined.
                    value.push(char::from_u32(unit).unwrap_or(char::REPLACEMENT_CHARACTER));
                }
                other => value.push(other),
            },
            _ => value.push(character),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_decodes_every_string_value() {
        let json = r#"{"packages":[{"name":"a","manifest_path":"C:\\work\\bayan-core\\crates\\a\\Cargo.toml"},{"manifest_path" : "/x/\"quoted\"/Cargo.toml"}],"rendered":null}"#;
        assert_eq!(
            string_values(json, "manifest_path"),
            [
                r"C:\work\bayan-core\crates\a\Cargo.toml",
                r#"/x/"quoted"/Cargo.toml"#
            ]
        );
        assert!(
            string_values(json, "rendered").is_empty(),
            "null is not a string"
        );
    }

    #[test]
    fn ignores_the_key_inside_string_values() {
        let json = r#"{"message":"the \"manifest_path\":\"fake\" text","manifest_path":"real"}"#;
        assert_eq!(string_values(json, "manifest_path"), ["real"]);
    }

    #[test]
    fn decodes_escapes() {
        let json = r#"{"rendered":"error: line one\nline \u00e9 \/ \\ \t end"}"#;
        assert_eq!(
            string_values(json, "rendered"),
            ["error: line one\nline é / \\ \t end"]
        );
    }

    #[test]
    fn parses_a_document() {
        let json = r#" {"packages":[{"name":"a\u00e9","targets":[{"kind":["custom-build"],"doc":false,"edition":null,"n":-1.5e3}]}],"empty":{},"none":[]} "#;
        let value = parse(json).unwrap();
        let package = &value.get("packages").unwrap().as_array().unwrap()[0];
        assert_eq!(package.get("name").unwrap().as_str(), Some("aé"));
        let target = &package.get("targets").unwrap().as_array().unwrap()[0];
        assert_eq!(
            target.get("kind"),
            Some(&Value::Array(vec![Value::String(
                "custom-build".to_owned()
            )]))
        );
        assert_eq!(target.get("doc"), Some(&Value::Bool(false)));
        assert_eq!(target.get("edition"), Some(&Value::Null));
        assert_eq!(target.get("n"), Some(&Value::Number("-1.5e3".to_owned())));
        assert_eq!(value.get("empty"), Some(&Value::Object(Vec::new())));
        assert_eq!(value.get("missing"), None);
    }

    #[test]
    fn rejects_invalid_documents() {
        for json in [
            "",
            "{",
            "[1,]",
            "{\"a\" 1}",
            "{a:1}",
            "[1] 2",
            "\"open",
            "nul",
            &"[".repeat(100),
        ] {
            assert!(parse(json).is_err(), "{json:?}");
        }
    }

    #[test]
    fn stops_at_an_unterminated_string() {
        assert!(string_values(r#"{"rendered":"no end"#, "rendered").is_empty());
    }
}
