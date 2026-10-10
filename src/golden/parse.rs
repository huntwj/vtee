//! Reading a golden (design sections 6.1 to 6.6 and 6.13): the file syntax, both headers, `[text]`, `[attrs]`,
//! `[masks]` and `[assert]`, and every rule a reader must reject.

use std::collections::BTreeSet;
use std::fmt;

use super::escape;
use super::model::*;
use super::pattern::Pattern;
use super::version::Refusal;
use crate::snapshot::{Cell, Color, CursorShape};

/// The largest screen a golden may describe (the resize limit, in both directions).
const MAX_DIMENSION: usize = 1000;

/// A golden that is not well formed. The run ends with exit code 65.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// The 1-based line of the file, or 0 when the problem is not about one line.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 { write!(f, "{}", self.message) } else { write!(f, "line {}: {}", self.line, self.message) }
    }
}

impl std::error::Error for ParseError {}

/// Why a golden could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Not a valid golden (exit 65).
    Malformed(ParseError),
    /// A valid golden this build will not read (exit 2). Only a grammar number above this build's can be decided before
    /// the rest of the file is read.
    Refused(Refusal),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Malformed(e) => e.fmt(f),
            LoadError::Refused(r) => f.write_str(&r.reason()),
        }
    }
}

impl std::error::Error for LoadError {}

type PResult<T> = Result<T, ParseError>;

fn fail<T>(line: usize, message: impl Into<String>) -> PResult<T> {
    Err(ParseError { line, message: message.into() })
}

impl Golden {
    /// Reads a golden from the bytes of a file.
    pub fn parse(bytes: &[u8]) -> Result<Golden, LoadError> {
        match std::str::from_utf8(bytes) {
            Ok(text) => Golden::parse_str(text),
            Err(e) => {
                let line = bytes[..e.valid_up_to()].iter().filter(|b| **b == b'\n').count() + 1;
                Err(LoadError::Malformed(ParseError { line, message: "the file is not valid UTF-8".to_string() }))
            }
        }
    }

    /// Reads a golden from text.
    pub fn parse_str(text: &str) -> Result<Golden, LoadError> {
        let lines = split_lines(text).map_err(LoadError::Malformed)?;
        let grammar = first_line(&lines)?;
        parse_body(grammar, &lines[1..]).map_err(LoadError::Malformed)
    }
}

// ---- lines and sections ------------------------------------------------------------------------------------------

type Line<'a> = (usize, &'a str);

fn split_lines(text: &str) -> PResult<Vec<Line<'_>>> {
    if text.starts_with('\u{feff}') {
        return fail(1, "the file starts with a byte order mark; remove it (goldens are UTF-8 without a BOM)");
    }
    let mut pieces: Vec<&str> = text.split('\n').collect();
    if pieces.last() == Some(&"") {
        pieces.pop();
    }
    let mut lines = Vec::with_capacity(pieces.len());
    for (i, piece) in pieces.into_iter().enumerate() {
        let n = i + 1;
        let line = piece.strip_suffix('\r').unwrap_or(piece);
        if line.contains('\0') {
            return fail(n, "a NUL byte");
        }
        if line.contains('\r') {
            return fail(n, "a carriage return that is not part of a CRLF line ending");
        }
        lines.push((n, line));
    }
    Ok(lines)
}

fn first_line(lines: &[Line<'_>]) -> Result<u32, LoadError> {
    let bad = |message: &str| Err(LoadError::Malformed(ParseError { line: 1, message: message.to_string() }));
    let Some(&(_, first)) = lines.first() else {
        return bad("the file is empty; the first line must be `vtee-golden N`");
    };
    let tokens: Vec<&str> = first.split_whitespace().collect();
    let [name, number] = tokens.as_slice() else {
        return bad("the first line must be `vtee-golden N`");
    };
    if *name != "vtee-golden" || number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return bad("the first line must be `vtee-golden N`");
    }
    match number.parse::<u32>() {
        Ok(0) => bad(
            "`vtee-golden 0` is the unreleased draft layout and is not supported; record the golden again with --update",
        ),
        Ok(n) if n > GRAMMAR_VERSION => Err(LoadError::Refused(Refusal::Grammar { golden: n, tool: GRAMMAR_VERSION })),
        Ok(n) => Ok(n),
        Err(_) => bad("the grammar number is too large"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionName {
    Masks,
    Text,
    Attrs,
    Assert,
    Screen(usize),
}

struct Section<'a> {
    name: SectionName,
    line: usize,
    body: Vec<Line<'a>>,
}

fn section_name(line: usize, text: &str) -> PResult<SectionName> {
    let trimmed = text.trim_end();
    let Some(inner) = trimmed.strip_prefix('[').and_then(|t| t.strip_suffix(']')) else {
        return fail(line, "a line starting with '[' must be a section header such as `[text]`");
    };
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    if tokens.join(" ") != inner {
        return fail(line, "malformed section header");
    }
    match tokens.as_slice() {
        ["masks"] => Ok(SectionName::Masks),
        ["text"] => Ok(SectionName::Text),
        ["attrs"] => Ok(SectionName::Attrs),
        ["assert"] => Ok(SectionName::Assert),
        ["exit"] => fail(line, "`[exit]` is a reserved section name and is not defined in grammar 1"),
        ["screen"] => fail(line, "`[screen]` needs a number: `[screen 1]`"),
        ["screen", n] if n.bytes().all(|b| b.is_ascii_digit()) => match n.parse::<usize>() {
            Ok(n) => Ok(SectionName::Screen(n)),
            Err(_) => fail(line, "the screen number is too large"),
        },
        _ => fail(line, format!("unknown section `[{inner}]`")),
    }
}

fn split_sections<'a>(lines: &[Line<'a>]) -> PResult<(Vec<Line<'a>>, Vec<Section<'a>>)> {
    let mut header = Vec::new();
    let mut sections: Vec<Section<'a>> = Vec::new();
    for &(n, text) in lines {
        if text.starts_with('[') {
            sections.push(Section { name: section_name(n, text)?, line: n, body: Vec::new() });
        } else if let Some(section) = sections.last_mut() {
            section.body.push((n, text));
        } else {
            header.push((n, text));
        }
    }
    Ok((header, sections))
}

