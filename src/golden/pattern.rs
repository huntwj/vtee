//! The regex dialect of masks and assertions (design section 6.7).
//!
//! A small common subset of Perl, Rust, Python, JavaScript and RE2 syntax, ASCII-only for the class shorthands, with a
//! list of rejected constructs, so a pattern means the same in every implementation of the checker. This module checks
//! a pattern against the dialect and translates it into the syntax of the `regex` crate, which does the matching
//! (leftmost-first, like Perl).
//!
//! Translation rules: every literal scalar becomes `\x{H}`; `.` becomes `(?s:.)`; `\d \w \s` become explicit ASCII
//! sets and their complements `[^...]` (nested when inside a class); a capturing group becomes a non-capturing one.

use std::fmt::{self, Write as _};

use regex::{Regex, RegexBuilder};

/// Why a pattern is not in the dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexError {
    /// What is wrong.
    pub message: String,
    /// The offset, in scalars from the start of the pattern, where it was noticed.
    pub offset: usize,
}

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "regex: {} (at offset {})", self.message, self.offset)
    }
}

impl std::error::Error for RegexError {}

/// The largest count in a `{m,n}` quantifier, as in RE2.
const MAX_REPEAT: u32 = 1000;

/// Characters that stand for themselves after a backslash.
const ESCAPABLE: &str = "\\/.+*?()[]{}|^$-";

struct Translator {
    chars: Vec<char>,
    pos: usize,
}

type Result<T> = std::result::Result<T, RegexError>;

enum ClassItem {
    Scalar(char),
    Shorthand(char),
}

impl Translator {
    fn fail<T>(&self, message: impl Into<String>) -> Result<T> {
        self.fail_at(self.pos, message)
    }

    fn fail_at<T>(&self, offset: usize, message: impl Into<String>) -> Result<T> {
        Err(RegexError { message: message.into(), offset })
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn alternation(&mut self, out: &mut String) -> Result<()> {
        self.sequence(out)?;
        while self.peek() == Some('|') {
            self.pos += 1;
            out.push('|');
            self.sequence(out)?;
        }
        Ok(())
    }

    fn sequence(&mut self, out: &mut String) -> Result<()> {
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            if matches!(c, '*' | '+' | '?') {
                return self.fail("nothing to repeat");
            }
            if c == '{' {
                return self.fail("a '{' must be escaped as \\{ (no quantifier here to apply it to)");
            }
            let anchor = matches!(c, '^' | '$');
            self.atom(out)?;
            self.quantifier(out, anchor)?;
        }
        Ok(())
    }

    fn atom(&mut self, out: &mut String) -> Result<()> {
        let c = self.peek().expect("sequence checked there is a character");
        self.pos += 1;
        match c {
            '(' => {
                if self.peek() == Some('?') {
                    if self.peek_at(1) == Some(':') {
                        self.pos += 2;
                    } else {
                        return self.fail("only (?: ) is allowed after '(?'; flags, named groups and look-around are not in the dialect");
                    }
                }
                out.push_str("(?:");
                self.alternation(out)?;
                if self.peek() != Some(')') {
                    return self.fail("unbalanced '('");
                }
                self.pos += 1;
                out.push(')');
            }
            '[' => self.class(out)?,
            '.' => out.push_str("(?s:.)"),
            '^' => out.push('^'),
            '$' => out.push('$'),
            '\\' => self.escape(out)?,
            c => literal(c, out),
        }
        Ok(())
    }

    /// An escape outside a class.
    fn escape(&mut self, out: &mut String) -> Result<()> {
        let at = self.pos;
        let Some(c) = self.peek() else {
            return self.fail_at(at.saturating_sub(1), "a backslash ends the pattern");
        };
        self.pos += 1;
        if ESCAPABLE.contains(c) {
            literal(c, out);
        } else if matches!(c, 'd' | 'D' | 'w' | 'W' | 's' | 'S') {
            shorthand(c, false, out);
        } else {
            return self.fail_at(at - 1, format!("the escape \\{c} is not in the dialect"));
        }
        Ok(())
    }

