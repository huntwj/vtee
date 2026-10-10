//! The read side of the tap: turn `alacritty_terminal`'s state into plain, inspectable types
//! (requirement 3, full state exposure), stamped with the output-byte offset it reflects
//! (requirement 4: "snapshot as of byte N", so a query never sees a half-drawn frame).

use std::fmt;
use std::str::FromStr;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use vte::ansi::Color as AnsiColor;

use crate::ObserverListener;

/// A cell colour. In JSON it is a string: `"default"`, `"idx:N"` (palette index 0 to 255) or
/// `"rgb:#rrggbb"` (lower-case hex). `idx:1` and `rgb:#cd0000` are different values on purpose:
/// a program that changes how it asks for red is a visible change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Color {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn is_default(&self) -> bool {
        *self == Color::Default
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Color::Default => f.write_str("default"),
            Color::Indexed(n) => write!(f, "idx:{n}"),
            Color::Rgb(r, g, b) => write!(f, "rgb:#{r:02x}{g:02x}{b:02x}"),
        }
    }
}

/// Why a string is not a colour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseColorError(String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} is not a colour (expected \"default\", \"idx:0\" to \"idx:255\" or \"rgb:#rrggbb\")", self.0)
    }
}

impl std::error::Error for ParseColorError {}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bad = || ParseColorError(s.to_string());
        if s == "default" {
            return Ok(Color::Default);
        }
        if let Some(n) = s.strip_prefix("idx:") {
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            return n.parse::<u8>().map(Color::Indexed).map_err(|_| bad());
        }
        if let Some(hex) = s.strip_prefix("rgb:#") {
            if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(bad());
            }
            let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
            return Ok(Color::Rgb(channel(0)?, channel(2)?, channel(4)?));
        }
        Err(bad())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(de::Error::custom)
    }
}

/// `alacritty_terminal` stores a literal TAB in the cell where a tab stop movement started; a
/// terminal shows a blank there.
fn display_char(c: char) -> char {
    if c == '\t' { ' ' } else { c }
}

/// How a cell is underlined. Serialised as a lowercase string.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UnderlineStyle {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

impl UnderlineStyle {
    fn is_none(&self) -> bool {
        *self == UnderlineStyle::None
    }
}

fn underline_of(flags: Flags) -> UnderlineStyle {
    if flags.contains(Flags::UNDERCURL) {
        UnderlineStyle::Curly
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        UnderlineStyle::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        UnderlineStyle::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        UnderlineStyle::Single
    } else {
        UnderlineStyle::None
    }
}

fn convert_color(c: AnsiColor) -> Color {
    match c {
        AnsiColor::Named(n) => match n as usize {
            i @ 0..=15 => Color::Indexed(i as u8),
            // Dim* variants (DimBlack..DimWhite); the dim attribute itself is carried by `Flags`.
            i @ 259..=266 => Color::Indexed((i - 259) as u8),
            // Foreground/Background/Cursor and their bright/dim forms are "the terminal default".
            _ => Color::Default,
        },
        AnsiColor::Indexed(i) => Color::Indexed(i),
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

/// One screen cell. `ch` already includes any combining marks `alacritty_terminal` stores
/// separately via `Cell::zerowidth()` - callers never need to know about that split.
///
/// In JSON a cell is sparse: a field equal to its default is omitted when writing and read as
/// that default when absent, so a blank cell is `{}`. The defaults are `ch` = `" "`, colours
/// `default`, flags `false`, no underline and no hyperlink.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cell {
    #[serde(skip_serializing_if = "is_blank")]
    pub ch: String,
    #[serde(skip_serializing_if = "Color::is_default")]
    pub fg: Color,
    #[serde(skip_serializing_if = "Color::is_default")]
    pub bg: Color,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub italic: bool,
    /// How the cell is underlined; omitted when it is not.
    #[serde(skip_serializing_if = "UnderlineStyle::is_none")]
    pub underline: UnderlineStyle,
    /// SGR 58 underline colour; `Default` when none was set.
    #[serde(skip_serializing_if = "Color::is_default")]
    pub underline_color: Color,
    #[serde(skip_serializing_if = "is_false")]
    pub reverse: bool,
    #[serde(skip_serializing_if = "is_false")]
    pub strike: bool,
    /// SGR 8: the text stays in `ch`, a terminal just does not show it.
    #[serde(skip_serializing_if = "is_false")]
    pub conceal: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hyperlink: Option<String>,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: " ".to_string(),
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            dim: false,
            italic: false,
            underline: UnderlineStyle::None,
            underline_color: Color::Default,
            reverse: false,
            strike: false,
            conceal: false,
            hyperlink: None,
        }
    }
}

