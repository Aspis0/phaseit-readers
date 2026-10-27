// SPDX-License-Identifier: Apache-2.0
//! JSON writing by hand.
//!
//! The binary has no JSON dependency, so the few primitives a sweep line needs
//! are written here: a string literal, a number, and the two optional forms.
//! Everything this module emits is one line of valid JSON — a sweep record is
//! read line by line, so a line break inside a record would split it.

/// A JSON string literal, with the escapes JSON requires.
pub fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON string, or `null` when there is no string.
pub fn string_or_null(value: Option<&str>) -> String {
    match value {
        Some(text) => string(text),
        None => "null".to_string(),
    }
}

/// A JSON number. JSON has no form for NaN or infinity, so a non-finite value
/// is written as `null` rather than as something a parser would reject.
pub fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "null".to_string()
    }
}

/// A JSON number from an integer.
pub fn integer(value: usize) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_quoted() {
        assert_eq!(string("abc"), "\"abc\"");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(string("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }

    #[test]
    fn control_characters_are_escaped() {
        assert_eq!(string("a\nb"), "\"a\\nb\"");
        assert_eq!(string("a\tb"), "\"a\\tb\"");
        assert_eq!(string("a\u{01}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn non_ascii_stays_utf8() {
        assert_eq!(string("\u{00b5}m"), "\"\u{00b5}m\"");
    }

    #[test]
    fn missing_string_is_null() {
        assert_eq!(string_or_null(None), "null");
        assert_eq!(string_or_null(Some("x")), "\"x\"");
    }

    #[test]
    fn non_finite_numbers_are_null() {
        assert_eq!(number(f64::NAN), "null");
        assert_eq!(number(f64::INFINITY), "null");
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(0.0), "0");
    }
}