// ---- the file ----------------------------------------------------------------------------------------------------

struct ScreenParts<'a> {
    line: usize,
    header: Vec<Line<'a>>,
    text: Option<Section<'a>>,
    attrs: Option<Section<'a>>,
    masks: Option<Section<'a>>,
    asserts: Option<Section<'a>>,
}

fn parse_body(grammar: u32, lines: &[Line<'_>]) -> PResult<Golden> {
    let (header, sections) = split_sections(lines)?;
    let file = file_header(&header)?;
    let mut shared: Option<Vec<Entry<Mask>>> = None;
    let mut parts: Vec<ScreenParts<'_>> = Vec::new();
    for section in sections {
        let Section { name, line, body } = section;
        if let SectionName::Screen(n) = name {
            let expected = parts.len() + 1;
            if n != expected {
                return fail(
                    line,
                    format!("`[screen {n}]` is out of order: the next screen block must be `[screen {expected}]`"),
                );
            }
            parts.push(ScreenParts { line, header: body, text: None, attrs: None, masks: None, asserts: None });
            continue;
        }
        let Some(current) = parts.last_mut() else {
            match name {
                SectionName::Masks => {
                    if shared.is_some() {
                        return fail(line, "two shared `[masks]` sections before the first screen");
                    }
                    shared = Some(mask_entries(&body)?);
                    continue;
                }
                _ => return fail(line, "this section belongs inside a `[screen N]` block"),
            }
        };
        let (slot, what) = match name {
            SectionName::Text => (&mut current.text, "[text]"),
            SectionName::Attrs => (&mut current.attrs, "[attrs]"),
            SectionName::Masks => (&mut current.masks, "[masks]"),
            SectionName::Assert => (&mut current.asserts, "[assert]"),
            SectionName::Screen(_) => unreachable!("handled above"),
        };
        if slot.is_some() {
            return fail(line, format!("two {what} sections in one screen"));
        }
        *slot = Some(Section { name, line, body });
    }
    if parts.is_empty() {
        return fail(0, "the file has no `[screen 1]` block");
    }
    let screens = parts.into_iter().map(build_screen).collect::<PResult<Vec<_>>>()?;
    let shared_masks = shared.unwrap_or_default();
    for mask in shared_masks.iter().filter_map(|e| if let Entry::Item(m) = e { Some(m) } else { None }) {
        for (i, screen) in screens.iter().enumerate() {
            if let Err(message) = check_region(&mask.region, screen.cols, screen.rows) {
                return fail(
                    mask.line,
                    format!(
                        "the shared mask does not fit screen {} ({}x{}): {message}",
                        i + 1,
                        screen.cols,
                        screen.rows
                    ),
                );
            }
        }
    }
    Ok(Golden {
        grammar,
        schema: file.schema,
        semantics: file.semantics,
        ignore: file.ignore,
        header_comments: file.comments,
        x_lines: file.x_lines,
        shared_masks,
        screens,
    })
}

// ---- headers -----------------------------------------------------------------------------------------------------

struct FileHeader {
    schema: u32,
    semantics: Semantics,
    ignore: Ignore,
    comments: Vec<String>,
    x_lines: Vec<String>,
}

const SCREEN_KEYS: [&str; 7] = ["size", "cursor", "modes", "alt_screen", "title", "wrapped", "name"];
const FILE_KEYS: [&str; 3] = ["vtee-golden", "schema", "semantics"];

/// Splits a header line into its key and value.
fn key_value(text: &str) -> (&str, &str) {
    let text = text.trim();
    match text.split_once(char::is_whitespace) {
        Some((key, value)) => (key, value.trim()),
        None => (text, ""),
    }
}

fn number(line: usize, what: &str, text: &str) -> PResult<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return fail(line, format!("{what} must be a decimal number, not {text:?}"));
    }
    text.parse().or_else(|_| fail(line, format!("{what} is too large")))
}

fn bool_value(line: usize, what: &str, text: &str) -> PResult<bool> {
    match text {
        "true" => Ok(true),
        "false" => Ok(false),
        other => fail(line, format!("{what} must be true or false, not {other:?}")),
    }
}

