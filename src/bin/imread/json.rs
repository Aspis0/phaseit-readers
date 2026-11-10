// SPDX-License-Identifier: Apache-2.0
//! JSON written by hand (no dependency). Output stays on one line: sweep records are read line by line.

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

pub fn string_or_null(value: Option<&str>) -> String {
    match value {
        Some(text) => string(text),
        None => "null".to_string(),
    }
}

/// NaN and infinity have no JSON form, so a non-finite value is written as `null`.
pub fn number(value: f64) -> String {
    if value.is_finite() {
        format!("{value}")
    } else {
        "null".to_string()
    }
}

pub fn integer(value: usize) -> String {
    value.to_string()
}

/// An unrecorded measurement is absent, not zero: `null` must not read as a measured `0`.
pub fn number_or_null(value: Option<f64>) -> String {
    match value {
        Some(value) => number(value),
        None => "null".to_string(),
    }
}

pub fn string_array(values: &[String]) -> String {
    let items = values
        .iter()
        .map(|value| string(value))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{items}]")
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

    #[test]
    fn a_missing_number_is_null() {
        assert_eq!(number_or_null(None), "null");
        assert_eq!(number_or_null(Some(1.5)), "1.5");
        assert_eq!(number_or_null(Some(0.0)), "0");
    }

    #[test]
    fn a_string_array_is_a_json_array() {
        assert_eq!(string_array(&[]), "[]");
        assert_eq!(string_array(&["a".to_string()]), "[\"a\"]");
        assert_eq!(
            string_array(&["a".to_string(), "b\"c".to_string()]),
            "[\"a\",\"b\\\"c\"]"
        );
    }
}