fn is_blank(ch: &str) -> bool {
    ch == " "
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
    HollowBlock,
    Hidden,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Cursor {
    pub row: i32,
    pub col: usize,
    pub hidden: bool,
    pub shape: CursorShape,
}

/// Bumped only when an existing field is renamed, removed or changes meaning. Adding a field
/// does not bump it. See `docs/guide/protocol.md`.
pub const SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    1
}

/// The full state of a tap's screen as of `byte_offset` bytes of child output. Two snapshots
/// built from the same byte offset (e.g. one live, one replayed from the raw log) should be
/// identical - that's what makes a golden test out of this stable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// The JSON schema version this snapshot follows (`SCHEMA_VERSION`). Absent in JSON written
    /// before the field existed, which reads as 1.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub byte_offset: u64,
    pub cols: usize,
    pub rows: usize,
    /// Plain-text rows, for convenience when attributes don't matter.
    pub lines: Vec<String>,
    /// Row-major cells, `rows` x `cols`.
    pub cells: Vec<Vec<Cell>>,
    pub cursor: Cursor,
    /// Active DEC private modes, as their conventional string form (e.g. `"?1000"` for mouse
    /// click reporting, `"DECKPAM"` for application keypad) - the same vocabulary used
    /// throughout this project's probes and decisions.
    pub modes: Vec<String>,
    pub alt_screen: bool,
    pub title: Option<String>,
    /// Scrolled-off rows, oldest first, text only (cell-level scrollback detail is not
    /// exposed - a possible future addition if something needs it).
    pub scrollback: Vec<String>,
}

pub(crate) fn build(term: &Term<ObserverListener>, byte_offset: u64, title: Option<String>) -> Snapshot {
    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines();

    let mut lines = Vec::with_capacity(rows);
    let mut cells = Vec::with_capacity(rows);
    for r in 0..rows {
        let line = Line(r as i32);
        let mut text = String::new();
        let mut row_cells = Vec::with_capacity(cols);
        for c in 0..cols {
            let cell = &grid[Point::new(line, Column(c))];
            let mut ch = String::new();
            // The second half of a wide char is padding the first half already covers; keeping
            // it as ' ' would shift every later column in the text and in `redraw_sequence`.
            if !cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                ch.push(display_char(cell.c));
                if let Some(zw) = cell.zerowidth() {
                    ch.extend(zw);
                }
            }
            text.push_str(&ch);
            let f = cell.flags;
            row_cells.push(Cell {
                ch,
                fg: convert_color(cell.fg),
                bg: convert_color(cell.bg),
                bold: f.contains(Flags::BOLD) || f.contains(Flags::DIM_BOLD),
                dim: f.contains(Flags::DIM) || f.contains(Flags::DIM_BOLD),
                italic: f.contains(Flags::ITALIC),
                underline: underline_of(f),
                underline_color: cell.underline_color().map(convert_color).unwrap_or_default(),
                reverse: f.contains(Flags::INVERSE),
                strike: f.contains(Flags::STRIKEOUT),
                conceal: f.contains(Flags::HIDDEN),
                hyperlink: cell.hyperlink().map(|h| h.uri().to_string()),
            });
        }
        lines.push(text);
        cells.push(row_cells);
    }

    let mode = *term.mode();
    let mut modes: Vec<String> = Vec::new();
    if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        modes.push("?1000".to_string());
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        modes.push("?1006".to_string());
    }
    if mode.contains(TermMode::BRACKETED_PASTE) {
        modes.push("?2004".to_string());
    }
    if mode.contains(TermMode::APP_CURSOR) {
        modes.push("?1".to_string());
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        modes.push("DECKPAM".to_string());
    }

    let cursor_point = grid.cursor.point;
    let shape = match term.cursor_style().shape {
        vte::ansi::CursorShape::Block => CursorShape::Block,
        vte::ansi::CursorShape::Underline => CursorShape::Underline,
        vte::ansi::CursorShape::Beam => CursorShape::Bar,
        vte::ansi::CursorShape::HollowBlock => CursorShape::HollowBlock,
        vte::ansi::CursorShape::Hidden => CursorShape::Hidden,
    };

    Snapshot {
        schema_version: SCHEMA_VERSION,
        byte_offset,
        cols,
        rows,
        lines,
        cells,
        cursor: Cursor {
            row: cursor_point.line.0,
            col: cursor_point.column.0,
            hidden: !mode.contains(TermMode::SHOW_CURSOR),
            shape,
        },
        modes,
        alt_screen: mode.contains(TermMode::ALT_SCREEN),
        title,
        scrollback: build_scrollback(term),
    }
}

