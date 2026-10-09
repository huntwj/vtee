//! The read side of the tap: turn `alacritty_terminal`'s state into plain, inspectable types
//! (requirement 3, full state exposure), stamped with the output-byte offset it reflects
//! (requirement 4: "snapshot as of byte N", so a query never sees a half-drawn frame).

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Term, TermMode};
use serde::{Deserialize, Serialize};
use vte::ansi::Color as AnsiColor;

use crate::ObserverListener;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Color {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cell {
    pub ch: String,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    /// Always false: `alacritty_terminal` has no blink flag at all (a known, accepted gap -
    /// see docs/decisions.md).
    pub blink: bool,
    pub reverse: bool,
    pub strike: bool,
    pub hyperlink: Option<String>,
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

/// The full state of a tap's screen as of `byte_offset` bytes of child output. Two snapshots
/// built from the same byte offset (e.g. one live, one replayed from the raw log) should be
/// identical - that's what makes a golden test out of this stable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
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
                ch.push(cell.c);
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
                underline: f.intersects(Flags::ALL_UNDERLINES),
                blink: false,
                reverse: f.contains(Flags::INVERSE),
                strike: f.contains(Flags::STRIKEOUT),
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

/// Reconstructs `snap`'s visible screen as raw ANSI bytes, for a freshly-attached raw-TTY client
/// (`Tap::serve_tty`, `docs/architecture.md`) - sent before the live byte feed starts, so
/// attaching mid-session shows the current picture immediately, the way `tmux attach` does,
/// rather than a blank screen waiting for new output. No existing `alacritty_terminal` API does
/// this (checked its 0.26.0 source directly, not assumed) - it only ever consumes ANSI, never
/// re-emits it.
///
/// Scope, deliberately: visual state only (cells + cursor position/visibility). Emits SGR
/// unconditionally per cell rather than only on an attribute change - correct, not the most
/// compact possible output, and not worth optimizing until a real session makes it matter.
/// Does *not* restore input-affecting modes (app-cursor-keys, mouse reporting) for the new
/// client's own keystrokes - a known, smaller follow-on, not a correctness gap in what's drawn.
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
            if cell.underline {
                codes.push("4".to_string());
            }
            if cell.reverse {
                codes.push("7".to_string());
            }
            if cell.strike {
                codes.push("9".to_string());
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
                .map(|cell| cell.c)
                .collect::<String>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use vte::ansi::NamedColor;

    #[test]
    fn named_colors_map_to_palette_indices() {
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::Red)), Color::Indexed(1));
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::BrightWhite)), Color::Indexed(15));
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::DimRed)), Color::Indexed(1));
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::Foreground)), Color::Default);
        assert_eq!(convert_color(AnsiColor::Named(NamedColor::Background)), Color::Default);
    }
}
