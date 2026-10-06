use std::fs;
use std::panic;

use serde_json::{json, Value};
use vte::{Params, Parser, Perform};

const COLS: usize = 40;
const ROWS: usize = 12;

#[derive(Clone)]
struct Cell {
    ch: char,
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    blink: bool,
    reverse: bool,
    strike: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: None,
            bg: None,
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

#[derive(Clone, Copy)]
enum Color {
    Idx(u16),
    Rgb(u8, u8, u8),
}

impl Color {
    fn to_json(&self) -> Value {
        match self {
            Color::Idx(n) => json!(n),
            Color::Rgb(r, g, b) => json!(format!("{:02x}{:02x}{:02x}", r, g, b)),
        }
    }
}

#[derive(Clone)]
struct Grid {
    cells: Vec<Vec<Cell>>,
    top: usize,    // scroll region top (0-indexed, inclusive)
    bottom: usize, // scroll region bottom (0-indexed, inclusive)
}

impl Grid {
    fn new() -> Self {
        Grid {
            cells: (0..ROWS).map(|_| vec![Cell::default(); COLS]).collect(),
            top: 0,
            bottom: ROWS - 1,
        }
    }

    fn clear(&mut self) {
        self.cells = (0..ROWS).map(|_| vec![Cell::default(); COLS]).collect();
        self.top = 0;
        self.bottom = ROWS - 1;
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            self.cells.remove(self.top);
            self.cells.insert(self.bottom, vec![Cell::default(); COLS]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        for _ in 0..n {
            self.cells.remove(self.bottom);
            self.cells.insert(self.top, vec![Cell::default(); COLS]);
        }
    }

    fn insert_lines(&mut self, row: usize, n: usize) {
        if row < self.top || row > self.bottom {
            return;
        }
        for _ in 0..n {
            if self.bottom < self.cells.len() {
                self.cells.remove(self.bottom);
            }
            self.cells.insert(row, vec![Cell::default(); COLS]);
        }
    }

    fn delete_lines(&mut self, row: usize, n: usize) {
        if row < self.top || row > self.bottom {
            return;
        }
        for _ in 0..n {
            if row < self.cells.len() {
                self.cells.remove(row);
            }
            self.cells.insert(self.bottom, vec![Cell::default(); COLS]);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Charset {
    Ascii,
    Graphics,
}

struct State {
    main: Grid,
    alt: Grid,
    in_alt: bool,
    saved_cursor: (usize, usize),
    row: usize,
    col: usize,
    hidden: bool,
    cursor_shape: Option<String>,
    irm: bool,
    g0: Charset,
    pending_attr: Cell,
    title: Option<String>,
    link: Option<String>,
    modes: Vec<String>,
}

impl State {
    fn new() -> Self {
        State {
            main: Grid::new(),
            alt: Grid::new(),
            in_alt: false,
            saved_cursor: (0, 0),
            row: 0,
            col: 0,
            hidden: false,
            cursor_shape: None,
            irm: false,
            g0: Charset::Ascii,
            pending_attr: Cell::default(),
            title: None,
            link: None,
            modes: Vec::new(),
        }
    }

    fn grid(&mut self) -> &mut Grid {
        if self.in_alt {
            &mut self.alt
        } else {
            &mut self.main
        }
    }

    fn clamp_cursor(&mut self) {
        if self.row >= ROWS {
            self.row = ROWS - 1;
        }
        if self.col >= COLS {
            self.col = COLS - 1;
        }
    }

    fn newline(&mut self) {
        let (top, bottom) = {
            let g = self.grid();
            (g.top, g.bottom)
        };
        if self.row == bottom {
            self.grid().scroll_up(1);
        } else if self.row < bottom {
            self.row += 1;
        }
        let _ = top;
    }

    fn reverse_index(&mut self) {
        let (top, bottom) = {
            let g = self.grid();
            (g.top, g.bottom)
        };
        if self.row == top {
            self.grid().scroll_down(1);
        } else if self.row > top {
            self.row -= 1;
        }
        let _ = bottom;
    }
}

fn graphics_translate(c: char) -> char {
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

impl Perform for State {
    fn print(&mut self, c: char) {
        let ch = if self.g0 == Charset::Graphics {
            graphics_translate(c)
        } else {
            c
        };
        if self.col >= COLS {
            self.col = 0;
            self.newline();
        }
        let attr = self.pending_attr.clone();
        let (row, col) = (self.row, self.col);
        let irm = self.irm;
        let g = self.grid();
        if irm && col < COLS - 1 {
            for i in (col + 1..COLS).rev() {
                g.cells[row][i] = g.cells[row][i - 1].clone();
            }
        }
        g.cells[row][col] = Cell { ch, ..attr };
        self.col += 1;
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x08 => {
                if self.col > 0 {
                    self.col -= 1;
                }
            }
            0x09 => {
                let next = ((self.col / 8) + 1) * 8;
                self.col = next.min(COLS - 1);
            }
            0x0A => self.newline(),
            0x0D => self.col = 0,
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        match params[0] {
            b"0" | b"1" | b"2" => {
                if params.len() > 1 {
                    self.title = Some(String::from_utf8_lossy(params[1]).to_string());
                }
            }
            b"8" => {
                if params.len() > 2 && !params[2].is_empty() {
                    self.link = Some(String::from_utf8_lossy(params[2]).to_string());
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        let flat: Vec<u16> = params.iter().flat_map(|g| g.iter().copied()).collect();
        let private = intermediates.first() == Some(&b'?');
        let get = |i: usize, default: u16| -> u16 {
            flat.get(i).copied().filter(|&v| v != 0 || default == 0).unwrap_or(default)
        };

        match action {
            'H' | 'f' => {
                self.row = (get(0, 1).max(1) as usize) - 1;
                self.col = (get(1, 1).max(1) as usize) - 1;
                self.clamp_cursor();
            }
            'A' => {
                let n = get(0, 1).max(1) as usize;
                self.row = self.row.saturating_sub(n);
            }
            'B' => {
                let n = get(0, 1).max(1) as usize;
                self.row = (self.row + n).min(ROWS - 1);
            }
            'C' => {
                let n = get(0, 1).max(1) as usize;
                self.col = (self.col + n).min(COLS - 1);
            }
            'D' => {
                let n = get(0, 1).max(1) as usize;
                self.col = self.col.saturating_sub(n);
            }
            'G' => {
                self.col = (get(0, 1).max(1) as usize - 1).min(COLS - 1);
            }
            'd' => {
                self.row = (get(0, 1).max(1) as usize - 1).min(ROWS - 1);
            }
            'J' => {
                let mode = get(0, 0);
                let (row, col) = (self.row, self.col);
                let g = self.grid();
                match mode {
                    0 => {
                        for c in col..COLS {
                            g.cells[row][c] = Cell::default();
                        }
                        for r in (row + 1)..ROWS {
                            g.cells[r] = vec![Cell::default(); COLS];
                        }
                    }
                    1 => {
                        for c in 0..=col.min(COLS - 1) {
                            g.cells[row][c] = Cell::default();
                        }
                        for r in 0..row {
                            g.cells[r] = vec![Cell::default(); COLS];
                        }
                    }
                    _ => g.clear(),
                }
            }
            'K' => {
                let mode = get(0, 0);
                let (row, col) = (self.row, self.col);
                let g = self.grid();
                match mode {
                    0 => {
                        for c in col..COLS {
                            g.cells[row][c] = Cell::default();
                        }
                    }
                    1 => {
                        for c in 0..=col.min(COLS - 1) {
                            g.cells[row][c] = Cell::default();
                        }
                    }
                    _ => g.cells[row] = vec![Cell::default(); COLS],
                }
            }
            'L' => {
                let n = get(0, 1).max(1) as usize;
                let row = self.row;
                self.grid().insert_lines(row, n);
            }
            'M' => {
                let n = get(0, 1).max(1) as usize;
                let row = self.row;
                self.grid().delete_lines(row, n);
            }
            '@' => {
                let n = get(0, 1).max(1) as usize;
                let (row, col) = (self.row, self.col);
                let g = self.grid();
                for _ in 0..n {
                    if col < COLS {
                        g.cells[row].insert(col, Cell::default());
                        g.cells[row].pop();
                    }
                }
            }
            'P' => {
                let n = get(0, 1).max(1) as usize;
                let (row, col) = (self.row, self.col);
                let g = self.grid();
                for _ in 0..n {
                    if col < g.cells[row].len() {
                        g.cells[row].remove(col);
                        g.cells[row].push(Cell::default());
                    }
                }
            }
            'r' => {
                if flat.is_empty() {
                    let g = self.grid();
                    g.top = 0;
                    g.bottom = ROWS - 1;
                } else {
                    let top = get(0, 1).max(1) as usize - 1;
                    let bottom = (get(1, ROWS as u16).max(1) as usize - 1).min(ROWS - 1);
                    let g = self.grid();
                    if top < bottom {
                        g.top = top;
                        g.bottom = bottom;
                    }
                }
            }
            'm' => {
                let mut i = 0;
                if flat.is_empty() {
                    self.pending_attr = Cell::default();
                }
                while i < flat.len() {
                    let code = flat[i];
                    match code {
                        0 => self.pending_attr = Cell::default(),
                        1 => self.pending_attr.bold = true,
                        2 => self.pending_attr.dim = true,
                        3 => self.pending_attr.italic = true,
                        4 => self.pending_attr.underline = true,
                        5 => self.pending_attr.blink = true,
                        7 => self.pending_attr.reverse = true,
                        9 => self.pending_attr.strike = true,
                        22 => self.pending_attr.bold = false,
                        23 => self.pending_attr.italic = false,
                        24 => self.pending_attr.underline = false,
                        25 => self.pending_attr.blink = false,
                        27 => self.pending_attr.reverse = false,
                        29 => self.pending_attr.strike = false,
                        30..=37 => self.pending_attr.fg = Some(Color::Idx(code - 30)),
                        39 => self.pending_attr.fg = None,
                        40..=47 => self.pending_attr.bg = Some(Color::Idx(code - 40)),
                        49 => self.pending_attr.bg = None,
                        90..=97 => self.pending_attr.fg = Some(Color::Idx(code - 90 + 8)),
                        100..=107 => self.pending_attr.bg = Some(Color::Idx(code - 100 + 8)),
                        38 | 48 => {
                            let is_fg = code == 38;
                            if let Some(&kind) = flat.get(i + 1) {
                                if kind == 5 {
                                    if let Some(&idx) = flat.get(i + 2) {
                                        let col = Color::Idx(idx);
                                        if is_fg {
                                            self.pending_attr.fg = Some(col);
                                        } else {
                                            self.pending_attr.bg = Some(col);
                                        }
                                        i += 2;
                                    }
                                } else if kind == 2 {
                                    if let (Some(&r), Some(&g_), Some(&b)) =
                                        (flat.get(i + 2), flat.get(i + 3), flat.get(i + 4))
                                    {
                                        let col = Color::Rgb(r as u8, g_ as u8, b as u8);
                                        if is_fg {
                                            self.pending_attr.fg = Some(col);
                                        } else {
                                            self.pending_attr.bg = Some(col);
                                        }
                                        i += 4;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            'h' | 'l' => {
                let set = action == 'h';
                for &p in &flat {
                    if private {
                        let name = match p {
                            1 => Some("?1"),
                            47 | 1047 | 1049 => {
                                if set && !self.in_alt {
                                    self.saved_cursor = (self.row, self.col);
                                    self.in_alt = true;
                                    self.alt.clear();
                                } else if !set && self.in_alt {
                                    self.in_alt = false;
                                    (self.row, self.col) = self.saved_cursor;
                                }
                                None
                            }
                            25 => {
                                self.hidden = !set;
                                None
                            }
                            1000 => Some("?1000"),
                            1006 => Some("?1006"),
                            2004 => Some("?2004"),
                            _ => None,
                        };
                        if let Some(n) = name {
                            self.modes.retain(|m| m != n);
                            if set {
                                self.modes.push(n.to_string());
                            }
                        }
                    } else if p == 4 {
                        self.irm = set;
                    }
                }
            }
            'q' => {
                // DECSCUSR: CSI Ps SP q
                if intermediates.first() == Some(&b' ') {
                    let shape = match get(0, 1) {
                        0 | 1 | 2 => "block",
                        3 | 4 => "underline",
                        5 | 6 => "bar",
                        _ => "block",
                    };
                    self.cursor_shape = Some(shape.to_string());
                }
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates.first(), byte) {
            (Some(b'('), b'0') => self.g0 = Charset::Graphics,
            (Some(b'('), b'B') => self.g0 = Charset::Ascii,
            (None, b'=') => {
                self.modes.retain(|m| m != "DECKPAM");
                self.modes.push("DECKPAM".to_string());
            }
            (None, b'>') => self.modes.retain(|m| m != "DECKPAM"),
            (None, b'D') => self.newline(),
            (None, b'M') => self.reverse_index(),
            _ => {}
        }
    }
}

fn cell_json(c: &Cell) -> Value {
    json!({
        "char": c.ch.to_string(),
        "fg": c.fg.map(|x| x.to_json()),
        "bg": c.bg.map(|x| x.to_json()),
        "bold": c.bold, "dim": c.dim, "italic": c.italic, "underline": c.underline,
        "blink": c.blink, "reverse": c.reverse, "strike": c.strike,
    })
}

fn run_test(name: &str) -> Option<Value> {
    let path = format!("../../{}.bin", name);
    let data = fs::read(&path).ok()?;
    let result = panic::catch_unwind(|| {
        let mut parser = Parser::new();
        let mut state = State::new();
        parser.advance(&mut state, &data);
        let lines: Vec<String> = state
            .main
            .cells
            .iter()
            .map(|r| r.iter().map(|c| c.ch).collect::<String>())
            .collect();
        let active_lines: Vec<String> = state
            .grid()
            .cells
            .iter()
            .map(|r| r.iter().map(|c| c.ch).collect::<String>())
            .collect();
        let cell_grid: Vec<Vec<Value>> = {
            let g = state.grid();
            g.cells
                .iter()
                .map(|r| r.iter().map(cell_json).collect())
                .collect()
        };
        let use_lines = if state.in_alt { active_lines } else { lines };
        json!({
            "test": name,
            "lines": use_lines,
            "cell": cell_grid,
            "cursor": {"row": state.row, "col": state.col, "hidden": state.hidden, "shape": state.cursor_shape},
            "title": state.title,
            "modes": state.modes,
            "links": state.link,
            "scrollback": null,
        })
    });
    result.ok()
}

fn main() {
    let tests = [
        "T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13",
        "T14",
    ];
    for t in tests {
        if let Some(v) = run_test(t) {
            println!("{}", v);
        }
    }
}
