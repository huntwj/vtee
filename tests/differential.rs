//! Differential test of vtee's virtual terminal (`docs/development/testing.md`, "Differential
//! emulator tests").
//!
//! Every case in `tests/fixtures/differential/cases.json` is run through vtee's own emulator path
//! (`vtee::replay`, the same `Term` the live tap uses) and compared cell by cell with two
//! independent emulators, `avt` and `vt100`, whose output is committed as `oracle_avt.json` and
//! `oracle_vt100.json` (regenerate with `tests/differential/regenerate.sh`).
//!
//! A case passes when vtee equals both oracles, on every attribute both oracles can report. Where
//! the oracles disagree with each other or with vtee, `adjudications.json` records which side is
//! right and why; that is the only place a disagreement may be accepted. Set `VTEE_DIFF_REPORT=1`
//! to print every disagreement instead of failing, which is how new cases are triaged.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use vtee::{Color, Snapshot};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/differential")
}

fn load(name: &str) -> Value {
    let path = fixtures().join(name);
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `\e` is ESC and `\\` is a backslash; the JSON already carries CR, LF and BEL. The oracle
/// generator has the same function.
fn decode(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('e') => out.push('\x1b'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// One cell's attributes in the fixture vocabulary: colours as `default`, `idx:N`, `rgb:R,G,B`; flags
/// as the letters b d i u r s.
#[derive(Clone, PartialEq, Debug)]
struct Style {
    fg: String,
    bg: String,
    flags: String,
}

impl Style {
    fn default_style() -> Style {
        Style { fg: "default".into(), bg: "default".into(), flags: String::new() }
    }

    fn describe(&self) -> String {
        format!("fg={} bg={} flags=[{}]", self.fg, self.bg, self.flags)
    }

    fn without(&self, unreported: &BTreeSet<char>) -> Style {
        Style {
            fg: self.fg.clone(),
            bg: self.bg.clone(),
            flags: self.flags.chars().filter(|c| !unreported.contains(c)).collect(),
        }
    }
}

/// A screen in comparable form: one string per row (NUL for the empty second half of a wide
/// character), the style of every cell, and the cursor.
#[derive(Clone)]
struct Screen {
    rows: Vec<String>,
    styles: BTreeMap<(usize, usize), Style>,
    cursor: (u64, u64, bool),
    unreported: BTreeSet<char>,
}

fn flag_letter(name: &str) -> char {
    match name {
        "bold" => 'b',
        "dim" => 'd',
        "italic" => 'i',
        "underline" => 'u',
        "reverse" => 'r',
        "strike" => 's',
        other => panic!("unknown flag {other}"),
    }
}

fn from_oracle(v: &Value) -> Screen {
    let rows = v["rows"].as_array().unwrap().iter().map(|r| r.as_str().unwrap().to_string()).collect();
    let mut styles = BTreeMap::new();
    for s in v["styled"].as_array().unwrap() {
        let s = s.as_array().unwrap();
        styles.insert(
            (s[0].as_u64().unwrap() as usize, s[1].as_u64().unwrap() as usize),
            Style {
                fg: s[2].as_str().unwrap().into(),
                bg: s[3].as_str().unwrap().into(),
                flags: s[4].as_str().unwrap().into(),
            },
        );
    }
    let c = &v["cursor"];
    Screen {
        rows,
        styles,
        cursor: (c["row"].as_u64().unwrap(), c["col"].as_u64().unwrap(), c["hidden"].as_bool().unwrap()),
        unreported: v["unreported"].as_array().unwrap().iter().map(|n| flag_letter(n.as_str().unwrap())).collect(),
    }
}

fn color_string(c: Color) -> String {
    match c {
        Color::Default => "default".into(),
        Color::Indexed(n) => format!("idx:{n}"),
        Color::Rgb(r, g, b) => format!("rgb:{r},{g},{b}"),
    }
}

fn from_snapshot(snap: &Snapshot) -> Screen {
    let mut rows = Vec::new();
    let mut styles = BTreeMap::new();
    for (r, row) in snap.cells.iter().enumerate() {
        let mut text = String::new();
        for (c, cell) in row.iter().enumerate() {
            text.push_str(if cell.ch.is_empty() { "\u{0}" } else { &cell.ch });
            let mut flags = String::new();
            for (on, letter) in [
                (cell.bold, 'b'),
                (cell.dim, 'd'),
                (cell.italic, 'i'),
                (cell.underline, 'u'),
                (cell.reverse, 'r'),
                (cell.strike, 's'),
            ] {
                if on {
                    flags.push(letter);
                }
            }
            let style = Style { fg: color_string(cell.fg), bg: color_string(cell.bg), flags };
            if style != Style::default_style() {
                styles.insert((r, c), style);
            }
        }
        rows.push(text);
    }
    Screen {
        rows,
        styles,
        cursor: (snap.cursor.row as u64, snap.cursor.col as u64, snap.cursor.hidden),
        unreported: BTreeSet::new(),
    }
}

/// The first difference between two screens, ignoring flags that either side cannot report.
fn first_difference(a: &Screen, a_name: &str, b: &Screen, b_name: &str) -> Option<String> {
    let skip: BTreeSet<char> = a.unreported.union(&b.unreported).copied().collect();
    for (r, (ra, rb)) in a.rows.iter().zip(&b.rows).enumerate() {
        if ra != rb {
            let c = ra.chars().zip(rb.chars()).position(|(x, y)| x != y).unwrap_or(0);
            return Some(format!(
                "row {r} text, first difference at col {c}: {a_name}={:?} {b_name}={:?}",
                ra.replace('\u{0}', "~"),
                rb.replace('\u{0}', "~")
            ));
        }
    }
    let positions: BTreeSet<&(usize, usize)> = a.styles.keys().chain(b.styles.keys()).collect();
    for &(r, c) in positions {
        let sa = a.styles.get(&(r, c)).cloned().unwrap_or_else(Style::default_style).without(&skip);
        let sb = b.styles.get(&(r, c)).cloned().unwrap_or_else(Style::default_style).without(&skip);
        if sa != sb {
            return Some(format!(
                "cell (row {r}, col {c}) attributes: {a_name}={} {b_name}={}",
                sa.describe(),
                sb.describe()
            ));
        }
    }
    if a.cursor != b.cursor {
        return Some(format!("cursor (row, col, hidden): {a_name}={:?} {b_name}={:?}", a.cursor, b.cursor));
    }
    None
}

fn run_vtee(dir: &Path, name: &str, cols: u16, rows: u16, input: &str) -> Snapshot {
    let path = dir.join(format!("{name}.cast"));
    let header = format!("{{\"version\":2,\"width\":{cols},\"height\":{rows}}}");
    let event = serde_json::json!([0.0, "o", input]);
    fs::write(&path, format!("{header}\n{event}\n")).unwrap();
    vtee::replay(&path, cols, rows).unwrap_or_else(|e| panic!("{name}: replay failed: {e}"))
}

/// Live resize needs a running pty (replay has no resize event), so these cases go through `Tap`,
/// on Unix only. The child prints the case input byte for byte (`stty -onlcr` keeps the pty from
/// rewriting newlines), the test waits until every byte has been seen, then resizes.
#[cfg(unix)]
fn run_vtee_resized(cols: u16, rows: u16, input: &str, resize: (u16, u16)) -> Snapshot {
    use std::time::{Duration, Instant};
    let octal: String = input.bytes().map(|b| format!("\\0{b:03o}")).collect();
    let script = format!("stty -onlcr; printf '%b' '{octal}'; sleep 30");
    let opts = vtee::SpawnOptions { cols, rows, log_path: None, forward_input: false, forward_output: false, env: &[] };
    let tap = vtee::Tap::spawn("sh", &["-c".to_string(), script], opts).expect("spawn sh");
    let deadline = Instant::now() + Duration::from_secs(20);
    while tap.bytes_seen() < input.len() as u64 {
        assert!(
            Instant::now() < deadline,
            "child output never arrived ({} of {} bytes)",
            tap.bytes_seen(),
            input.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    tap.resize(resize.0, resize.1).expect("resize");
    let snap = tap.snapshot();
    tap.kill();
    snap
}

/// Checks that only vtee can answer: title, hyperlinks, cursor shape.
fn check_extras(case: &Value, snap: &Snapshot) -> Option<String> {
    let check = case.get("check")?;
    if let Some(hidden) = check.get("cursor_hidden").and_then(Value::as_bool)
        && snap.cursor.hidden != hidden
    {
        return Some(format!("cursor hidden: want {hidden}, got {}", snap.cursor.hidden));
    }
    if let Some(shape) = check.get("cursor_shape").and_then(Value::as_str) {
        let got = format!("{:?}", snap.cursor.shape);
        if got != shape {
            return Some(format!("cursor shape: want {shape}, got {got}"));
        }
    }
    if let Some(title) = check.get("title").and_then(Value::as_str)
        && snap.title.as_deref() != Some(title)
    {
        return Some(format!("title: want {title:?}, got {:?}", snap.title));
    }
    if let Some(links) = check.get("hyperlink_at").and_then(Value::as_array) {
        for link in links {
            let (r, c, url) =
                (link[0].as_u64().unwrap() as usize, link[1].as_u64().unwrap() as usize, link[2].as_str().unwrap());
            let got = snap.cells[r][c].hyperlink.as_deref();
            if got != Some(url) {
                return Some(format!("hyperlink at ({r},{c}): want {url:?}, got {got:?}"));
            }
        }
    }
    None
}

#[test]
fn vtee_agrees_with_independent_emulators() {
    let cases = load("cases.json");
    let avt = load("oracle_avt.json");
    let vt100 = load("oracle_vt100.json");
    let adjudications = load("adjudications.json");
    // `VTEE_DIFF_REPORT=<file>` lists every disagreement as JSON (case -> problems) in that file
    // instead of failing, which is the starting point for a new adjudication.
    let report_file = std::env::var_os("VTEE_DIFF_REPORT").map(PathBuf::from);
    let report_only = report_file.is_some();
    let mut report = serde_json::Map::new();

    let dir = std::env::temp_dir().join(format!("vtee-differential-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    let mut failures = Vec::new();
    let cases = cases.as_array().unwrap();
    assert!(cases.len() >= 40, "the corpus should have at least 40 cases, has {}", cases.len());

    for case in cases {
        let name = case["name"].as_str().unwrap();
        let cols = case["cols"].as_u64().unwrap() as u16;
        let rows = case["rows"].as_u64().unwrap() as u16;
        let input = decode(case["input"].as_str().unwrap());
        let resize = case.get("resize").map(|r| (r[0].as_u64().unwrap() as u16, r[1].as_u64().unwrap() as u16));
        let snap = match resize {
            None => run_vtee(&dir, name, cols, rows, &input),
            #[cfg(unix)]
            Some(to) => run_vtee_resized(cols, rows, &input, to),
            #[cfg(not(unix))]
            Some(_) => continue, // live resize is covered on Unix only
        };
        let ours = from_snapshot(&snap);
        let a = from_oracle(&avt[name]);
        let v = from_oracle(&vt100[name]);

        let mut problems = Vec::new();
        if let Some(d) = first_difference(&a, "avt", &v, "vt100") {
            problems.push(format!("oracles disagree: {d}"));
        }
        if let Some(d) = first_difference(&ours, "vtee", &a, "avt") {
            problems.push(format!("vtee vs avt: {d}"));
        }
        if let Some(d) = first_difference(&ours, "vtee", &v, "vt100") {
            problems.push(format!("vtee vs vt100: {d}"));
        }
        if let Some(d) = check_extras(case, &snap) {
            problems.push(format!("vtee only check: {d}"));
        }

        if report_only {
            if !problems.is_empty() {
                report.insert(name.to_string(), Value::from(problems));
            }
            continue;
        }

        match adjudications.get(name) {
            None if problems.is_empty() => {}
            None => failures.push(format!("{name}: {}", problems.join("\n    "))),
            Some(adj) => {
                // An adjudication pins exactly the problems it names, so a fix (or a regression)
                // shows up as a mismatch here instead of passing silently.
                let want: Vec<String> =
                    adj["problems"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
                if problems != want {
                    failures.push(format!(
                        "{name}: adjudicated problems no longer match ({}).\n    now: {problems:#?}\n    recorded: {want:#?}",
                        adj["verdict"].as_str().unwrap_or("")
                    ));
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);

    if let Some(path) = report_file {
        fs::write(&path, serde_json::to_string_pretty(&Value::Object(report)).unwrap()).unwrap();
        return;
    }
    assert!(failures.is_empty(), "{} case(s) differ:\n  {}", failures.len(), failures.join("\n  "));
}
