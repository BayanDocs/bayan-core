//! Just enough TOML reading for this repository's own configuration files.
//!
//! It understands tables of `key = value` entries whose values are strings, numbers, booleans, arrays and inline tables, possibly spread over several lines. It is not a general TOML parser, and it reports anything it does not understand as an error, so a guardrail can never pass because a file was misread.

/// One `key = value` entry of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The key, as written.
    pub key: String,
    /// The value with comments removed and whitespace outside strings dropped, so that formatting differences do not matter: `{ level = "warn" }` becomes `{level="warn"}`.
    pub value: String,
    /// The line on which the entry starts, counting from 1.
    pub line: usize,
}

/// Returns the entries of the table `[name]` in `text`, or `None` if there is no such table. The empty name selects the entries before the first table header.
pub fn table(text: &str, name: &str) -> Result<Option<Vec<Entry>>, String> {
    let mut found: Option<Vec<Entry>> = name.is_empty().then(Vec::new);
    let mut in_table = name.is_empty();
    // An entry whose value continues on the next lines: its key, first line and value so far.
    let mut pending: Option<(String, usize, String)> = None;

    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        let line = strip_comment(raw).map_err(|error| format!("line {number}: {error}"))?;
        let line = line.trim();

        // The entry this line continues or starts: its key, first line and value so far.
        let (key, start, value) = if let Some((key, start, mut value)) = pending.take() {
            value.push_str(&compact(line).map_err(|error| format!("line {number}: {error}"))?);
            (key, start, value)
        } else if line.is_empty() {
            continue;
        } else if line.starts_with('[') {
            let header = header_name(line).map_err(|error| format!("line {number}: {error}"))?;
            in_table = header == name && !line.starts_with("[[");
            if in_table {
                if found.is_some() {
                    return Err(format!("line {number}: the table [{name}] appears twice"));
                }
                found = Some(Vec::new());
            }
            continue;
        } else {
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("line {number}: expected `key = value`, found `{line}`"))?;
            let value = compact(value).map_err(|error| format!("line {number}: {error}"))?;
            (key.trim().to_owned(), number, value)
        };
        if depth(&value)? > 0 {
            pending = Some((key, start, value));
        } else if in_table && let Some(entries) = found.as_mut() {
            entries.push(Entry {
                key,
                value,
                line: start,
            });
        }
    }
    if let Some((key, start, _)) = pending {
        return Err(format!(
            "line {start}: the value of `{key}` is never closed"
        ));
    }
    Ok(found)
}

/// Returns the value of `key` in the table `[table_name]`, if both exist.
pub fn value(text: &str, table_name: &str, key: &str) -> Result<Option<String>, String> {
    Ok(table(text, table_name)?
        .and_then(|entries| entries.into_iter().find(|entry| entry.key == key))
        .map(|entry| entry.value))
}

/// Reads a compacted value that must be one basic string, such as `"1.99.0"`, and returns its contents.
pub fn string(value: &str) -> Result<String, String> {
    let inner = value
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .ok_or_else(|| format!("expected a string in double quotes, found `{value}`"))?;
    unescape(inner)
}

/// Reads a compacted array of inline tables, such as `[{path="f64::sin",reason="…"},…]`, and returns the string value of `field` in each element.
pub fn field_of_each(array: &str, field: &str) -> Result<Vec<String>, String> {
    let inner = array
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(|| format!("expected an array, found `{array}`"))?;
    let mut values = Vec::new();
    for element in split_top_level(inner)? {
        let fields = element
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
            .ok_or_else(|| format!("expected an inline table, found `{element}`"))?;
        let mut value = None;
        for pair in split_top_level(fields)? {
            let (key, text) = pair
                .split_once('=')
                .ok_or_else(|| format!("expected `key = value` in `{element}`"))?;
            if key == field {
                value = Some(string(text)?);
            }
        }
        values.push(value.ok_or_else(|| format!("`{element}` has no `{field}`"))?);
    }
    Ok(values)
}

/// Removes a comment from one line, leaving `#` characters inside strings alone.
fn strip_comment(line: &str) -> Result<&str, String> {
    let mut quote = None;
    let mut escaped = false;
    for (position, character) in line.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if character == '#' => return Ok(&line[..position]),
            None if character == '"' || character == '\'' => {
                if line[position..].starts_with("\"\"\"") || line[position..].starts_with("'''") {
                    return Err("multi-line strings are not supported".to_owned());
                }
                quote = Some(character);
            }
            None => {}
        }
    }
    if quote.is_some() {
        return Err("a string is not closed on this line".to_owned());
    }
    Ok(line)
}

/// Drops the whitespace outside strings.
fn compact(text: &str) -> Result<String, String> {
    let mut result = String::with_capacity(text.len());
    let mut quote = None;
    let mut escaped = false;
    for character in text.chars() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(open) if character == open => quote = None,
            Some(_) => {}
            None if character.is_whitespace() => continue,
            None if character == '"' || character == '\'' => quote = Some(character),
            None => {}
        }
        result.push(character);
    }
    if quote.is_some() {
        return Err("a string is not closed".to_owned());
    }
    Ok(result)
}

/// How many arrays and inline tables are still open at the end of a compacted value.
fn depth(value: &str) -> Result<usize, String> {
    let mut open: usize = 0;
    let mut quote = None;
    let mut escaped = false;
    for character in value.chars() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(close) if character == close => quote = None,
            Some(_) => {}
            None => match character {
                '"' | '\'' => quote = Some(character),
                '[' | '{' => open += 1,
                ']' | '}' => {
                    open = open
                        .checked_sub(1)
                        .ok_or_else(|| format!("unbalanced brackets in `{value}`"))?;
                }
                _ => {}
            },
        }
    }
    Ok(open)
}

