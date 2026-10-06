// Minimal hand-rolled terminal emulator built on termwiz::escape's Action parser. termwiz itself is
// "encoding and decoding only" (no screen-state emulation; its real app, wezterm, uses an unpublished
// wezterm-term crate for that), so to grade it against the probe we interpret Action ourselves, same as
// the vte fork does for raw Perform callbacks. Enough to grade the 15 tests honestly, not a full emulator.
use serde_json::{json, Value};
use std::panic;
use termwiz::escape::csi::{
    Cursor, Edit, EraseInDisplay, EraseInLine, Mode, Sgr, CSI,
};
use termwiz::escape::osc::OperatingSystemCommand;
use termwiz::escape::parser::Parser;
use termwiz::escape::{Action, ControlCode, Esc, EscCode};
use termwiz::color::ColorSpec;
use unicode_width::UnicodeWidthChar;

const ROWS: usize = 12;
const COLS: usize = 40;

#[derive(Clone)]
struct Cell {
    ch: String,
    fg: Value,
    bg: Value,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    reverse: bool,
    strike: bool,
}

impl Cell {
    fn blank() -> Cell {
        Cell {
            ch: " ".to_string(),
            fg: Value::Null,
            bg: Value::Null,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            blink: false,
            reverse: false,
            strike: false,
        }
    }
    fn to_json(&self) -> Value {
        json!({
            "char": self.ch, "fg": self.fg, "bg": self.bg,
            "bold": self.bold, "dim": self.dim, "italic": self.italic,
            "underline": self.underline, "blink": self.blink,
            "reverse": self.reverse, "strike": self.strike,
        })
    }
}

type Grid = Vec<Vec<Cell>>;

fn blank_row() -> Vec<Cell> {
    (0..COLS).map(|_| Cell::blank()).collect()
}
fn blank_grid() -> Grid {
    (0..ROWS).map(|_| blank_row()).collect()
}

#[derive(Clone, Copy, PartialEq)]
enum Charset {
    Ascii,
    DecLineDrawing,
}

struct Attrs {
    fg: Value,
    bg: Value,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    reverse: bool,
    strike: bool,
}
impl Attrs {
    fn default_attrs() -> Attrs {
        Attrs {
            fg: Value::Null,
            bg: Value::Null,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            blink: false,
            reverse: false,
            strike: false,
        }
    }
}

struct Emu {
    main: Grid,
    alt: Grid,
    alt_screen: bool,
    row: usize,
    col: usize,
    scroll_top: usize,
    scroll_bottom: usize,
    insert_mode: bool,
    cursor_hidden: bool,
    cursor_shape: Option<String>,
    attrs: Attrs,
    g0: Charset,
    modes: std::collections::BTreeSet<String>,
    title: Option<String>,
    link: Option<String>,
    scrollback: Vec<Vec<Cell>>,
}

fn color_to_json(c: &ColorSpec) -> Value {
    match c {
        ColorSpec::Default => Value::Null,
        ColorSpec::PaletteIndex(n) => json!(*n as u64),
        ColorSpec::TrueColor(rgba) => {
            let (r, g, b, _a) = rgba.to_srgb_u8();
            Value::String(format!("{:02x}{:02x}{:02x}", r, g, b))
        }
    }
}

fn dec_line_drawing(c: char) -> char {
    match c {
        '`' => '\u{25C6}',
        'a' => '\u{2592}',
        'b' => '\u{2409}',
        'c' => '\u{240C}',
        'd' => '\u{240D}',
        'e' => '\u{240A}',
        'f' => '\u{00B0}',
        'g' => '\u{00B1}',
        'h' => '\u{2424}',
        'i' => '\u{240B}',
        'j' => '\u{2518}',
        'k' => '\u{2510}',
        'l' => '\u{250C}',
        'm' => '\u{2514}',
        'n' => '\u{253C}',
        'o' => '\u{23BA}',
        'p' => '\u{23BB}',
        'q' => '\u{2500}',
        'r' => '\u{23BC}',
        's' => '\u{23BD}',
        't' => '\u{251C}',
        'u' => '\u{2524}',
        'v' => '\u{2534}',
        'w' => '\u{252C}',
        'x' => '\u{2502}',
        'y' => '\u{2264}',
        'z' => '\u{2265}',
        '{' => '\u{03C0}',
        '|' => '\u{2260}',
        '}' => '\u{00A3}',
        '~' => '\u{00B7}',
        other => other,
    }
}