/// Reconstructs `snap`'s visible screen as raw ANSI bytes for a freshly attached raw-TTY
/// client, sent before the live feed so it sees the current picture at once. `alacritty_terminal`
/// has no API that re-emits ANSI.
///
/// Visual state only (cells, cursor); input-affecting modes such as app-cursor-keys and mouse
/// reporting are not restored. Emits SGR per cell rather than only on change, which is correct
/// but not compact.
pub fn redraw_sequence(snap: &Snapshot) -> Vec<u8> {
    let mut out = Vec::new();
    if snap.alt_screen {
        out.extend_from_slice(b"\x1b[?1049h");
    }
    out.extend_from_slice(b"\x1b[2J");
    for (r, row) in snap.cells.iter().enumerate() {
        out.extend_from_slice(format!("\x1b[{};1H", r + 1).as_bytes());
        for cell in row {
            out.extend_from_slice(b"\x1b[0m");
            let mut codes = Vec::new();
            if cell.bold {
                codes.push("1".to_string());
            }
            if cell.dim {
                codes.push("2".to_string());
            }
            if cell.italic {
                codes.push("3".to_string());
            }
            // Plain `4` for a single underline (understood everywhere); the others use the
            // colon sub-parameter form `4:N` (xterm, kitty, VTE, Windows Terminal). `21` is not
            // used for double: the Linux console reads it as "normal intensity".
            match cell.underline {
                UnderlineStyle::None => {}
                UnderlineStyle::Single => codes.push("4".to_string()),
                UnderlineStyle::Double => codes.push("4:2".to_string()),
                UnderlineStyle::Curly => codes.push("4:3".to_string()),
                UnderlineStyle::Dotted => codes.push("4:4".to_string()),
                UnderlineStyle::Dashed => codes.push("4:5".to_string()),
            }
            push_underline_color(&mut codes, cell.underline_color);
            if cell.reverse {
                codes.push("7".to_string());
            }
            if cell.strike {
                codes.push("9".to_string());
            }
            if cell.conceal {
                codes.push("8".to_string());
            }
            push_color_codes(&mut codes, cell.fg, true);
            push_color_codes(&mut codes, cell.bg, false);
            if !codes.is_empty() {
                out.extend_from_slice(format!("\x1b[{}m", codes.join(";")).as_bytes());
            }
            out.extend_from_slice(cell.ch.as_bytes());
        }
    }
    out.extend_from_slice(format!("\x1b[{};{}H", snap.cursor.row + 1, snap.cursor.col + 1).as_bytes());
    out.extend_from_slice(if snap.cursor.hidden { b"\x1b[?25l" } else { b"\x1b[?25h" });
    out
}

fn push_color_codes(codes: &mut Vec<String>, color: Color, foreground: bool) {
    match color {
        Color::Default => {}
        Color::Indexed(i) => codes.push(format!("{};5;{i}", if foreground { 38 } else { 48 })),
        Color::Rgb(r, g, b) => codes.push(format!("{};2;{r};{g};{b}", if foreground { 38 } else { 48 })),
    }
}

fn push_underline_color(codes: &mut Vec<String>, color: Color) {
    match color {
        Color::Default => {}
        Color::Indexed(i) => codes.push(format!("58;5;{i}")),
        Color::Rgb(r, g, b) => codes.push(format!("58;2;{r};{g};{b}")),
    }
}

