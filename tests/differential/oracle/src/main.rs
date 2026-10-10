//! Runs every case in `tests/fixtures/differential/cases.json` through two independent terminal
//! emulators (`avt`, `vt100`) and writes their cell grids next to it, in the same normalised form
//! the vtee test uses. `null` means "this emulator cannot report that attribute".

use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "../../fixtures/differential".into()))
}

/// `\e` is ESC and `\` is a backslash; every other character is literal (the JSON already
/// carries CR, LF and BEL).
fn decode(input: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('e') => out.push(0x1b),
                Some('\\') => out.push(b'\\'),
                Some(other) => {
                    out.push(b'\\');
                    out.extend(other.to_string().bytes());
                }
                None => out.push(b'\\'),
            }
        } else {
            out.extend(c.to_string().bytes());
        }
    }
    out
}

fn avt_color(c: Option<avt::Color>) -> Value {
    match c {
        None => json!("default"),
        Some(avt::Color::Indexed(n)) => json!(format!("idx:{n}")),
        Some(avt::Color::RGB(rgb)) => json!(format!("rgb:{},{},{}", rgb.r, rgb.g, rgb.b)),
    }
}

fn run_avt(cols: usize, rows: usize, bytes: &[u8], resize: Option<(usize, usize)>) -> Value {
    let mut vt = avt::Vt::new(cols, rows);
    vt.feed_str(&String::from_utf8_lossy(bytes));
    if let Some((c, r)) = resize {
        vt.resize(c, r);
    }
    let grid: Vec<Vec<Value>> = vt
        .view()
        .map(|line| {
            line.cells()
                .iter()
                .map(|cell| {
                    let pen = cell.pen();
                    let ch = if cell.width() == 0 { String::new() } else { cell.char().to_string() };
                    json!({
                        "ch": ch,
                        "fg": avt_color(pen.foreground()),
                        "bg": avt_color(pen.background()),
                        "bold": pen.is_bold(),
                        "dim": pen.is_faint(),
                        "italic": pen.is_italic(),
                        "underline": pen.is_underline(),
                        "reverse": pen.is_inverse(),
                        "strike": pen.is_strikethrough(),
                    })
                })
                .collect()
        })
        .collect();
    let cur = vt.cursor();
    json!({"cells": grid, "cursor": {"row": cur.row, "col": cur.col, "hidden": !cur.visible}})
}

fn vt100_color(c: vt100::Color) -> Value {
    match c {
        vt100::Color::Default => json!("default"),
        vt100::Color::Idx(n) => json!(format!("idx:{n}")),
        vt100::Color::Rgb(r, g, b) => json!(format!("rgb:{r},{g},{b}")),
    }
}

fn run_vt100(cols: usize, rows: usize, bytes: &[u8], resize: Option<(usize, usize)>) -> Value {
    let mut parser = vt100::Parser::new(rows as u16, cols as u16, 0);
    parser.process(bytes);
    let (cols, rows) = match resize {
        Some((c, r)) => {
            parser.screen_mut().set_size(r as u16, c as u16);
            (c, r)
        }
        None => (cols, rows),
    };
    let screen = parser.screen();
    let mut grid = Vec::new();
    for r in 0..rows {
        let mut row = Vec::new();
        for c in 0..cols {
            let cell = screen.cell(r as u16, c as u16).expect("cell in range");
            let ch = if cell.is_wide_continuation() {
                String::new()
            } else if cell.has_contents() {
                cell.contents().to_string()
            } else {
                " ".to_string()
            };
            row.push(json!({
                "ch": ch,
                "fg": vt100_color(cell.fgcolor()),
                "bg": vt100_color(cell.bgcolor()),
                "bold": cell.bold(),
                "dim": cell.dim(),
                "italic": cell.italic(),
                "underline": cell.underline(),
                "reverse": cell.inverse(),
                "strike": Value::Null,
            }));
        }
        grid.push(row);
    }
    let (row, col) = screen.cursor_position();
    json!({"cells": grid, "cursor": {"row": row, "col": col, "hidden": screen.hide_cursor()}})
}

const FLAGS: [(&str, char); 6] =
    [("bold", 'b'), ("dim", 'd'), ("italic", 'i'), ("underline", 'u'), ("reverse", 'r'), ("strike", 's')];

/// Dense grid -> `{rows, styled, unreported, cursor}`. A row is its cells' text joined, with NUL
/// standing in for the empty second half of a wide character so columns stay aligned. `styled` lists
/// only cells that are not default-coloured and flag-free: `[row, col, fg, bg, "bdiurs flags"]`.
fn compact(full: &Value) -> Value {
    if full.is_null() {
        return Value::Null;
    }
    let mut rows = Vec::new();
    let mut styled = Vec::new();
    let mut unreported = std::collections::BTreeSet::new();
    for (r, row) in full["cells"].as_array().unwrap().iter().enumerate() {
        let mut text = String::new();
        for (c, cell) in row.as_array().unwrap().iter().enumerate() {
            let ch = cell["ch"].as_str().unwrap();
            text.push_str(if ch.is_empty() { "\u{0}" } else { ch });
            let mut flags = String::new();
            for (name, letter) in FLAGS {
                match cell[name].as_bool() {
                    Some(true) => flags.push(letter),
                    Some(false) => {}
                    None => {
                        unreported.insert(name);
                    }
                }
            }
            let fg = cell["fg"].as_str().unwrap();
            let bg = cell["bg"].as_str().unwrap();
            if fg != "default" || bg != "default" || !flags.is_empty() {
                styled.push(json!([r, c, fg, bg, flags]));
            }
        }
        rows.push(Value::String(text));
    }
    json!({"rows": rows, "styled": styled, "unreported": unreported, "cursor": full["cursor"]})
}

fn main() {
    let dir = dir();
    let cases: Vec<Value> = serde_json::from_str(&fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    let mut avt_out = serde_json::Map::new();
    let mut vt100_out = serde_json::Map::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap().to_string();
        let cols = case["cols"].as_u64().unwrap() as usize;
        let rows = case["rows"].as_u64().unwrap() as usize;
        let bytes = decode(case["input"].as_str().unwrap());
        let resize = case.get("resize").map(|r| (r[0].as_u64().unwrap() as usize, r[1].as_u64().unwrap() as usize));
        let a = std::panic::catch_unwind(|| run_avt(cols, rows, &bytes, resize)).unwrap_or(Value::Null);
        let v = std::panic::catch_unwind(|| run_vt100(cols, rows, &bytes, resize)).unwrap_or(Value::Null);
        avt_out.insert(name.clone(), compact(&a));
        vt100_out.insert(name, compact(&v));
    }
    // Compact: one cell per line would be huge, so write each case on its own line.
    for (file, map) in [("oracle_avt.json", avt_out), ("oracle_vt100.json", vt100_out)] {
        let mut text = String::from("{\n");
        let total = map.len();
        for (i, (k, v)) in map.iter().enumerate() {
            text.push_str(&format!("{}: {}{}\n", serde_json::to_string(k).unwrap(), v, if i + 1 < total { "," } else { "" }));
        }
        text.push_str("}\n");
        fs::write(dir.join(file), text).unwrap();
    }
    println!("wrote {} cases", cases.len());
}
