//! What a parsed golden holds (design section 6).

use std::collections::BTreeSet;
use std::fmt;

use super::escape;
use super::pattern::Pattern;
use crate::snapshot::{Cell, Color, CursorShape, UnderlineStyle};

/// The grammar number this build reads and writes (`vtee-golden 1`).
pub const GRAMMAR_VERSION: u32 = 1;

/// The mode tokens a snapshot reports at semantics 1 and a golden may name (section 3.8).
pub const MODE_VOCABULARY: [&str; 8] = ["?1", "?1000", "?1002", "?1003", "?1005", "?1006", "?2004", "DECKPAM"];

/// The sort key of a mode token: private modes by number, then names alphabetically.
pub fn mode_key(token: &str) -> (u8, u32, &str) {
    match token.strip_prefix('?').and_then(|n| n.parse::<u32>().ok()) {
        Some(n) => (0, n, ""),
        None => (1, 0, token),
    }
}

/// A set of mode tokens as a sorted, de-duplicated list.
pub fn sorted_modes<I, S>(tokens: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut modes: Vec<String> = tokens.into_iter().map(|t| t.as_ref().to_string()).collect();
    modes.sort_by(|a, b| mode_key(a).cmp(&mode_key(b)));
    modes.dedup();
    modes
}

/// A cell attribute a golden compares, in the order a writer lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attr {
    Fg,
    Bg,
    Underline,
    Ulcolor,
    Link,
    Bold,
    Dim,
    Italic,
    Reverse,
    Strike,
    Conceal,
}

impl Attr {
    /// Every attribute, in the writer's order.
    pub const ALL: [Attr; 11] = [
        Attr::Fg,
        Attr::Bg,
        Attr::Underline,
        Attr::Ulcolor,
        Attr::Link,
        Attr::Bold,
        Attr::Dim,
        Attr::Italic,
        Attr::Reverse,
        Attr::Strike,
        Attr::Conceal,
    ];

    /// The name used in goldens.
    pub fn name(self) -> &'static str {
        match self {
            Attr::Fg => "fg",
            Attr::Bg => "bg",
            Attr::Underline => "underline",
            Attr::Ulcolor => "ulcolor",
            Attr::Link => "link",
            Attr::Bold => "bold",
            Attr::Dim => "dim",
            Attr::Italic => "italic",
            Attr::Reverse => "reverse",
            Attr::Strike => "strike",
            Attr::Conceal => "conceal",
        }
    }

    /// The attribute called `name`, if there is one.
    pub fn from_name(name: &str) -> Option<Attr> {
        Attr::ALL.into_iter().find(|a| a.name() == name)
    }

    /// True for the six on/off attributes.
    pub fn is_flag(self) -> bool {
        matches!(self, Attr::Bold | Attr::Dim | Attr::Italic | Attr::Reverse | Attr::Strike | Attr::Conceal)
    }
}

/// The name of an underline style as written in goldens (`none` for no underline).
pub fn underline_name(style: UnderlineStyle) -> &'static str {
    match style {
        UnderlineStyle::None => "none",
        UnderlineStyle::Single => "single",
        UnderlineStyle::Double => "double",
        UnderlineStyle::Curly => "curly",
        UnderlineStyle::Dotted => "dotted",
        UnderlineStyle::Dashed => "dashed",
    }
}

/// The underline style called `name` (`none` included).
pub fn underline_from_name(name: &str) -> Option<UnderlineStyle> {
    [
        UnderlineStyle::None,
        UnderlineStyle::Single,
        UnderlineStyle::Double,
        UnderlineStyle::Curly,
        UnderlineStyle::Dotted,
        UnderlineStyle::Dashed,
    ]
    .into_iter()
    .find(|s| underline_name(*s) == name)
}

/// The shape name used in goldens and snapshots.
pub fn shape_name(shape: CursorShape) -> &'static str {
    match shape {
        CursorShape::Block => "block",
        CursorShape::Underline => "underline",
        CursorShape::Bar => "bar",
    }
}

