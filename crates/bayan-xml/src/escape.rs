//! Escaping text and attribute values for writing, so that reading them back gives exactly the same characters.
//!
//! Both functions expect characters that XML allows (callers check); they never fail.

/// Appends `value` to `out` as the content of a double-quoted attribute value: `&`, `<`, `>` and `"` as entity references, and tab, line feed and carriage return as character references, which attribute-value normalization (XML 1.0 §3.3.3) keeps as they are.
pub(crate) fn attribute(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            other => out.push(other),
        }
    }
}

/// Appends `value` to `out` as character data: `&`, `<` and `>` as entity references, and carriage return as a character reference, which line-end normalization (XML 1.0 §2.11) keeps.
pub(crate) fn text(value: &str, out: &mut String) {
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            other => out.push(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_what_reading_would_change() {
        let mut out = String::new();
        attribute("a&b<c>d\"e'f\tg\nh\ri é", &mut out);
        assert_eq!(out, "a&amp;b&lt;c&gt;d&quot;e'f&#x9;g&#xA;h&#xD;i é");
        let mut out = String::new();
        text("a&b<c>d\"e'f\tg\nh\ri ]]> é", &mut out);
        assert_eq!(out, "a&amp;b&lt;c&gt;d\"e'f\tg\nh&#xD;i ]]&gt; é");
    }
}