impl Emu {
    fn new() -> Emu {
        Emu {
            main: blank_grid(),
            alt: blank_grid(),
            alt_screen: false,
            row: 0,
            col: 0,
            scroll_top: 0,
            scroll_bottom: ROWS - 1,
            insert_mode: false,
            cursor_hidden: false,
            cursor_shape: None,
            attrs: Attrs::default_attrs(),
            g0: Charset::Ascii,
            modes: std::collections::BTreeSet::new(),
            title: None,
            link: None,
            scrollback: Vec::new(),
        }
    }

    fn grid_mut(&mut self) -> &mut Grid {
        if self.alt_screen {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    fn cell_from_attrs(&self, ch: &str) -> Cell {
        Cell {
            ch: ch.to_string(),
            fg: self.attrs.fg.clone(),
            bg: self.attrs.bg.clone(),
            bold: self.attrs.bold,
            dim: self.attrs.dim,
            italic: self.attrs.italic,
            underline: self.attrs.underline,
            blink: self.attrs.blink,
            reverse: self.attrs.reverse,
            strike: self.attrs.strike,
        }
    }

    fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        for _ in 0..n {
            if top == 0 && !self.alt_screen {
                let leaving = self.main[0].clone();
                self.scrollback.push(leaving);
            }
            let g = self.grid_mut();
            g.remove(top);
            g.insert(bottom, blank_row());
        }
    }

    fn newline(&mut self) {
        if self.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.row + 1 < ROWS {
            self.row += 1;
        }
    }

    fn put_char(&mut self, c: char) {
        let mapped = if self.g0 == Charset::DecLineDrawing && (c as u32) >= 0x60 && (c as u32) <= 0x7e {
            dec_line_drawing(c)
        } else {
            c
        };
        let w = UnicodeWidthChar::width(mapped).unwrap_or(1);
        if w == 0 {
            // combining mark: fold into the previous cell instead of advancing the cursor
            if self.col > 0 {
                let (r, prev_col) = (self.row, self.col - 1);
                let g = self.grid_mut();
                g[r][prev_col].ch.push(mapped);
            }
            return;
        }
        if self.col >= COLS {
            self.col = 0;
            self.newline();
        }
        let new_cell = self.cell_from_attrs(&mapped.to_string());
        let (r, c0) = (self.row, self.col);
        if self.insert_mode {
            let g = self.grid_mut();
            let row = &mut g[r];
            for _ in 0..w {
                if c0 < row.len() {
                    row.insert(c0, Cell::blank());
                    row.truncate(COLS);
                }
            }
        }
        let blank_tail = Cell {
            ch: String::new(),
            ..self.cell_from_attrs("")
        };
        let g = self.grid_mut();
        g[r][c0] = new_cell;
        if w == 2 && c0 + 1 < COLS {
            g[r][c0 + 1] = blank_tail;
        }
        self.col += w;
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::Print(c) => self.put_char(c),
            Action::PrintString(s) => {
                for c in s.chars() {
                    self.put_char(c);
                }
            }
            Action::Control(code) => match code {
                ControlCode::LineFeed => self.newline(),
                ControlCode::CarriageReturn => self.col = 0,
                ControlCode::Backspace => {
                    if self.col > 0 {
                        self.col -= 1;
                    }
                }
                ControlCode::HorizontalTab => {
                    let next = ((self.col / 8) + 1) * 8;
                    self.col = next.min(COLS - 1);
                }
                _ => {}
            },
            Action::Esc(esc) => {
                if let Esc::Code(code) = esc {
                    match code {
                        EscCode::DecApplicationKeyPad => {
                            self.modes.insert("DECKPAM".to_string());
                        }
                        EscCode::DecNormalKeyPad => {
                            self.modes.remove("DECKPAM");
                        }
                        EscCode::DecLineDrawingG0 => self.g0 = Charset::DecLineDrawing,
                        EscCode::AsciiCharacterSetG0 => self.g0 = Charset::Ascii,
                        _ => {}
                    }
                }
            }
            Action::CSI(csi) => self.apply_csi(csi),
            Action::OperatingSystemCommand(osc) => match *osc {
                OperatingSystemCommand::SetWindowTitle(t) => self.title = Some(t),
                OperatingSystemCommand::SetIconNameAndWindowTitle(t) => self.title = Some(t),
                OperatingSystemCommand::SetHyperlink(Some(link)) => {
                    self.link = Some(link.uri().to_string())
                }
                OperatingSystemCommand::SetHyperlink(None) => {}
                _ => {}
            },
            _ => {}
        }
    }

    fn enter_alt(&mut self, clear: bool) {
        if !self.alt_screen {
            self.alt_screen = true;
            if clear {
                self.alt = blank_grid();
            }
            self.row = 0;
            self.col = 0;
        }
    }
    fn leave_alt(&mut self) {
        if self.alt_screen {
            self.alt_screen = false;
            self.row = 0;
            self.col = 0;
        }
    }