fn build_scrollback(term: &Term<ObserverListener>) -> Vec<String> {
    let grid = term.grid();
    let history = grid.history_size();
    if history == 0 {
        return Vec::new();
    }
    let cols = grid.columns();
    (1..=history)
        .rev()
        .map(|i| {
            let line = Line(-(i as i32));
            (0..cols)
                .map(|c| &grid[Point::new(line, Column(c))])
                .filter(|cell| !cell.flags.contains(Flags::WIDE_CHAR_SPACER))
                .map(|cell| display_char(cell.c))
                .collect::<String>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vte::ansi::NamedColor;

    #[test]
    fn colours_are_strings_in_json() {
        for (color, text) in [
            (Color::Default, "default"),
            (Color::Indexed(0), "idx:0"),
            (Color::Indexed(255), "idx:255"),
            (Color::Rgb(0xc8, 0x0a, 0x14), "rgb:#c80a14"),
            (Color::Rgb(0, 0, 0), "rgb:#000000"),
        ] {
            assert_eq!(color.to_string(), text);
            assert_eq!(serde_json::to_value(color).unwrap(), text);
            assert_eq!(serde_json::from_value::<Color>(text.into()).unwrap(), color);
        }
    }

    #[test]
    fn colour_strings_are_parsed_strictly() {
        assert_eq!("rgb:#C80A14".parse::<Color>(), Ok(Color::Rgb(0xc8, 0x0a, 0x14)), "upper-case hex is read");
        for bad in [
            "",
            "Default",
            "idx:",
            "idx:256",
            "idx:-1",
            "idx:+1",
            "idx: 1",
            "rgb:c80a14",
            "rgb:#c80a1",
            "rgb:#c80a14f",
            "red",
        ] {
            assert!(bad.parse::<Color>().is_err(), "{bad:?} should not parse");
        }
        assert!(serde_json::from_str::<Color>("{\"Indexed\":1}").is_err(), "the pre-0.1.0 shape is not read");
        assert!(serde_json::from_str::<Color>("\"Default\"").is_err());
    }

    #[test]
    fn named_colors_map_to_palette_indices() {
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::Red)), Color::Indexed(1));
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::BrightWhite)), Color::Indexed(15));
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::DimRed)), Color::Indexed(1));
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::Foreground)), Color::Default);
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::Background)), Color::Default);
    }
}

#[cfg(test)]
mod emulator_tests {
    use super::*;
    use crate::Size;
    use alacritty_terminal::term::Config;
    use vte::ansi::Processor;

    fn snap(bytes: &[u8], cols: usize, rows: usize) -> Snapshot {
        let listener = ObserverListener::new();
        let mut term = Term::new(Config::default(), &Size { cols, rows }, listener.clone());
        Processor::<vte::ansi::StdSyncHandler>::new().advance(&mut term, bytes);
        build(&term, bytes.len() as u64, listener.current_title())
    }

    fn row_text(s: &Snapshot, row: usize) -> String {
        s.lines[row].trim_end().to_string()
    }

    #[test]
    fn text_attributes_reach_the_cells() {
        let s =
            snap(b"\x1b[1mB\x1b[0m\x1b[2mD\x1b[0m\x1b[3mI\x1b[0m\x1b[4mU\x1b[0m\x1b[7mR\x1b[0m\x1b[9mS\x1b[0mP", 20, 3);
        let c = &s.cells[0];
        assert!(c[0].bold && !c[0].dim && !c[0].italic);
        assert!(c[1].dim && !c[1].bold);
        assert!(c[2].italic);
        assert_eq!(c[3].underline, UnderlineStyle::Single);
        assert!(c[4].reverse);
        assert!(c[5].strike);
        let plain = &c[6];
        assert!(!plain.bold && !plain.dim && !plain.italic && !plain.reverse && !plain.strike);
        assert_eq!(plain.underline, UnderlineStyle::None);
        assert_eq!(row_text(&s, 0), "BDIURSP");
    }

    #[test]
    fn colors_survive_in_all_three_forms() {
        let s = snap(b"\x1b[31mA\x1b[0m\x1b[38;5;200mB\x1b[0m\x1b[48;2;1;2;3mC\x1b[0m\x1b[94mD", 20, 3);
        let c = &s.cells[0];
        assert_eq!(c[0].fg, Color::Indexed(1));
        assert_eq!(c[1].fg, Color::Indexed(200));
        assert_eq!(c[2].bg, Color::Rgb(1, 2, 3));
        assert_eq!(c[3].fg, Color::Indexed(12), "bright blue is palette index 12");
        assert_eq!(c[0].bg, Color::Default);
    }