/// The attribute tokens of a cell in the writer's order, leaving out defaults and the attributes in `ignored`:
/// `fg=C bg=C underline=STYLE ulcolor=C link="URI" bold dim italic reverse strike conceal`.
pub fn attr_tokens(cell: &Cell, ignored: &BTreeSet<Attr>) -> Vec<String> {
    let mut tokens = Vec::new();
    for attr in Attr::ALL {
        if ignored.contains(&attr) {
            continue;
        }
        let token = match attr {
            Attr::Fg => (cell.fg != Color::Default).then(|| format!("fg={}", cell.fg)),
            Attr::Bg => (cell.bg != Color::Default).then(|| format!("bg={}", cell.bg)),
            Attr::Underline => (cell.underline != UnderlineStyle::None)
                .then(|| format!("underline={}", underline_name(cell.underline))),
            Attr::Ulcolor => {
                (cell.underline_color != Color::Default).then(|| format!("ulcolor={}", cell.underline_color))
            }
            Attr::Link => cell.hyperlink.as_ref().map(|uri| format!("link={}", escape::encode_string(uri))),
            Attr::Bold => cell.bold.then(|| "bold".to_string()),
            Attr::Dim => cell.dim.then(|| "dim".to_string()),
            Attr::Italic => cell.italic.then(|| "italic".to_string()),
            Attr::Reverse => cell.reverse.then(|| "reverse".to_string()),
            Attr::Strike => cell.strike.then(|| "strike".to_string()),
            Attr::Conceal => cell.conceal.then(|| "conceal".to_string()),
        };
        tokens.extend(token);
    }
    tokens
}

/// Parts of a screen a check can skip (`ignore` lines). A screen skips the union of the file's and its own.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ignore {
    pub title: bool,
    pub alt_screen: bool,
    pub wrapped: bool,
    pub cursor_position: bool,
    pub cursor_shape: bool,
    pub cursor_hidden: bool,
    pub modes: BTreeSet<String>,
    pub attrs: BTreeSet<Attr>,
}

impl Ignore {
    /// True when nothing is ignored.
    pub fn is_empty(&self) -> bool {
        *self == Ignore::default()
    }

    /// Everything either side ignores.
    pub fn union(&self, other: &Ignore) -> Ignore {
        Ignore {
            title: self.title || other.title,
            alt_screen: self.alt_screen || other.alt_screen,
            wrapped: self.wrapped || other.wrapped,
            cursor_position: self.cursor_position || other.cursor_position,
            cursor_shape: self.cursor_shape || other.cursor_shape,
            cursor_hidden: self.cursor_hidden || other.cursor_hidden,
            modes: self.modes.union(&other.modes).cloned().collect(),
            attrs: self.attrs.union(&other.attrs).copied().collect(),
        }
    }

    /// The canonical `ignore` lines (without the leading `ignore `).
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if !self.modes.is_empty() {
            let modes = sorted_modes(&self.modes);
            lines.push(format!("modes {}", modes.join(" ")));
        }
        if !self.attrs.is_empty() {
            let names: Vec<&str> = self.attrs.iter().map(|a| a.name()).collect();
            lines.push(format!("attrs {}", names.join(" ")));
        }
        if self.cursor_position && self.cursor_shape && self.cursor_hidden {
            lines.push("cursor".to_string());
        } else {
            if self.cursor_position {
                lines.push("cursor.position".to_string());
            }
            if self.cursor_shape {
                lines.push("cursor.shape".to_string());
            }
            if self.cursor_hidden {
                lines.push("cursor.hidden".to_string());
            }
        }
        if self.alt_screen {
            lines.push("alt_screen".to_string());
        }
        if self.title {
            lines.push("title".to_string());
        }
        if self.wrapped {
            lines.push("wrapped".to_string());
        }
        lines
    }
}

/// The `semantics` stamp of a golden: the number it was recorded under, or `unknown` (a migration could not recover
/// it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantics {
    Number(u32),
    Unknown,
}

impl fmt::Display for Semantics {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Semantics::Number(n) => write!(f, "{n}"),
            Semantics::Unknown => f.write_str("unknown"),
        }
    }
}

/// A line of a `[masks]` or `[assert]` section: kept so a re-record can leave comments where they were.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry<T> {
    /// A `#` comment line, without trailing whitespace.
    Comment(String),
    Item(T),
}

/// Rows and columns of a mask or assertion. Rows are 0-based and inclusive; a region has `rows`, a `find` pattern, or
/// both (`find=/RE/ rows=2-4`).
#[derive(Debug, Clone)]
pub struct Region {
    pub rows: Option<(usize, usize)>,
    pub cols: Option<(usize, usize)>,
    pub find: Option<Pattern>,
}

