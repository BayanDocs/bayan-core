//! Reads string fields from the JSON that Cargo prints (`cargo metadata`, `--message-format=json`).
//!
//! This is not a general JSON parser: it finds every `"key": "string"` pair with a given key and decodes the string. That is all this tool needs, and it keeps xtask free of dependencies.

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
    fn stops_at_an_unterminated_string() {
        assert!(string_values(r#"{"rendered":"no end"#, "rendered").is_empty());
    }
}
