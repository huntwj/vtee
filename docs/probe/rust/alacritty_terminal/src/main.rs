use std::cell::RefCell;
use std::fs;
use std::panic;
use std::rc::Rc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use vte::ansi::{Color, CursorShape, Processor};

const COLS: usize = 40;
const ROWS: usize = 12;

const TESTS: &[&str] = &[
    "T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13", "T14",
];

#[derive(Clone)]
struct CapturingListener {
    title: Rc<RefCell<Option<String>>>,
}

impl EventListener for CapturingListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(t) => *self.title.borrow_mut() = Some(t),
            Event::ResetTitle => *self.title.borrow_mut() = None,
            _ => {}
        }
    }
}

struct Size {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

fn color_json(c: Color) -> serde_json::Value {
    match c {
        Color::Named(_) => serde_json::Value::Null,
        Color::Indexed(i) => serde_json::json!(i),
        Color::Spec(rgb) => serde_json::json!(format!("{:02x}{:02x}{:02x}", rgb.r, rgb.g, rgb.b)),
    }
}

fn cell_text(cell: &Cell) -> String {
    let mut s = String::new();
    s.push(cell.c);
    if let Some(zw) = cell.zerowidth() {
        for ch in zw {
            s.push(*ch);
        }
    }
    s
}

fn cell_json(cell: &Cell) -> serde_json::Value {
    let f = cell.flags;
    serde_json::json!({
        "char": cell_text(cell),
        "fg": color_json(cell.fg),
        "bg": color_json(cell.bg),
        "bold": f.contains(Flags::BOLD) || f.contains(Flags::DIM_BOLD),
        "dim": f.contains(Flags::DIM) || f.contains(Flags::DIM_BOLD),
        "italic": f.contains(Flags::ITALIC),
        "underline": f.intersects(Flags::ALL_UNDERLINES),
        "blink": false,
        "reverse": f.contains(Flags::INVERSE),
        "strike": f.contains(Flags::STRIKEOUT),
    })
}

fn cursor_shape_name(shape: CursorShape) -> Option<&'static str> {
    match shape {
        CursorShape::Block => Some("block"),
        CursorShape::Underline => Some("underline"),
        CursorShape::Beam => Some("bar"),
        CursorShape::HollowBlock => Some("hollow_block"),
        CursorShape::Hidden => Some("hidden"),
    }
}

fn snapshot(term: &Term<CapturingListener>, title: &Rc<RefCell<Option<String>>>) -> serde_json::Value {
    let grid = term.grid();
    let mode = *term.mode();

    let mut lines = Vec::with_capacity(ROWS);
    let mut cell_grid = Vec::with_capacity(ROWS);
    let mut links_found: Option<String> = None;
    for r in 0..ROWS {
        let mut line = String::new();
        let mut row_cells = Vec::with_capacity(COLS);
        for c in 0..COLS {
            let point = Point::new(Line(r as i32), Column(c));
            let cell = &grid[point];
            line.push_str(&cell_text(cell));
            if links_found.is_none() {
                if let Some(h) = cell.hyperlink() {
                    links_found = Some(h.uri().to_string());
                }
            }
            row_cells.push(cell_json(cell));
        }
        lines.push(line);
        cell_grid.push(row_cells);
    }

    let cursor_point = grid.cursor.point;
    let cursor = serde_json::json!({
        "row": cursor_point.line.0,
        "col": cursor_point.column.0,
        "hidden": !mode.contains(TermMode::SHOW_CURSOR),
        "shape": cursor_shape_name(term.cursor_style().shape),
    });

    let mut modes: Vec<&'static str> = Vec::new();
    if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        modes.push("?1000");
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        modes.push("?1006");
    }
    if mode.contains(TermMode::BRACKETED_PASTE) {
        modes.push("?2004");
    }
    if mode.contains(TermMode::APP_CURSOR) {
        modes.push("?1");
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        modes.push("DECKPAM");
    }

    let history = grid.history_size();
    let scrollback = if history > 0 {
        let mut sb = Vec::with_capacity(history);
        for i in (1..=history).rev() {
            let line_idx = -(i as i32);
            let mut s = String::new();
            for c in 0..COLS {
                let point = Point::new(Line(line_idx), Column(c));
                s.push(grid[point].c);
            }
            sb.push(s);
        }
        Some(sb)
    } else {
        None
    };

    serde_json::json!({
        "lines": lines,
        "cell": cell_grid,
        "cursor": cursor,
        "title": *title.borrow(),
        "modes": modes,
        "links": links_found,
        "scrollback": scrollback,
    })
}

fn run_test(name: &str) -> Option<serde_json::Value> {
    let path = format!("../../{}.bin", name);
    let data = fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {}", path, e));

    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let size = Size { columns: COLS, screen_lines: ROWS };
        let title = Rc::new(RefCell::new(None));
        let listener = CapturingListener { title: title.clone() };
        let mut term = Term::new(Config::default(), &size, listener);
        let mut parser = Processor::<vte::ansi::StdSyncHandler>::new();
        parser.advance(&mut term, &data);
        snapshot(&term, &title)
    }));

    match result {
        Ok(v) => Some(v),
        Err(e) => {
            eprintln!("{} panicked: {:?}", name, e);
            None
        }
    }
}

fn main() {
    for name in TESTS {
        if let Some(mut snap) = run_test(name) {
            snap["test"] = serde_json::json!(name);
            println!("{}", snap);
        } else {
            eprintln!("{} produced no snapshot", name);
        }
    }
}