    #[test]
    fn osc_title_and_hyperlinks_are_reported() {
        let s = snap(b"\x1b]0;my title\x07\x1b]8;;http://example.com\x07link\x1b]8;;\x07 after", 30, 3);
        assert_eq!(s.title.as_deref(), Some("my title"));
        assert_eq!(s.cells[0][0].hyperlink.as_deref(), Some("http://example.com"));
        assert_eq!(s.cells[0][3].hyperlink.as_deref(), Some("http://example.com"));
        assert_eq!(s.cells[0][5].hyperlink, None);
        assert_eq!(snap(b"plain", 10, 2).title, None);
    }

    #[test]
    fn input_modes_and_the_alternate_screen_are_reported_and_cleared() {
        let on = snap(b"\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b=\x1b[?1049h", 20, 4);
        for mode in ["?1000", "?1006", "?2004", "?1", "DECKPAM"] {
            assert!(on.modes.iter().any(|m| m == mode), "{mode} missing from {:?}", on.modes);
        }
        assert!(on.alt_screen);
        let off =
            snap(b"\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b=\x1b[?1049h\x1b[?1049l\x1b[?1000l\x1b[?2004l", 20, 4);
        assert!(!off.alt_screen);
        assert!(!off.modes.iter().any(|m| m == "?1000" || m == "?2004"), "{:?}", off.modes);
        assert!(off.modes.iter().any(|m| m == "?1006"), "?1006 was never turned off");
    }

    #[test]
    fn cursor_position_visibility_and_shape() {
        let s = snap(b"ab\r\ncd\x1b[4 q", 10, 3);
        assert_eq!((s.cursor.row, s.cursor.col), (1, 2));
        assert_eq!(s.cursor.shape, CursorShape::Underline);
        assert!(!s.cursor.hidden);
        assert_eq!(snap(b"\x1b[6 q", 10, 3).cursor.shape, CursorShape::Bar);
        assert_eq!(snap(b"\x1b[2 q", 10, 3).cursor.shape, CursorShape::Block);
        assert!(snap(b"\x1b[?25l", 10, 3).cursor.hidden);
    }

    #[test]
    fn lines_that_scroll_off_are_kept_as_scrollback_oldest_first() {
        let s = snap(b"one\r\ntwo\r\nthree\r\nfour\r\nfive", 10, 3);
        let scrolled: Vec<_> = s.scrollback.iter().map(|l| l.trim_end().to_string()).collect();
        assert_eq!(scrolled, ["one", "two"]);
        assert_eq!(row_text(&s, 0), "three");
        assert_eq!(row_text(&s, 2), "five");
    }

    #[test]
    fn tab_stops_leave_blanks_not_tab_characters() {
        let s = snap(b"a\tb\tc", 20, 2);
        assert_eq!(s.lines[0], "a       b       c   ");
        assert!(s.cells[0].iter().all(|c| !c.ch.contains('\t')));
        assert!(!redraw_sequence(&s).contains(&b'\t'));
    }

    #[test]
    fn tab_stops_leave_blanks_in_scrollback_too() {
        let s = snap(b"a\tb\r\ntwo\r\nthree\r\nfour", 20, 2);
        let first = s.scrollback.first().expect("a row scrolled off");
        assert_eq!(first.trim_end(), "a       b");
    }

    #[test]
    fn wide_and_combining_characters_occupy_the_right_cells() {
        let s = snap("你好X".as_bytes(), 10, 2);
        assert_eq!(row_text(&s, 0), "你好X");
        let c = &s.cells[0];
        assert_eq!((c[0].ch.as_str(), c[1].ch.as_str(), c[2].ch.as_str(), c[3].ch.as_str()), ("你", "", "好", ""));
        assert_eq!(c[4].ch, "X");
        let combined = snap("e\u{301}x".as_bytes(), 10, 2);
        assert_eq!(combined.cells[0][0].ch, "e\u{301}");
        assert_eq!(combined.cells[0][1].ch, "x");
    }