    fn quantifier(&mut self, out: &mut String, after_anchor: bool) -> Result<()> {
        let Some(c) = self.peek() else { return Ok(()) };
        let start = self.pos;
        match c {
            '*' | '+' | '?' => {
                self.pos += 1;
                out.push(c);
            }
            '{' => {
                let (min, max, used) = self.counted()?;
                self.pos += used;
                match max {
                    Some(max) if max == min => write!(out, "{{{min}}}").expect("write to String"),
                    Some(max) => write!(out, "{{{min},{max}}}").expect("write to String"),
                    None => write!(out, "{{{min},}}").expect("write to String"),
                }
            }
            _ => return Ok(()),
        }
        if after_anchor {
            return self.fail_at(start, "an anchor cannot be repeated");
        }
        if self.peek() == Some('?') {
            self.pos += 1;
            out.push('?');
        }
        if matches!(self.peek(), Some('*' | '+' | '?' | '{')) {
            return self.fail("nothing to repeat");
        }
        Ok(())
    }

    /// Reads `{m}`, `{m,}` or `{m,n}` at the current position without consuming it: returns the bounds and the length.
    fn counted(&self) -> Result<(u32, Option<u32>, usize)> {
        let rest: String = self.chars[self.pos..].iter().collect();
        let bad =
            || self.fail_at(self.pos, "a '{' must be escaped as \\{ unless it starts a quantifier {m}, {m,} or {m,n}");
        let Some(close) = rest.find('}') else { return bad() };
        let body = &rest[1..close];
        let number = |s: &str| -> Option<u32> {
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) { None } else { s.parse().ok() }
        };
        let (min, max) = match body.split_once(',') {
            None => {
                let Some(n) = number(body) else { return bad() };
                (n, Some(n))
            }
            Some((lo, "")) => {
                let Some(lo) = number(lo) else { return bad() };
                (lo, None)
            }
            Some((lo, hi)) => {
                let (Some(lo), Some(hi)) = (number(lo), number(hi)) else { return bad() };
                (lo, Some(hi))
            }
        };
        if min > MAX_REPEAT || max.is_some_and(|m| m > MAX_REPEAT) {
            return self.fail_at(self.pos, format!("a repeat count is at most {MAX_REPEAT}"));
        }
        if max.is_some_and(|m| m < min) {
            return self.fail_at(self.pos, "the upper bound of a repeat is below the lower bound");
        }
        Ok((min, max, body.chars().count() + 2))
    }

    fn class(&mut self, out: &mut String) -> Result<()> {
        let open = self.pos - 1;
        out.push('[');
        if self.peek() == Some('^') {
            self.pos += 1;
            out.push('^');
        }
        let mut items = 0;
        loop {
            let Some(c) = self.peek() else {
                return self.fail_at(open, "unterminated character class");
            };
            if c == ']' {
                self.pos += 1;
                break;
            }
            if c == '-' && items > 0 && self.peek_at(1) != Some(']') {
                return self.fail("a '-' in the middle of a class must be escaped as \\-");
            }
            let item = self.class_atom()?;
            items += 1;
            let range_follows = self.peek() == Some('-') && self.peek_at(1).is_some_and(|n| n != ']');
            match item {
                ClassItem::Shorthand(s) => {
                    if range_follows {
                        return self.fail("a range cannot start with a class such as \\d");
                    }
                    shorthand(s, true, out);
                }
                ClassItem::Scalar(lo) if range_follows => {
                    self.pos += 1;
                    let hi_at = self.pos;
                    let ClassItem::Scalar(hi) = self.class_atom()? else {
                        return self.fail_at(hi_at, "a range cannot end with a class such as \\d");
                    };
                    if hi < lo {
                        return self.fail_at(hi_at, "the range is backwards");
                    }
                    write!(out, "\\x{{{:x}}}-\\x{{{:x}}}", lo as u32, hi as u32).expect("write to String");
                }
                ClassItem::Scalar(c) => literal(c, out),
            }
        }
        if items == 0 {
            return self.fail_at(open, "an empty character class");
        }
        out.push(']');
        Ok(())
    }

    fn class_atom(&mut self) -> Result<ClassItem> {
        let c = self.peek().expect("class checked there is a character");
        self.pos += 1;
        match c {
            '[' => self.fail_at(
                self.pos - 1,
                "a '[' inside a class must be escaped; POSIX classes and nested classes are not in the dialect",
            ),
            '\\' => {
                let at = self.pos;
                let Some(e) = self.peek() else {
                    return self.fail_at(at - 1, "a backslash ends the pattern");
                };
                self.pos += 1;
                if ESCAPABLE.contains(e) {
                    Ok(ClassItem::Scalar(e))
                } else if matches!(e, 'd' | 'D' | 'w' | 'W' | 's' | 'S') {
                    Ok(ClassItem::Shorthand(e))
                } else {
                    self.fail_at(at - 1, format!("the escape \\{e} is not in the dialect"))
                }
            }
            c => Ok(ClassItem::Scalar(c)),
        }
    }
}