/// The name in a table header line: `[ workspace.lints.rust ]` gives `workspace.lints.rust`.
fn header_name(line: &str) -> Result<String, String> {
    let inner = line
        .strip_prefix("[[")
        .and_then(|rest| rest.strip_suffix("]]"))
        .or_else(|| {
            line.strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
        })
        .ok_or_else(|| format!("expected a table header, found `{line}`"))?;
    compact(inner)
}

/// Splits the inside of a compacted array or inline table at the commas that are not inside a string or a nested value. A trailing comma is allowed.
fn split_top_level(inner: &str) -> Result<Vec<&str>, String> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut nested: usize = 0;
    let mut quote = None;
    let mut escaped = false;
    for (position, character) in inner.char_indices() {
        match quote {
            Some('"') if escaped => escaped = false,
            Some('"') if character == '\\' => escaped = true,
            Some(close) if character == close => quote = None,
            Some(_) => {}
            None => match character {
                '"' | '\'' => quote = Some(character),
                '[' | '{' => nested += 1,
                ']' | '}' => {
                    nested = nested
                        .checked_sub(1)
                        .ok_or_else(|| format!("unbalanced brackets in `{inner}`"))?;
                }
                ',' if nested == 0 => {
                    parts.push(&inner[start..position]);
                    start = position + 1;
                }
                _ => {}
            },
        }
    }
    if !inner[start..].is_empty() {
        parts.push(&inner[start..]);
    }
    Ok(parts)
}

/// Resolves the escape sequences of a TOML basic string.
fn unescape(text: &str) -> Result<String, String> {
    let mut result = String::with_capacity(text.len());
    let mut characters = text.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some('"') => result.push('"'),
            Some('\\') => result.push('\\'),
            Some('n') => result.push('\n'),
            Some('t') => result.push('\t'),
            Some('r') => result.push('\r'),
            Some('u') => {
                let code: String = characters.by_ref().take(4).collect();
                let decoded = u32::from_str_radix(&code, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| format!("invalid escape `\\u{code}`"))?;
                result.push(decoded);
            }
            other => {
                return Err(format!(
                    "unsupported escape `\\{}`",
                    other.map(String::from).unwrap_or_default()
                ));
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
top = "level" # a comment
[workspace.lints.rust]
unsafe_code = "forbid" # trailing comment
unexpected_cfgs = { level = "warn", check-cfg = ["cfg(x, values(any()))"] }
hash = "a # inside a string"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
spread = [
  "one", # comment inside an array
  "two",
]
[[bin]]
name = "not a table we read"
"#;

    #[test]
    fn reads_a_table_and_normalizes_values() {
        let entries = table(SAMPLE, "workspace.lints.rust").unwrap().unwrap();
        let pairs: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| (entry.key.as_str(), entry.value.as_str()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("unsafe_code", "\"forbid\""),
                (
                    "unexpected_cfgs",
                    "{level=\"warn\",check-cfg=[\"cfg(x, values(any()))\"]}"
                ),
                ("hash", "\"a # inside a string\""),
            ]
        );
        assert_eq!(entries[0].line, 4);
    }

    #[test]
    fn joins_values_spread_over_several_lines() {
        let entries = table(SAMPLE, "workspace.lints.clippy").unwrap().unwrap();
        assert_eq!(entries[1].key, "spread");
        assert_eq!(entries[1].value, "[\"one\",\"two\",]");
        assert_eq!(entries[1].line, 10);
    }

    #[test]
    fn reads_the_root_table_and_reports_missing_tables() {
        assert_eq!(
            value(SAMPLE, "", "top").unwrap().as_deref(),
            Some("\"level\"")
        );
        assert_eq!(table(SAMPLE, "lints").unwrap(), None);
        assert_eq!(
            table(SAMPLE, "bin").unwrap(),
            None,
            "[[bin]] is an array of tables, not a table"
        );
    }

    #[test]
    fn treats_whitespace_in_headers_as_insignificant() {
        let text = "[ package ]\nname = \"x\"\n";
        assert_eq!(
            value(text, "package", "name").unwrap().as_deref(),
            Some("\"x\"")
        );
    }

    #[test]
    fn rejects_what_it_does_not_understand() {
        assert!(table("[a]\nkey = \"unclosed\n", "a").is_err());
        assert!(table("[a]\nkey = \"\"\"multi\nline\"\"\"\n", "a").is_err());
        assert!(table("[a]\nno equals sign\n", "a").is_err());
        assert!(table("[a]\nkey = [1, 2\n", "a").is_err());
        assert!(table("[a]\nkey = 1\n[a]\nkey = 2\n", "a").is_err());
    }

    #[test]
    fn reads_strings_with_escapes() {
        assert_eq!(string("\"1.99.0\"").unwrap(), "1.99.0");
        assert_eq!(string(r#""say \"hi\"é""#).unwrap(), "say \"hi\"é");
        assert!(string("1.99").is_err());
        assert!(string(r#""\q""#).is_err());
    }

    #[test]
    fn reads_a_field_from_each_inline_table() {
        let array = compact(
            r#"[ { path = "f64::sin", reason = "a, b" }, { reason = "c", path = "f32::cos" }, ]"#,
        )
        .unwrap();
        assert_eq!(
            field_of_each(&array, "path").unwrap(),
            ["f64::sin", "f32::cos"]
        );
        assert!(field_of_each("[{reason=\"no path\"}]", "path").is_err());
        assert!(field_of_each("\"not an array\"", "path").is_err());
    }
}