    #[test]
    fn underline_styles_and_colour_reach_the_cells() {
        let styles = [
            (&b"\x1b[4mx"[..], UnderlineStyle::Single),
            // SGR 21 is "bold off" in this emulator, not double underline.
            (b"\x1b[21mx", UnderlineStyle::None),
            (b"\x1b[4:2mx", UnderlineStyle::Double),
            (b"\x1b[4:3mx", UnderlineStyle::Curly),
            (b"\x1b[4:4mx", UnderlineStyle::Dotted),
            (b"\x1b[4:5mx", UnderlineStyle::Dashed),
            (b"\x1b[4:0mx", UnderlineStyle::None),
            (b"\x1b[4m\x1b[24mx", UnderlineStyle::None),
            (b"x", UnderlineStyle::None),
        ];
        for (bytes, want) in styles {
            let c = &snap(bytes, 10, 2).cells[0][0];
            assert_eq!(c.underline, want, "{:?}", String::from_utf8_lossy(bytes));
        }
        let c = &snap(b"\x1b[4:3m\x1b[58;2;1;2;3mx\x1b[58;5;9my\x1b[59mz", 10, 2).cells[0];
        assert_eq!(c[0].underline_color, Color::Rgb(1, 2, 3));
        assert_eq!(c[1].underline_color, Color::Indexed(9));
        assert_eq!(c[2].underline_color, Color::Default);
    }

    #[test]
    fn conceal_is_reported_and_blink_is_not() {
        let s = snap(b"\x1b[8mh\x1b[0mv\x1b[5mb", 10, 2);
        let c = &s.cells[0];
        assert!(c[0].conceal && c[0].ch == "h", "concealed text stays in ch");
        assert!(!c[1].conceal);
        let json = serde_json::to_string(&s.cells).unwrap();
        assert!(!json.contains("blink"), "blink is reserved and never written: {json}");
        assert_eq!(serde_json::to_value(&c[0]).unwrap(), serde_json::json!({"ch": "h", "conceal": true}));
    }

    #[test]
    fn redraw_reproduces_concealed_text() {
        assert_round_trips(b"\x1b[8msecret\x1b[0m shown", 30, 3);
    }

    #[test]
    fn cells_are_sparse_and_defaults_are_never_written() {
        assert_eq!(serde_json::to_string(&Cell::default()).unwrap(), "{}");
        let s = snap(b"\x1b[1;31mR \x1b[0m\x1b[4m\x1b[58;2;1;2;3mU\x1b[0m", 6, 2);
        let cell = |i: usize| serde_json::to_value(&s.cells[0][i]).unwrap();
        assert_eq!(cell(0), serde_json::json!({"ch": "R", "fg": "idx:1", "bold": true}));
        assert_eq!(cell(1), serde_json::json!({"fg": "idx:1", "bold": true}), "a blank cell keeps its attributes only");
        assert_eq!(cell(2), serde_json::json!({"ch": "U", "underline": "single", "underline_color": "rgb:#010203"}));
        assert_eq!(cell(5), serde_json::json!({}));
        assert_eq!(cell(3), serde_json::json!({}));
    }

    #[test]
    fn absent_fields_read_as_defaults_and_explicit_defaults_are_accepted() {
        assert_eq!(serde_json::from_str::<Cell>("{}").unwrap(), Cell::default());
        let dense = r#"{"ch":" ","fg":"default","bg":"default","bold":false,"dim":false,"italic":false,
            "underline":"none","underline_color":"default","reverse":false,"strike":false,"conceal":false,"hyperlink":null}"#;
        assert_eq!(serde_json::from_str::<Cell>(dense).unwrap(), Cell::default());
        let continuation: Cell = serde_json::from_str(r#"{"ch":""}"#).unwrap();
        assert_eq!(continuation.ch, "", "an empty ch is a wide character's continuation cell, not a default");
        assert_eq!(serde_json::to_string(&continuation).unwrap(), r#"{"ch":""}"#);
    }

    #[test]
    fn a_whole_snapshot_round_trips_through_sparse_json() {
        let s = snap(
            "\x1b[1;38;2;1;2;3mbold rgb\x1b[0m \x1b[4:3;48;5;9mcurly\x1b[0m\r\n你好\x1b]8;;http://x.example\x07link\x1b]8;;\x07".as_bytes(),
            30,
            4,
        );
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.cells, s.cells);
        assert_eq!(back.lines, s.lines);
    }

    #[test]
    fn the_schema_version_comes_first_and_older_json_reads_as_version_one() {
        let s = snap(b"x", 5, 2);
        assert_eq!(s.schema_version, SCHEMA_VERSION);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.starts_with("{\"schema_version\":1,\"byte_offset\":"), "{}", &json[..60]);