fn file_header(lines: &[Line<'_>]) -> PResult<FileHeader> {
    let mut schema = None;
    let mut semantics = None;
    let mut ignore = Ignore::default();
    let mut comments = Vec::new();
    let mut x_lines = Vec::new();
    for &(n, text) in lines {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with('#') {
            comments.push(text.to_string());
            continue;
        }
        let (key, value) = key_value(text);
        match key {
            "schema" => {
                if schema.is_some() {
                    return fail(n, "`schema` appears twice");
                }
                let value = number(n, "`schema`", value)?;
                schema = Some(u32::try_from(value).or_else(|_| fail(n, "`schema` is too large"))?);
            }
            "semantics" => {
                if semantics.is_some() {
                    return fail(n, "`semantics` appears twice");
                }
                semantics = Some(if value == "unknown" {
                    Semantics::Unknown
                } else {
                    let value = number(n, "`semantics` (a number, or the word unknown)", value)?;
                    Semantics::Number(u32::try_from(value).or_else(|_| fail(n, "`semantics` is too large"))?)
                });
            }
            "ignore" => parse_ignore(n, value, &mut ignore)?,
            key if key.starts_with("x-") => x_lines.push(text.to_string()),
            key if SCREEN_KEYS.contains(&key) => {
                return fail(n, format!("`{key}` belongs in a screen header, not in the file header"));
            }
            "vtee-golden" => return fail(n, "`vtee-golden` must be the first line and appear once"),
            other => {
                return fail(
                    n,
                    format!("unknown file header key `{other}` (keys that are informational start with `x-`)"),
                );
            }
        }
    }
    let Some(schema) = schema else { return fail(0, "the file header has no `schema` line") };
    let Some(semantics) = semantics else { return fail(0, "the file header has no `semantics` line") };
    Ok(FileHeader { schema, semantics, ignore, comments, x_lines })
}

fn parse_ignore(line: usize, value: &str, ignore: &mut Ignore) -> PResult<()> {
    let mut words = value.split_whitespace();
    let Some(part) = words.next() else { return fail(line, "`ignore` needs the part to ignore") };
    let rest: Vec<&str> = words.collect();
    let no_arguments = |part: &str| {
        if rest.is_empty() { Ok(()) } else { fail(line, format!("`ignore {part}` takes no arguments")) }
    };
    match part {
        "title" => {
            no_arguments(part)?;
            ignore.title = true;
        }
        "alt_screen" => {
            no_arguments(part)?;
            ignore.alt_screen = true;
        }
        "wrapped" => {
            no_arguments(part)?;
            ignore.wrapped = true;
        }
        "cursor" => {
            no_arguments(part)?;
            ignore.cursor_position = true;
            ignore.cursor_shape = true;
            ignore.cursor_hidden = true;
        }
        "cursor.position" => {
            no_arguments(part)?;
            ignore.cursor_position = true;
        }
        "cursor.shape" => {
            no_arguments(part)?;
            ignore.cursor_shape = true;
        }
        "cursor.hidden" => {
            no_arguments(part)?;
            ignore.cursor_hidden = true;
        }
        "modes" => {
            if rest.is_empty() {
                return fail(line, "`ignore modes` needs at least one mode token");
            }
            for token in rest {
                if !MODE_VOCABULARY.contains(&token) {
                    return fail(line, format!("`{token}` is not a mode token this build reports"));
                }
                ignore.modes.insert(token.to_string());
            }
        }
        "attrs" => {
            if rest.is_empty() {
                return fail(line, "`ignore attrs` needs at least one attribute name");
            }
            for name in rest {
                let Some(attr) = Attr::from_name(name) else {
                    return fail(line, format!("`{name}` is not an attribute a golden compares"));
                };
                ignore.attrs.insert(attr);
            }
        }
        other => return fail(line, format!("`ignore {other}` is not something a check can skip")),
    }
    Ok(())
}

struct ScreenHeader {
    cols: usize,
    rows: usize,
    cursor_row: usize,
    cursor_col: usize,
    cursor_shape: CursorShape,
    cursor_hidden: bool,
    modes: Vec<String>,
    alt_screen: bool,
    title: Option<String>,
    wrapped: Vec<usize>,
    name: Option<String>,
    ignore: Ignore,
    comments: Vec<String>,
    x_lines: Vec<String>,
}

fn screen_header(screen_line: usize, lines: &[Line<'_>]) -> PResult<ScreenHeader> {
    let mut size = None;
    let mut cursor = None;
    let mut modes = None;
    let mut alt_screen = None;
    let mut title: Option<Option<String>> = None;
    let mut wrapped: Option<Vec<usize>> = None;
    let mut name = None;
    let mut ignore = Ignore::default();
    let mut comments = Vec::new();
    let mut x_lines = Vec::new();
    let mut wrapped_line = 0;
    let mut cursor_line = 0;
    for &(n, text) in lines {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with('#') {
            comments.push(text.to_string());
            continue;
        }
        let (key, value) = key_value(text);
        let duplicate =
            |seen: bool| if seen { fail(n, format!("`{key}` appears twice in one screen header")) } else { Ok(()) };
        match key {
            "size" => {
                duplicate(size.is_some())?;
                size = Some(parse_size(n, value)?);
            }
            "cursor" => {
                duplicate(cursor.is_some())?;
                cursor_line = n;
                cursor = Some(parse_cursor(n, value)?);
            }
            "modes" => {
                duplicate(modes.is_some())?;
                for token in value.split_whitespace() {
                    if !MODE_VOCABULARY.contains(&token) {
                        return fail(n, format!("`{token}` is not a mode token this build reports"));
                    }
                }
                modes = Some(sorted_modes(value.split_whitespace()));
            }
            "alt_screen" => {
                duplicate(alt_screen.is_some())?;
                alt_screen = Some(bool_value(n, "`alt_screen`", value)?);
            }
            "title" => {
                duplicate(title.is_some())?;
                title = Some(if value == "null" { None } else { Some(whole_string(n, "`title`", value)?) });
            }
            "wrapped" => {
                duplicate(wrapped.is_some())?;
                wrapped_line = n;
                let mut rows = BTreeSet::new();
                for word in value.split_whitespace() {
                    rows.insert(number(n, "a `wrapped` row", word)?);
                }
                wrapped = Some(rows.into_iter().collect());
            }
            "name" => {
                duplicate(name.is_some())?;
                name = Some(whole_string(n, "`name`", value)?);
            }
            "ignore" => parse_ignore(n, value, &mut ignore)?,
            key if key.starts_with("x-") => x_lines.push(text.to_string()),
            key if FILE_KEYS.contains(&key) => {
                return fail(n, format!("`{key}` belongs in the file header, not in a screen header"));
            }
            other => {
                return fail(
                    n,
                    format!("unknown screen header key `{other}` (keys that are informational start with `x-`)"),
                );
            }
        }
    }
    let Some((cols, rows)) = size else { return fail(screen_line, "the screen header has no `size` line") };
    let Some((cursor_row, cursor_col, cursor_shape, cursor_hidden)) = cursor else {
        return fail(screen_line, "the screen header has no `cursor` line");
    };
    let Some(modes) = modes else { return fail(screen_line, "the screen header has no `modes` line") };
    let Some(alt_screen) = alt_screen else { return fail(screen_line, "the screen header has no `alt_screen` line") };
    let Some(title) = title else { return fail(screen_line, "the screen header has no `title` line") };
    if cursor_row >= rows || cursor_col >= cols {
        return fail(
            cursor_line,
            format!("the cursor (row {cursor_row}, col {cursor_col}) is outside the {cols}x{rows} screen"),
        );
    }
    let wrapped = wrapped.unwrap_or_default();
    if let Some(&row) = wrapped.iter().find(|r| **r >= rows) {
        return fail(wrapped_line, format!("wrapped row {row} is outside the {rows}-row screen"));
    }
    Ok(ScreenHeader {
        cols,
        rows,
        cursor_row,
        cursor_col,
        cursor_shape,
        cursor_hidden,
        modes,
        alt_screen,
        title,
        wrapped,
        name,
        ignore,
        comments,
        x_lines,
    })
}

fn whole_string(line: usize, what: &str, value: &str) -> PResult<String> {
    match escape::decode_string(value) {
        Ok((s, used)) if used == value.len() => Ok(s),
        Ok(_) => fail(line, format!("{what} has text after the string literal")),
        Err(e) => fail(line, format!("{what}: {e}")),
    }
}

fn parse_size(line: usize, value: &str) -> PResult<(usize, usize)> {
    let canonical =
        |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s == "0" || !s.starts_with('0'));
    let Some((w, h)) = value.split_once('x') else { return fail(line, "`size` must be WxH, for example `size 80x24`") };
    if !canonical(w) || !canonical(h) {
        return fail(line, "`size` must be WxH in decimal with no leading zeros");
    }
    let (w, h): (usize, usize) = (w.parse().unwrap_or(usize::MAX), h.parse().unwrap_or(usize::MAX));
    if !(1..=MAX_DIMENSION).contains(&w) || !(1..=MAX_DIMENSION).contains(&h) {
        return fail(line, format!("`size` must be between 1x1 and {MAX_DIMENSION}x{MAX_DIMENSION}"));
    }
    Ok((w, h))
}

fn parse_cursor(line: usize, value: &str) -> PResult<(usize, usize, CursorShape, bool)> {
    let mut row = None;
    let mut col = None;
    let mut shape = None;
    let mut hidden = None;
    for word in value.split_whitespace() {
        let Some((key, v)) = word.split_once('=') else {
            return fail(line, format!("`cursor` field {word:?} must be name=value"));
        };
        let again =
            |seen: bool| if seen { fail(line, format!("`cursor` field `{key}` appears twice")) } else { Ok(()) };
        match key {
            "row" => {
                again(row.is_some())?;
                row = Some(number(line, "`cursor` row", v)?);
            }
            "col" => {
                again(col.is_some())?;
                col = Some(number(line, "`cursor` col", v)?);
            }
            "shape" => {
                again(shape.is_some())?;
                shape = Some(match v {
                    "block" => CursorShape::Block,
                    "underline" => CursorShape::Underline,
                    "bar" => CursorShape::Bar,
                    other => {
                        return fail(line, format!("`cursor` shape must be block, underline or bar, not {other:?}"));
                    }
                });
            }
            "hidden" => {
                again(hidden.is_some())?;
                hidden = Some(bool_value(line, "`cursor` hidden", v)?);
            }
            other => return fail(line, format!("unknown `cursor` field `{other}`")),
        }
    }
    match (row, col, shape, hidden) {
        (Some(r), Some(c), Some(s), Some(h)) => Ok((r, c, s, h)),
        _ => fail(line, "`cursor` needs row=, col=, shape= and hidden="),
    }
}

// ---- a screen ----------------------------------------------------------------------------------------------------

fn build_screen(parts: ScreenParts<'_>) -> PResult<GoldenScreen> {
    let header = screen_header(parts.line, &parts.header)?;
    let (cols, rows) = (header.cols, header.rows);
    let Some(text) = parts.text else { return fail(parts.line, "the screen has no `[text]` section") };
    let mut cells = parse_text(&text.body, cols, rows, text.line)?;
    if let Some(attrs) = parts.attrs {
        apply_attrs(&attrs.body, &mut cells)?;
    }
    let masks = match parts.masks {
        Some(section) => mask_entries(&section.body)?,
        None => Vec::new(),
    };
    for mask in masks.iter().filter_map(|e| if let Entry::Item(m) = e { Some(m) } else { None }) {
        if let Err(message) = check_region(&mask.region, cols, rows) {
            return fail(mask.line, message);
        }
    }
    let asserts = match parts.asserts {
        Some(section) => assert_entries(&section.body)?,
        None => Vec::new(),
    };
    for assertion in asserts.iter().filter_map(|e| if let Entry::Item(a) = e { Some(a) } else { None }) {
        if let Err(message) = check_region(&assertion.region, cols, rows) {
            return fail(assertion.line, message);
        }
    }
    Ok(GoldenScreen {
        line: parts.line,
        cols,
        rows,
        cells,
        cursor_row: header.cursor_row,
        cursor_col: header.cursor_col,
        cursor_shape: header.cursor_shape,
        cursor_hidden: header.cursor_hidden,
        modes: header.modes,
        alt_screen: header.alt_screen,
        title: header.title,
        wrapped: header.wrapped,
        name: header.name,
        ignore: header.ignore,
        header_comments: header.comments,
        x_lines: header.x_lines,
        masks,
        asserts,
    })
}

fn check_region(region: &Region, cols: usize, rows: usize) -> Result<(), String> {
    if let Some((_, last)) = region.rows
        && last >= rows
    {
        return Err(format!("row {last} is outside the {rows}-row screen"));
    }
    if let Some((_, last)) = region.cols
        && last >= cols
    {
        return Err(format!("column {last} is outside the {cols}-column screen"));
    }
    Ok(())
}

fn parse_text(body: &[Line<'_>], cols: usize, rows: usize, section_line: usize) -> PResult<Vec<Vec<Cell>>> {
    let mut grid = Vec::with_capacity(rows);
    for &(n, text) in body {
        if text.trim().is_empty() {
            continue;
        }
        if text.starts_with('#') {
            return fail(n, "comments are not allowed in `[text]`");
        }
        let row = grid.len();
        if row >= rows {
            return fail(n, format!("`[text]` has more than the {rows} rows of the screen"));
        }
        let Some((label, tokens)) = text.split_once('|') else {
            return fail(n, "a `[text]` row needs a '|' after its row number");
        };
        if number(n, "the row label", label.trim())? != row {
            return fail(n, format!("expected the label of row {row}, found {:?}", label.trim()));
        }
        let decoded = match escape::decode_row(tokens) {
            Ok(cells) => cells,
            Err(e) => return fail(n, e.to_string()),
        };
        if decoded.len() > cols {
            return fail(n, format!("the row describes {} cells but the screen has {cols} columns", decoded.len()));
        }
        let mut cells: Vec<Cell> = decoded.into_iter().map(|ch| Cell { ch, ..Cell::default() }).collect();
        cells.resize_with(cols, Cell::default);
        grid.push(cells);
    }
    if grid.len() != rows {
        return fail(section_line, format!("`[text]` has {} rows but the screen has {rows}", grid.len()));
    }
    Ok(grid)
}

// ---- the tokens of a line ----------------------------------------------------------------------------------------

enum Value {
    Plain(String),
    /// The source between the slashes of `/RE/`.
    Pattern(String),
    Str(String),
}

struct Token {
    key: String,
    value: Option<Value>,
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().expect("index inside the text");
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let start = i;
        while i < text.len() {
            let c = text[i..].chars().next().expect("index inside the text");
            if c == '=' || c.is_whitespace() {
                break;
            }
            i += c.len_utf8();
        }
        let key = text[start..i].to_string();
        if key.is_empty() {
            return Err("a token cannot start with '='".to_string());
        }
        if !text[i..].starts_with('=') {
            tokens.push(Token { key, value: None });
            continue;
        }
        i += 1;
        let value = if text[i..].starts_with('/') {
            let body = &text[i + 1..];
            let mut escaped = false;
            let mut end = None;
            for (j, c) in body.char_indices() {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '/' {
                    end = Some(j);
                    break;
                }
            }
            let Some(end) = end else { return Err(format!("the pattern of `{key}` has no closing '/'")) };
            i += 1 + end + 1;
            Value::Pattern(body[..end].to_string())
        } else if text[i..].starts_with('"') {
            let (s, used) = escape::decode_string(&text[i..]).map_err(|e| format!("`{key}`: {e}"))?;
            i += used;
            Value::Str(s)
        } else {
            let from = i;
            while i < text.len() {
                let c = text[i..].chars().next().expect("index inside the text");
                if c.is_whitespace() {
                    break;
                }
                i += c.len_utf8();
            }
            Value::Plain(text[from..i].to_string())
        };
        if let Some(next) = text[i..].chars().next()
            && !next.is_whitespace()
        {
            return Err(format!(
                "unexpected text after the value of `{key}`: a pattern ends at the next '/' that is not escaped"
            ));
        }
        tokens.push(Token { key, value: Some(value) });
    }
    Ok(tokens)
}

fn compile(source: &str) -> Result<Pattern, String> {
    Pattern::new(source).map_err(|e| e.to_string())
}

fn range_value(key: &str, value: &Option<Value>) -> Result<(usize, usize), String> {
    let Some(Value::Plain(text)) = value else { return Err(format!("`{key}` needs a number or a range N-M")) };
    let digits = |s: &str| -> Result<usize, String> {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("`{key}={text}` is not a number or a range N-M"));
        }
        s.parse().map_err(|_| format!("`{key}={text}` is too large"))
    };
    let (first, last) = match text.split_once('-') {
        Some((a, b)) => (digits(a)?, digits(b)?),
        None => {
            let n = digits(text)?;
            (n, n)
        }
    };
    if first > last {
        return Err(format!("`{key}={text}` runs backwards"));
    }
    Ok((first, last))
}