    fn apply_csi(&mut self, csi: CSI) {
        match csi {
            CSI::Cursor(c) => match c {
                Cursor::Up(n) => self.row = self.row.saturating_sub(n as usize),
                Cursor::Down(n) => self.row = (self.row + n as usize).min(ROWS - 1),
                Cursor::Left(n) => self.col = self.col.saturating_sub(n as usize),
                Cursor::Right(n) => self.col = (self.col + n as usize).min(COLS - 1),
                Cursor::Position { line, col } => {
                    self.row = (line.as_zero_based() as usize).min(ROWS - 1);
                    self.col = (col.as_zero_based() as usize).min(COLS - 1);
                }
                Cursor::SetTopAndBottomMargins { top, bottom } => {
                    self.scroll_top = (top.as_zero_based() as usize).min(ROWS - 1);
                    self.scroll_bottom = (bottom.as_zero_based() as usize).min(ROWS - 1);
                    if self.scroll_top > self.scroll_bottom {
                        self.scroll_top = 0;
                        self.scroll_bottom = ROWS - 1;
                    }
                }
                Cursor::CursorStyle(shape) => {
                    self.cursor_shape = Some(format!("{:?}", shape));
                }
                _ => {}
            },
            CSI::Edit(e) => match e {
                Edit::InsertCharacter(n) => {
                    let (r, c0) = (self.row, self.col);
                    let g = self.grid_mut();
                    let row = &mut g[r];
                    for _ in 0..n {
                        if c0 <= row.len() {
                            row.insert(c0, Cell::blank());
                            row.truncate(COLS);
                        }
                    }
                }
                Edit::DeleteCharacter(n) => {
                    let (r, c0) = (self.row, self.col);
                    let g = self.grid_mut();
                    let row = &mut g[r];
                    for _ in 0..n {
                        if c0 < row.len() {
                            row.remove(c0);
                            row.push(Cell::blank());
                        }
                    }
                }
                Edit::InsertLine(n) => {
                    let (row, bottom) = (self.row, self.scroll_bottom);
                    if row <= bottom {
                        let g = self.grid_mut();
                        for _ in 0..n {
                            g.remove(bottom);
                            g.insert(row, blank_row());
                        }
                    }
                }
                Edit::DeleteLine(n) => {
                    let (row, bottom) = (self.row, self.scroll_bottom);
                    if row <= bottom {
                        let g = self.grid_mut();
                        for _ in 0..n {
                            g.remove(row);
                            g.insert(bottom, blank_row());
                        }
                    }
                }
                Edit::EraseInLine(k) => {
                    let (r, c0) = (self.row, self.col);
                    let g = self.grid_mut();
                    let row = &mut g[r];
                    match k {
                        EraseInLine::EraseToEndOfLine => {
                            for c in c0..COLS {
                                row[c] = Cell::blank();
                            }
                        }
                        EraseInLine::EraseToStartOfLine => {
                            for c in 0..=c0.min(COLS - 1) {
                                row[c] = Cell::blank();
                            }
                        }
                        EraseInLine::EraseLine => {
                            for c in row.iter_mut() {
                                *c = Cell::blank();
                            }
                        }
                    }
                }
                Edit::EraseInDisplay(k) => match k {
                    EraseInDisplay::EraseDisplay => *self.grid_mut() = blank_grid(),
                    _ => {}
                },
                Edit::ScrollUp(n) => self.scroll_up(n as usize),
                Edit::ScrollDown(n) => {
                    for _ in 0..n {
                        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
                        let g = self.grid_mut();
                        g.remove(bottom);
                        g.insert(top, blank_row());
                    }
                }
                _ => {}
            },
            CSI::Sgr(sgr) => self.apply_sgr(sgr),
            CSI::Mode(m) => self.apply_mode(m),
            _ => {}
        }
    }

    fn apply_sgr(&mut self, sgr: Sgr) {
        use termwiz::cell::{Blink, Intensity, Underline};
        match sgr {
            Sgr::Reset => self.attrs = Attrs::default_attrs(),
            Sgr::Intensity(i) => match i {
                Intensity::Normal => {
                    self.attrs.bold = false;
                    self.attrs.dim = false;
                }
                Intensity::Bold => {
                    self.attrs.bold = true;
                    self.attrs.dim = false;
                }
                Intensity::Half => {
                    self.attrs.dim = true;
                    self.attrs.bold = false;
                }
            },
            Sgr::Italic(b) => self.attrs.italic = b,
            Sgr::Underline(u) => self.attrs.underline = !matches!(u, Underline::None),
            Sgr::Blink(b) => self.attrs.blink = !matches!(b, Blink::None),
            Sgr::Inverse(b) => self.attrs.reverse = b,
            Sgr::StrikeThrough(b) => self.attrs.strike = b,
            Sgr::Foreground(c) => self.attrs.fg = color_to_json(&c),
            Sgr::Background(c) => self.attrs.bg = color_to_json(&c),
            _ => {}
        }
    }