/// What a mask does to the text of its cells.
#[derive(Debug, Clone)]
pub enum MaskText {
    /// `text=ignore`: not compared.
    Ignore,
    /// `text=/RE/`: not compared with the stored text, but must match the pattern.
    Pattern(Pattern),
}

/// `mask REGION [text=ignore | text=/RE/] [attrs=ignore]`.
#[derive(Debug, Clone)]
pub struct Mask {
    /// The 1-based line of the golden file.
    pub line: usize,
    /// The line as written, trimmed.
    pub raw: String,
    pub region: Region,
    pub text: Option<MaskText>,
    pub attrs_ignore: bool,
}

/// The text constraint of an assertion.
#[derive(Debug, Clone)]
pub enum AssertText {
    Exact(String),
    Pattern(Pattern),
}

/// One attribute condition of an assertion.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrCheck {
    Fg(Color),
    Bg(Color),
    /// `underline=STYLE`, or `underline=none` for no underline.
    Underline(UnderlineStyle),
    Ulcolor(Color),
    Link(String),
    NoLink,
    /// A flag attribute that must be on (`true`) or off (`!flag`).
    Flag(Attr, bool),
}

/// `assert REGION [text=...] [ATTR ...]`.
#[derive(Debug, Clone)]
pub struct Assert {
    pub line: usize,
    pub raw: String,
    pub region: Region,
    pub text: Option<AssertText>,
    pub attrs: Vec<AttrCheck>,
}

/// One screen block of a golden.
#[derive(Debug, Clone)]
pub struct GoldenScreen {
    /// The 1-based line of its `[screen N]` header.
    pub line: usize,
    pub cols: usize,
    pub rows: usize,
    /// `rows` by `cols`, every cell present (the file omits trailing blanks and default attributes).
    pub cells: Vec<Vec<Cell>>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub cursor_shape: CursorShape,
    pub cursor_hidden: bool,
    /// Sorted, de-duplicated mode tokens.
    pub modes: Vec<String>,
    pub alt_screen: bool,
    pub title: Option<String>,
    /// Sorted, de-duplicated row indices that soft-wrap.
    pub wrapped: Vec<usize>,
    pub name: Option<String>,
    pub ignore: Ignore,
    /// Header comments and `x-` lines, rewritten after the other header lines.
    pub header_comments: Vec<String>,
    pub x_lines: Vec<String>,
    pub masks: Vec<Entry<Mask>>,
    pub asserts: Vec<Entry<Assert>>,
}

/// A parsed golden: one or more screens and what is true of the whole file.
#[derive(Debug, Clone)]
pub struct Golden {
    /// The grammar number of the file (always `GRAMMAR_VERSION` today).
    pub grammar: u32,
    /// The snapshot schema the golden was recorded under.
    pub schema: u32,
    pub semantics: Semantics,
    pub ignore: Ignore,
    pub header_comments: Vec<String>,
    pub x_lines: Vec<String>,
    /// The `[masks]` section before the first screen; applies to every screen.
    pub shared_masks: Vec<Entry<Mask>>,
    pub screens: Vec<GoldenScreen>,
}

impl GoldenScreen {
    /// The text of a row without its trailing blank cells.
    pub fn row_text(&self, row: usize) -> String {
        let cells = &self.cells[row];
        let used = cells.iter().rposition(|c| c.ch != " ").map_or(0, |i| i + 1);
        cells[..used].iter().map(|c| c.ch.as_str()).collect()
    }
}

impl Golden {
    /// Every mask of the shared list, skipping comments.
    pub fn shared_mask_items(&self) -> impl Iterator<Item = &Mask> {
        self.shared_masks.iter().filter_map(|e| match e {
            Entry::Item(m) => Some(m),
            Entry::Comment(_) => None,
        })
    }
}

impl GoldenScreen {
    /// Every mask of this screen, skipping comments.
    pub fn mask_items(&self) -> impl Iterator<Item = &Mask> {
        self.masks.iter().filter_map(|e| match e {
            Entry::Item(m) => Some(m),
            Entry::Comment(_) => None,
        })
    }

    /// Every assertion of this screen, skipping comments.
    pub fn assert_items(&self) -> impl Iterator<Item = &Assert> {
        self.asserts.iter().filter_map(|e| match e {
            Entry::Item(a) => Some(a),
            Entry::Comment(_) => None,
        })
    }
}
