//! The one-line JSON writer used for every NDJSON line vtee emits.
//!
//! `serde_json` leaves U+0085, U+2028 and U+2029 unescaped, which is valid JSON, but several line-oriented readers
//! (JavaScript's `String.prototype.split` on line terminators, Python's `str.splitlines`, .NET `ReadLine`) treat them as
//! line breaks and would cut a snapshot line in two. They are written as `\uXXXX` escapes so a line never contains one.

use std::io;

use serde::Serialize;
use serde_json::Serializer;
use serde_json::ser::{CompactFormatter, Formatter};

/// Characters that some readers treat as line terminators although JSON does not.
const ESCAPED: [char; 3] = ['\u{85}', '\u{2028}', '\u{2029}'];

struct NdjsonFormatter;

impl Formatter for NdjsonFormatter {
    fn write_string_fragment<W: ?Sized + io::Write>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()> {
        let mut rest = fragment;
        while let Some(at) = rest.find(ESCAPED) {
            CompactFormatter.write_string_fragment(writer, &rest[..at])?;
            let ch = rest[at..].chars().next().expect("find returned a character boundary");
            write!(writer, "\\u{:04x}", ch as u32)?;
            rest = &rest[at + ch.len_utf8()..];
        }
        CompactFormatter.write_string_fragment(writer, rest)
    }
}

/// Serializes `value` as compact JSON that contains no line terminator of any kind.
pub fn to_line<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut out = Vec::new();
    value.serialize(&mut Serializer::with_formatter(&mut out, NdjsonFormatter))?;
    Ok(String::from_utf8(out).expect("serde_json writes UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unicode_line_separators_are_escaped() {
        let line = to_line(&"a\u{85}b\u{2028}c\u{2029}d").unwrap();
        let bs = '\u{5c}';
        assert_eq!(line, format!("\"a{bs}u0085b{bs}u2028c{bs}u2029d\""));
        assert!(!line.contains(ESCAPED));
        let back: String = serde_json::from_str(&line).unwrap();
        assert_eq!(back, "a\u{85}b\u{2028}c\u{2029}d");
    }

    #[test]
    fn everything_else_matches_serde_json() {
        let value = serde_json::json!({ "t": "quote\" slash\\ nl\n é 😀", "n": [1, null, true] });
        assert_eq!(to_line(&value).unwrap(), serde_json::to_string(&value).unwrap());
    }

    #[test]
    fn keys_are_escaped_too() {
        let value = serde_json::json!({ "k\u{2029}": "v" });
        let bs = '\u{5c}';
        assert_eq!(to_line(&value).unwrap(), format!("{{\"k{bs}u2029\":\"v\"}}"));
    }
}