    fn apply_mode(&mut self, m: Mode) {
        use termwiz::escape::csi::{DecPrivateMode, DecPrivateModeCode, TerminalMode, TerminalModeCode};
        let dec_token = |code: &DecPrivateModeCode| -> Option<&'static str> {
            match code {
                DecPrivateModeCode::ApplicationCursorKeys => Some("?1"),
                DecPrivateModeCode::MouseTracking => Some("?1000"),
                DecPrivateModeCode::SGRMouse => Some("?1006"),
                DecPrivateModeCode::BracketedPaste => Some("?2004"),
                _ => None,
            }
        };
        match m {
            Mode::SetDecPrivateMode(DecPrivateMode::Code(code)) => {
                match code {
                    DecPrivateModeCode::ShowCursor => self.cursor_hidden = false,
                    DecPrivateModeCode::EnableAlternateScreen => self.enter_alt(false),
                    DecPrivateModeCode::OptEnableAlternateScreen => self.enter_alt(false),
                    DecPrivateModeCode::ClearAndEnableAlternateScreen => self.enter_alt(true),
                    _ => {}
                }
                if let Some(tok) = dec_token(&code) {
                    self.modes.insert(tok.to_string());
                }
            }
            Mode::ResetDecPrivateMode(DecPrivateMode::Code(code)) => {
                match code {
                    DecPrivateModeCode::ShowCursor => self.cursor_hidden = true,
                    DecPrivateModeCode::EnableAlternateScreen => self.leave_alt(),
                    DecPrivateModeCode::OptEnableAlternateScreen => self.leave_alt(),
                    DecPrivateModeCode::ClearAndEnableAlternateScreen => self.leave_alt(),
                    _ => {}
                }
                if let Some(tok) = dec_token(&code) {
                    self.modes.remove(tok);
                }
            }
            Mode::SetMode(TerminalMode::Code(TerminalModeCode::Insert)) => self.insert_mode = true,
            Mode::ResetMode(TerminalMode::Code(TerminalModeCode::Insert)) => self.insert_mode = false,
            _ => {}
        }
    }

    fn lines(&self) -> Vec<String> {
        let g = if self.alt_screen { &self.alt } else { &self.main };
        g.iter()
            .map(|row| row.iter().map(|c| c.ch.as_str()).collect::<String>())
            .collect()
    }

    fn cell_grid_json(&self) -> Value {
        let g = if self.alt_screen { &self.alt } else { &self.main };
        Value::Array(
            g.iter()
                .map(|row| Value::Array(row.iter().map(|c| c.to_json()).collect()))
                .collect(),
        )
    }

    fn snapshot(&self, test: &str) -> Value {
        let scrollback: Option<Vec<String>> = if self.scrollback.is_empty() {
            None
        } else {
            Some(
                self.scrollback
                    .iter()
                    .map(|row| row.iter().map(|c| c.ch.as_str()).collect::<String>())
                    .collect(),
            )
        };
        json!({
            "test": test,
            "lines": self.lines(),
            "cell": self.cell_grid_json(),
            "cursor": {
                "row": self.row, "col": self.col,
                "hidden": self.cursor_hidden,
                "shape": self.cursor_shape,
            },
            "title": self.title,
            "modes": Vec::from_iter(self.modes.iter().cloned()),
            "links": self.link,
            "scrollback": scrollback,
        })
    }
}

fn run_test(name: &str) -> Option<Value> {
    let path = format!("../../{}.bin", name);
    let data = std::fs::read(&path).ok()?;
    let result = panic::catch_unwind(|| {
        let mut emu = Emu::new();
        let mut parser = Parser::new();
        let actions = parser.parse_as_vec(&data);
        for a in actions {
            emu.apply(a);
        }
        emu.snapshot(name)
    });
    result.ok()
}

fn main() {
    panic::set_hook(Box::new(|_| {}));
    let tests = [
        "T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13",
        "T14",
    ];
    for t in tests {
        if let Some(snap) = run_test(t) {
            println!("{}", snap);
        } else {
            eprintln!("probe: {} panicked or file missing, omitting", t);
        }
    }
}