fn literal(c: char, out: &mut String) {
    write!(out, "\\x{{{:x}}}", c as u32).expect("write to String");
}

/// `\d \w \s` and their complements, as ASCII sets. Outside a class the result is a class of its own; inside one the
/// complements are nested classes.
fn shorthand(c: char, in_class: bool, out: &mut String) {
    let (set, negated) = match c {
        'd' => ("0-9", false),
        'D' => ("0-9", true),
        'w' => ("0-9A-Za-z_", false),
        'W' => ("0-9A-Za-z_", true),
        's' => ("\\x{9}-\\x{d}\\x{20}", false),
        _ => ("\\x{9}-\\x{d}\\x{20}", true),
    };
    match (in_class, negated) {
        (true, false) => out.push_str(set),
        (_, false) => {
            out.push('[');
            out.push_str(set);
            out.push(']');
        }
        (_, true) => {
            out.push_str("[^");
            out.push_str(set);
            out.push(']');
        }
    }
}

/// A pattern of the dialect, ready to match.
#[derive(Debug, Clone)]
pub struct Pattern {
    source: String,
    search: Regex,
    full: Regex,
}

impl Pattern {
    /// Checks `source` against the dialect and compiles it.
    pub fn new(source: &str) -> Result<Pattern> {
        let mut t = Translator { chars: source.chars().collect(), pos: 0 };
        let mut translated = String::new();
        t.alternation(&mut translated)?;
        if t.pos < t.chars.len() {
            return t.fail("unbalanced ')'");
        }
        let build = |pattern: &str| {
            RegexBuilder::new(pattern)
                .size_limit(1 << 22)
                .nest_limit(100)
                .build()
                .map_err(|e| RegexError { message: format!("the pattern is too complex: {e}"), offset: 0 })
        };
        let search = build(&translated)?;
        let full = build(&format!("^(?:{translated})$"))?;
        Ok(Pattern { source: source.to_string(), search, full })
    }

    /// The pattern as written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The first match in `subject` at or after the scalar offset `from`, as `[start, end)` scalar offsets. `^` still
    /// means the start of the subject, not `from`.
    pub fn search_from(&self, subject: &str, from: usize) -> Option<(usize, usize)> {
        let offsets: Vec<usize> = subject.char_indices().map(|(i, _)| i).chain([subject.len()]).collect();
        let start = *offsets.get(from)?;
        let m = self.search.find_at(subject, start)?;
        let scalar = |byte: usize| offsets.binary_search(&byte).expect("a match lies on character boundaries");
        Some((scalar(m.start()), scalar(m.end())))
    }

    /// The first match in `subject` (leftmost-first).
    pub fn search(&self, subject: &str) -> Option<(usize, usize)> {
        self.search_from(subject, 0)
    }

    /// True when the pattern matches the whole of `subject` (`^(?:RE)$`).
    pub fn is_full_match(&self, subject: &str) -> bool {
        self.full.is_match(subject)
    }