/// The region tokens shared by `mask` and `assert`.
#[derive(Default)]
struct RegionParts {
    rows: Option<(usize, usize)>,
    cols: Option<(usize, usize)>,
    find: Option<Pattern>,
}

impl RegionParts {
    /// Takes the token if it is a region token. Returns false when it is not one.
    fn take(&mut self, token: &Token) -> Result<bool, String> {
        let again = |seen: bool| if seen { Err(format!("`{}` appears twice", token.key)) } else { Ok(()) };
        match token.key.as_str() {
            "row" | "rows" => {
                again(self.rows.is_some())?;
                self.rows = Some(range_value(&token.key, &token.value)?);
            }
            "cols" => {
                again(self.cols.is_some())?;
                self.cols = Some(range_value(&token.key, &token.value)?);
            }
            "find" => {
                again(self.find.is_some())?;
                let Some(Value::Pattern(source)) = &token.value else {
                    return Err("`find` needs a pattern: find=/RE/".to_string());
                };
                self.find = Some(compile(source)?);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn finish(self) -> Result<Region, String> {
        if self.rows.is_none() && self.find.is_none() {
            return Err("a region needs row=, rows= or find=".to_string());
        }
        Ok(Region { rows: self.rows, cols: self.cols, find: self.find })
    }
}

fn keyword_rest<'a>(keyword: &str, text: &'a str) -> Option<&'a str> {
    let rest = text.strip_prefix(keyword)?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then_some(rest)
}

fn mask_entries(body: &[Line<'_>]) -> PResult<Vec<Entry<Mask>>> {
    let mut entries = Vec::new();
    for &(n, text) in body {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with('#') {
            entries.push(Entry::Comment(text.to_string()));
            continue;
        }
        let Some(rest) = keyword_rest("mask", text) else {
            return fail(n, "a line in `[masks]` must be a `mask ...` line or a comment");
        };
        let mask = parse_mask(n, text, rest).or_else(|message| fail(n, message))?;
        entries.push(Entry::Item(mask));
    }
    Ok(entries)
}

fn parse_mask(line: usize, raw: &str, rest: &str) -> Result<Mask, String> {
    let mut region = RegionParts::default();
    let mut text = None;
    let mut attrs_ignore = false;
    for token in tokenize(rest)? {
        if region.take(&token)? {
            continue;
        }
        match token.key.as_str() {
            "text" => {
                if text.is_some() {
                    return Err("`text` appears twice".to_string());
                }
                text = Some(match &token.value {
                    Some(Value::Plain(p)) if p == "ignore" => MaskText::Ignore,
                    Some(Value::Pattern(source)) => MaskText::Pattern(compile(source)?),
                    _ => return Err("a mask's `text` must be `ignore` or a pattern /RE/".to_string()),
                });
            }
            "attrs" => {
                if attrs_ignore {
                    return Err("`attrs` appears twice".to_string());
                }
                match &token.value {
                    Some(Value::Plain(p)) if p == "ignore" => attrs_ignore = true,
                    _ => return Err("a mask's `attrs` must be `ignore`".to_string()),
                }
            }
            other => return Err(format!("unknown token `{other}` in a mask")),
        }
    }
    if text.is_none() && !attrs_ignore {
        return Err("a mask must say text= or attrs=ignore".to_string());
    }
    Ok(Mask { line, raw: raw.to_string(), region: region.finish()?, text, attrs_ignore })
}

fn assert_entries(body: &[Line<'_>]) -> PResult<Vec<Entry<Assert>>> {
    let mut entries = Vec::new();
    for &(n, text) in body {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with('#') {
            entries.push(Entry::Comment(text.to_string()));
            continue;
        }
        let Some(rest) = keyword_rest("assert", text) else {
            return fail(n, "a line in `[assert]` must be an `assert ...` line or a comment");
        };
        let assertion = parse_assert(n, text, rest).or_else(|message| fail(n, message))?;
        entries.push(Entry::Item(assertion));
    }
    Ok(entries)
}

fn color_value(key: &str, value: &Option<Value>) -> Result<Color, String> {
    match value {
        Some(Value::Plain(text)) => text.parse().map_err(|e| format!("`{key}`: {e}")),
        _ => Err(format!("`{key}` needs a colour: default, idx:N or rgb:#rrggbb")),
    }
}

fn parse_assert(line: usize, raw: &str, rest: &str) -> Result<Assert, String> {
    let mut region = RegionParts::default();
    let mut text = None;
    let mut attrs: Vec<AttrCheck> = Vec::new();
    let mut seen: BTreeSet<Attr> = BTreeSet::new();
    for token in tokenize(rest)? {
        if region.take(&token)? {
            continue;
        }
        let (negated, key) = match token.key.strip_prefix('!') {
            Some(k) => (true, k),
            None => (false, token.key.as_str()),
        };
        if key == "text" && !negated {
            if text.is_some() {
                return Err("`text` appears twice".to_string());
            }
            text = Some(match &token.value {
                Some(Value::Str(s)) => AssertText::Exact(s.clone()),
                Some(Value::Pattern(source)) => AssertText::Pattern(compile(source)?),
                _ => return Err("an assertion's `text` must be a string \"...\" or a pattern /RE/".to_string()),
            });
            continue;
        }
        let Some(attr) = Attr::from_name(key) else {
            return Err(format!("unknown token `{}` in an assertion", token.key));
        };
        if !seen.insert(attr) {
            return Err(format!("the attribute `{}` is constrained twice", attr.name()));
        }
        let check = if negated {
            if token.value.is_some() {
                return Err(format!("`{}` takes no value", token.key));
            }
            match attr {
                Attr::Link => AttrCheck::NoLink,
                a if a.is_flag() => AttrCheck::Flag(a, false),
                _ => return Err(format!("`!{key}` is not allowed: only flags and link can be negated")),
            }
        } else {
            match attr {
                Attr::Fg => AttrCheck::Fg(color_value(key, &token.value)?),
                Attr::Bg => AttrCheck::Bg(color_value(key, &token.value)?),
                Attr::Ulcolor => AttrCheck::Ulcolor(color_value(key, &token.value)?),
                Attr::Underline => match &token.value {
                    Some(Value::Plain(name)) => AttrCheck::Underline(
                        underline_from_name(name)
                            .ok_or_else(|| format!("`underline={name}` is not an underline style or none"))?,
                    ),
                    _ => {
                        return Err(
                            "`underline` needs a style: single, double, curly, dotted, dashed or none".to_string()
                        );
                    }
                },
                Attr::Link => match &token.value {
                    Some(Value::Str(uri)) => AttrCheck::Link(uri.clone()),
                    _ => return Err("`link` needs a string: link=\"URI\"".to_string()),
                },
                flag => {
                    if token.value.is_some() {
                        return Err(format!("`{key}` takes no value"));
                    }
                    AttrCheck::Flag(flag, true)
                }
            }
        };
        attrs.push(check);
    }
    if text.is_none() && attrs.is_empty() {
        return Err("an assertion must say text= or at least one attribute".to_string());
    }
    let region = region.finish()?;
    if matches!(text, Some(AssertText::Exact(_))) {
        if region.find.is_some() {
            return Err("text=\"...\" cannot be combined with find= (the pattern is the text constraint)".to_string());
        }
        if region.rows.is_some_and(|(a, b)| a != b) {
            return Err("text=\"...\" needs a region of a single row".to_string());
        }
    }
    Ok(Assert { line, raw: raw.to_string(), region, text, attrs })
}

// ---- [attrs] -----------------------------------------------------------------------------------------------------

fn apply_attrs(body: &[Line<'_>], cells: &mut [Vec<Cell>]) -> PResult<()> {
    let rows = cells.len();
    let cols = cells.first().map_or(0, Vec::len);
    let mut covered = vec![vec![false; cols]; rows];
    for &(n, text) in body {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if text.starts_with('#') {
            return fail(n, "comments are not allowed in `[attrs]`");
        }
        let Some((label, rest)) = text.split_once(':') else {
            return fail(n, "an `[attrs]` run must be `ROW: SPAN TOKENS`");
        };
        let row = number(n, "the row of an `[attrs]` run", label.trim())?;
        if row >= rows {
            return fail(n, format!("row {row} is outside the {rows}-row screen"));
        }
        let rest = rest.trim_start();
        let (span, tokens) = match rest.split_once(char::is_whitespace) {
            Some((span, tokens)) => (span, tokens),
            None => (rest, ""),
        };
        let (first, last) = match span.split_once('-') {
            Some((a, b)) => (number(n, "a column", a)?, number(n, "a column", b)?),
            None => {
                let c = number(n, "a column", span)?;
                (c, c)
            }
        };
        if first > last {
            return fail(n, format!("the span {span} runs backwards"));
        }
        if last >= cols {
            return fail(n, format!("column {last} is outside the {cols}-column screen"));
        }
        let template = attr_template(tokens).or_else(|message| fail(n, message))?;
        for (c, done) in covered[row].iter_mut().enumerate().take(last + 1).skip(first) {
            if *done {
                return fail(n, format!("the run overlaps another run at row {row} column {c}"));
            }
            *done = true;
            let cell = &mut cells[row][c];
            cell.fg = template.fg;
            cell.bg = template.bg;
            cell.underline = template.underline;
            cell.underline_color = template.underline_color;
            cell.hyperlink = template.hyperlink.clone();
            cell.bold = template.bold;
            cell.dim = template.dim;
            cell.italic = template.italic;
            cell.reverse = template.reverse;
            cell.strike = template.strike;
            cell.conceal = template.conceal;
        }
    }
    Ok(())
}

/// The attributes of an `[attrs]` run as a cell (its text is not used).
fn attr_template(tokens: &str) -> Result<Cell, String> {
    let mut cell = Cell::default();
    let mut seen: BTreeSet<Attr> = BTreeSet::new();
    for token in tokenize(tokens)? {
        let Some(attr) = Attr::from_name(&token.key) else {
            return Err(format!("`{}` is not an attribute a golden compares", token.key));
        };
        if !seen.insert(attr) {
            return Err(format!("the attribute `{}` is repeated in the run", attr.name()));
        }
        match attr {
            Attr::Fg => cell.fg = color_value("fg", &token.value)?,
            Attr::Bg => cell.bg = color_value("bg", &token.value)?,
            Attr::Ulcolor => cell.underline_color = color_value("ulcolor", &token.value)?,
            Attr::Underline => match &token.value {
                Some(Value::Plain(name)) => {
                    cell.underline = underline_from_name(name)
                        .ok_or_else(|| format!("`underline={name}` is not an underline style"))?;
                }
                _ => return Err("`underline` needs a style".to_string()),
            },
            Attr::Link => match token.value {
                Some(Value::Str(uri)) => cell.hyperlink = Some(uri),
                _ => return Err("`link` needs a string: link=\"URI\"".to_string()),
            },
            flag => {
                if token.value.is_some() {
                    return Err(format!("`{}` takes no value", flag.name()));
                }
                match flag {
                    Attr::Bold => cell.bold = true,
                    Attr::Dim => cell.dim = true,
                    Attr::Italic => cell.italic = true,
                    Attr::Reverse => cell.reverse = true,
                    Attr::Strike => cell.strike = true,
                    _ => cell.conceal = true,
                }
            }
        }
    }
    if cell == Cell::default() {
        return Err("a run needs at least one attribute that is not the default".to_string());
    }
    Ok(cell)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A2: &str = "vtee-golden 1
schema 1
semantics 1

[screen 1]
size 6x2
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|ab 12
1|

[attrs]
0: 3-4 bold

[masks]
mask row=0 cols=3-4 text=/[0-9]+/
";

    fn malformed(text: &str) -> ParseError {
        match Golden::parse_str(text) {
            Err(LoadError::Malformed(e)) => e,
            other => panic!("expected a malformed golden, got {other:?}"),
        }
    }

    #[test]
    fn the_worked_example_reads() {
        let g = Golden::parse_str(A2).unwrap();
        assert_eq!((g.grammar, g.schema, g.semantics), (1, 1, Semantics::Number(1)));
        let s = &g.screens[0];
        assert_eq!((s.cols, s.rows), (6, 2));
        assert_eq!(s.row_text(0), "ab 12");
        assert!(s.cells[0][3].bold && s.cells[0][4].bold && !s.cells[0][2].bold);
        let mask = s.mask_items().next().unwrap();
        assert_eq!(mask.line, 20);
        assert_eq!(mask.region.cols, Some((3, 4)));
    }

    #[test]
    fn crlf_and_a_missing_final_newline_are_accepted() {
        let crlf = A2.replace('\n', "\r\n");
        assert!(Golden::parse_str(&crlf).is_ok());
        assert!(Golden::parse_str(A2.trim_end()).is_ok());
    }

    #[test]
    fn what_a_reader_refuses_to_read() {
        assert_eq!(malformed(&format!("\u{feff}{A2}")).line, 1);
        assert!(malformed(&A2.replace("0|ab 12", "0|ab\r 12")).message.contains("carriage return"));
        assert!(malformed(&A2.replace("0|ab 12", "0|ab\0 12")).message.contains("NUL"));
        assert!(malformed(&A2.replace("vtee-golden 1", "vtee-golden 0")).message.contains("draft"));
        assert!(malformed(&A2.replace("[screen 1]", "[screen 2]")).message.contains("out of order"));
        assert!(malformed(&A2.replace("[text]", "[exit]\n\n[text]")).message.contains("reserved"));
        assert!(malformed(&A2.replace("0: 3-4 bold", "0: 3-4 blink")).message.contains("blink"));
        assert!(malformed(&A2.replace("0: 3-4 bold", "0: 3-4 bold\n0: 4-5 dim")).message.contains("overlaps"));
        assert!(malformed(&A2.replace("title null", "title null\ntitle null")).message.contains("twice"));
        assert!(malformed(&A2.replace("schema 1", "schema 1\nsize 6x2")).message.contains("screen header"));
        assert!(
            malformed(&A2.replace("mask row=0 cols=3-4 text=/[0-9]+/", "mask row=0 cols=3-9 text=ignore"))
                .message
                .contains("outside")
        );
        assert!(malformed(&A2.replace("1|\n", "")).message.contains("1 rows"));
    }

    #[test]
    fn a_newer_grammar_is_refused_not_read() {
        match Golden::parse_str(&A2.replace("vtee-golden 1", "vtee-golden 2\nnew-key x")) {
            Err(LoadError::Refused(Refusal::Grammar { golden: 2, tool: 1 })) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_patterns_of_a_mask_may_contain_spaces_and_escaped_slashes() {
        let text = A2.replace("mask row=0 cols=3-4 text=/[0-9]+/", r"mask find=/a b\/c/ attrs=ignore");
        let g = Golden::parse_str(&text).unwrap();
        let mask = g.screens[0].mask_items().next().unwrap();
        assert_eq!(mask.region.find.as_ref().unwrap().source(), r"a b\/c");
        assert!(malformed(&A2.replace("text=/[0-9]+/", "text=/a/b/")).message.contains("unexpected text"));
    }
}
