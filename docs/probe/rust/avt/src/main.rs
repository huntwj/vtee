use avt::{Color, Vt};
use serde_json::{json, Value};
use std::fs;
use std::panic::{catch_unwind, AssertUnwindSafe};

const COLS: usize = 40;
const ROWS: usize = 12;
const TESTS: &[&str] = &[
    "T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13",
    "T14",
];

fn color_json(c: Option<Color>) -> Value {
    match c {
        None => Value::Null,
        Some(Color::Indexed(n)) => json!(n),
        Some(Color::RGB(rgb)) => json!([rgb.r, rgb.g, rgb.b]),
    }
}

fn run_one(name: &str) -> Option<Value> {
    let path = format!("../../{}.bin", name);
    let bytes = fs::read(&path).ok()?;
    let text = String::from_utf8_lossy(&bytes).into_owned();

    let mut vt = Vt::new(COLS, ROWS);
    vt.feed_str(&text);

    // `view()` is the current ROWS-sized viewport; `lines()` returns the full accumulated
    // buffer (viewport + scrolled-off history), which is where avt's scrollback actually lives.
    let lines: Vec<String> = vt.view().map(|l| l.text()).collect();
    let full_lines: Vec<String> = vt.lines().map(|l| l.text()).collect();
    let scrollback = if full_lines.len() > lines.len() {
        Some(full_lines[..full_lines.len() - lines.len()].to_vec())
    } else {
        None
    };

    let cell_grid: Vec<Vec<Value>> = vt
        .view()
        .map(|line| {
            line.cells()
                .iter()
                .map(|cell| {
                    let pen = cell.pen();
                    json!({
                        "char": cell.char().to_string(),
                        "fg": color_json(pen.foreground()),
                        "bg": color_json(pen.background()),
                        "bold": pen.is_bold(),
                        "dim": pen.is_faint(),
                        "italic": pen.is_italic(),
                        "underline": pen.is_underline(),
                        "blink": pen.is_blink(),
                        "reverse": pen.is_inverse(),
                        "strike": pen.is_strikethrough(),
                    })
                })
                .collect()
        })
        .collect();

    let cursor = vt.cursor();
    let modes: Vec<&str> = if vt.cursor_key_app_mode() {
        vec!["?1"]
    } else {
        vec![]
    };

    Some(json!({
        "test": name,
        "lines": lines,
        "cell": cell_grid,
        "cursor": {"row": cursor.row, "col": cursor.col, "hidden": !cursor.visible, "shape": null},
        "title": null,
        "modes": modes,
        "links": null,
        "scrollback": scrollback,
    }))
}

fn main() {
    for name in TESTS {
        let result = catch_unwind(AssertUnwindSafe(|| run_one(name)));
        match result {
            Ok(Some(v)) => println!("{}", v),
            Ok(None) => eprintln!("{}: no output (file missing?)", name),
            Err(_) => eprintln!("{}: panicked", name),
        }
    }
}