    /// The scan loop of section 6.5: successive non-empty matches, left to right, each search starting where the last
    /// match ended; an empty match is skipped by moving one scalar on.
    pub fn find_all(&self, subject: &str) -> Vec<(usize, usize)> {
        let length = subject.chars().count();
        let mut found = Vec::new();
        let mut pos = 0;
        while pos <= length {
            let Some((start, end)) = self.search_from(subject, pos) else { break };
            if start == end {
                pos = start + 1;
                continue;
            }
            found.push((start, end));
            pos = end;
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(src: &str) -> Pattern {
        Pattern::new(src).unwrap_or_else(|e| panic!("{src:?}: {e}"))
    }

    #[test]
    fn the_class_shorthands_are_ascii() {
        assert_eq!(pattern(r"\d+").search("ab 123 x"), Some((3, 6)));
        assert_eq!(pattern(r"\d+").search("\u{663}\u{664}"), None);
        assert_eq!(pattern(r"\w+").search("\u{e9}_x"), Some((1, 3)));
        assert_eq!(pattern(r"\s").search("a\u{a0}b"), None);
        assert_eq!(pattern(r"[\d.]+").search("v1.25x"), Some((1, 5)));
        assert_eq!(pattern(r"\D").search("12a"), Some((2, 3)));
        assert_eq!(pattern(r"\S+").search("  ab c"), Some((2, 4)));
        assert_eq!(pattern(r"\W").search("a-b"), Some((1, 2)));
        assert_eq!(pattern(r"[^\d]").search("12a"), Some((2, 3)));
        assert_eq!(pattern(r"[\D]").search("12a"), Some((2, 3)));
    }

    #[test]
    fn search_is_leftmost_first_not_longest() {
        assert_eq!(pattern("a|ab").search("ab"), Some((0, 1)));
        assert_eq!(pattern("(a|ab)(c|bcd)").search("abcd"), Some((0, 4)));
        assert_eq!(pattern("x{2,3}").search("xxxx"), Some((0, 3)));
        assert_eq!(pattern("x{2,3}?").search("xxxx"), Some((0, 2)));
        assert_eq!(pattern("a+?").search("aaa"), Some((0, 1)));
    }

    #[test]
    fn anchors_and_full_matches() {
        assert_eq!(pattern("^ab").search("ab"), Some((0, 2)));
        assert_eq!(pattern("b$").search("ab"), Some((1, 2)));
        assert!(pattern("^$").is_full_match(""));
        assert!(pattern("a.c").is_full_match("abc"));
        assert!(!pattern("a").is_full_match("ab"));
        assert!(pattern("a|ab").is_full_match("ab"));
    }

    #[test]
    fn dot_is_one_scalar_and_find_skips_empty_matches() {
        assert_eq!(pattern(".").search("\u{4f60}"), Some((0, 1)));
        assert_eq!(pattern("a*").find_all("baa"), [(1, 3)]);
        assert_eq!(pattern(r"\d\d:\d\d").find_all("at 03:18 and 04:19"), [(3, 8), (13, 18)]);
    }

    #[test]
    fn caret_still_means_the_start_of_the_subject_when_searching_later() {
        assert_eq!(pattern("^a").search_from("aa", 1), None);
    }

    #[test]
    fn escapes_and_delimiters() {
        assert_eq!(pattern(r"a\/b").search("a/b"), Some((0, 3)));
        assert_eq!(pattern(r"\.\.").search("a..b"), Some((1, 3)));
        assert_eq!(pattern(r"\+\d").search("a+1"), Some((1, 3)));
        assert_eq!(pattern(r"[ \d:.]+").search("  1:23.4 x"), Some((0, 9)));
        assert_eq!(pattern(r"[a\-c]+").search("-ac"), Some((0, 3)));
        assert_eq!(pattern(r"[a-]+").search("a-"), Some((0, 2)));
    }

    #[test]
    fn what_the_dialect_rejects() {
        for bad in [
            r"\bfoo",
            "(?i)abc",
            r"\x41",
            r"\p{L}",
            "(?=a)",
            r"(a)\1",
            "[[:alpha:]]",
            "(?P<n>a)",
            r"\Aa",
            r"a\z",
            r"\u0041",
            r"\h",
            "a**",
            "(a",
            "[a",
            "a)",
            "[]",
            "[^]",
            "{2}",
            "a{",
            "a{x}",
            "a{3,2}",
            "a{1001}",
            "*a",
            "a|*",
            "^*",
            "[z-a]",
            r"[\d-z]",
            r"[a-\d]",
            "[a-c-e]",
            r"\n",
            r"\t",
            "a\\",
            "a+++",
            "a{2}{3}",
            "(?<n>a)",
            "(?:a",
            "[a-",
        ] {
            assert!(Pattern::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn what_the_dialect_accepts() {
        for good in [
            "", "a|", "()", "(|a)", "a{0}", "a{0,}", "a{1000}", "]", "}", r"[\]]", r"\{\}", "(?:ab)+", "a??", "a{2,3}?",
        ] {
            assert!(Pattern::new(good).is_ok(), "{good:?} should be accepted");
        }
    }
}