        let mut v = serde_json::to_value(&s).unwrap();
        v.as_object_mut().unwrap().remove("schema_version");
        let old: Snapshot = serde_json::from_value(v).unwrap();
        assert_eq!(old.schema_version, 1);

        let mut v = serde_json::to_value(&s).unwrap();
        v["schema_version"] = 7.into();
        let newer: Snapshot = serde_json::from_value(v).unwrap();
        assert_eq!(newer.schema_version, 7, "a newer version is kept, not rewritten");
    }

    #[test]
    fn underline_is_one_style_string_and_the_provisional_shapes_are_not_read() {
        for (style, text) in [
            (UnderlineStyle::Single, "single"),
            (UnderlineStyle::Double, "double"),
            (UnderlineStyle::Curly, "curly"),
            (UnderlineStyle::Dotted, "dotted"),
            (UnderlineStyle::Dashed, "dashed"),
        ] {
            assert_eq!(serde_json::to_value(style).unwrap(), text);
            let cell: Cell = serde_json::from_str(&format!(r#"{{"underline":"{text}"}}"#)).unwrap();
            assert_eq!(cell.underline, style);
        }
        assert!(serde_json::from_str::<Cell>(r#"{"underline":true}"#).is_err(), "the old boolean is not read");
        assert!(serde_json::from_str::<Cell>(r#"{"underline":"wavy"}"#).is_err());
        let single = serde_json::to_value(snap(b"\x1b[4mx", 5, 2).cells[0][0].clone()).unwrap();
        assert!(single.get("underline_style").is_none(), "there is no separate underline_style field");
    }

    /// Replaying a snapshot's redraw into a fresh emulator must give back the same screen, which
    /// is what an attaching client's terminal does with it. Hyperlinks are not part of the
    /// redraw, so they are excluded from the comparison.
    fn assert_round_trips(bytes: &[u8], cols: usize, rows: usize) {
        let before = snap(bytes, cols, rows);
        let after = snap(&redraw_sequence(&before), cols, rows);
        assert_eq!(after.lines, before.lines, "text changed");
        assert_eq!(
            (after.cursor.row, after.cursor.col, after.cursor.hidden),
            (before.cursor.row, before.cursor.col, before.cursor.hidden)
        );
        assert_eq!(after.alt_screen, before.alt_screen);
        let strip = |s: &Snapshot| -> Vec<Vec<Cell>> {
            s.cells.iter().map(|r| r.iter().cloned().map(|c| Cell { hyperlink: None, ..c }).collect()).collect()
        };
        assert_eq!(strip(&after), strip(&before), "cell attributes changed");
    }

    #[test]
    fn redraw_reproduces_plain_text_and_the_cursor() {
        assert_round_trips(b"hello\r\nworld\x1b[2;3H", 20, 4);
        assert_round_trips(b"line\x1b[?25l", 20, 4);
    }

    #[test]
    fn redraw_reproduces_colors_and_attributes() {
        assert_round_trips(
            b"\x1b[1;31mred bold\x1b[0m \x1b[4;42munder on green\x1b[0m\r\n\x1b[38;2;10;20;30;48;5;99mrgb\x1b[7m rev",
            40,
            4,
        );
        assert_round_trips(b"\x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m \x1b[9mstrike", 40, 3);
    }

    #[test]
    fn redraw_reproduces_underline_styles_and_colours() {
        assert_round_trips(
            b"\x1b[4mone\x1b[0m \x1b[4:2mtwo\x1b[0m \x1b[4:3mcurly\x1b[0m \x1b[4:4mdot\x1b[0m \x1b[4:5mdash\x1b[0m\r\n\
              \x1b[4:3m\x1b[58;2;200;10;20mrgb\x1b[0m \x1b[4m\x1b[58;5;33midx",
            60,
            4,
        );
    }

    #[test]
    fn redraw_reproduces_wide_characters_and_the_alternate_screen() {
        assert_round_trips("你好 world 世界".as_bytes(), 30, 3);
        assert_round_trips(b"main screen\x1b[?1049h\x1b[2J\x1b[5;5Halt screen text", 30, 8);
    }

    #[test]
    fn redraw_of_a_full_width_screen_does_not_scroll() {
        let wide: Vec<u8> = (0..4).flat_map(|_| b"0123456789".iter().copied()).collect();
        assert_round_trips(&wide, 10, 4);
    }
}
