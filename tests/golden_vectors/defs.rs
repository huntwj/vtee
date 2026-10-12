//! The definitions the golden test vectors are generated from. Run `cargo test --test golden_vectors -- --ignored bless`
//! to write them to `tests/fixtures/golden-vectors/`; the files, not these definitions, are the frozen data.
//!
//! Every definition says what the verdict must be (exit code, and text that must appear in the report), so the
//! generated expectations are checked against the design, not just against what this implementation prints.

// The definitions read better with `&[screen.clone()]` than with `std::slice::from_ref`.
#![allow(clippy::cloned_ref_to_slice_refs)]

use vtee::golden::{Golden, RecordOptions};
use vtee::{Cell, Cursor, CursorShape, Snapshot};

/// A screen under construction: a snapshot plus the name the script gave its `snapshot` command.
#[derive(Clone)]
pub struct Scr {
    pub snap: Snapshot,
    pub name: Option<String>,
}

/// A screen of `cols` columns whose rows hold `rows` (plain text from column 0), cursor at the origin.
pub fn scr(cols: usize, rows: &[&str]) -> Scr {
    let cells = rows
        .iter()
        .map(|text| {
            let mut row: Vec<Cell> = text.chars().map(|c| Cell { ch: c.to_string(), ..Cell::default() }).collect();
            row.resize_with(cols, Cell::default);
            row
        })
        .collect();
    Scr {
        snap: Snapshot {
            schema_version: 1,
            semantics_version: 1,
            byte_offset: 0,
            cols,
            rows: rows.len(),
            lines: vec![],
            cells,
            wrapped_rows: vec![],
            cursor: Cursor { row: 0, col: 0, hidden: false, shape: CursorShape::Block },
            modes: vec![],
            alt_screen: false,
            title: None,
            scrollback: vec![],
            producer: Default::default(),
        },
        name: None,
    }
}

/// A 20-column screen.
pub fn plain(rows: &[&str]) -> Scr {
    scr(20, rows)
}

impl Scr {
    pub fn at(mut self, row: usize, col: usize, text: &str) -> Self {
        for (i, c) in text.chars().enumerate() {
            self.snap.cells[row][col + i].ch = c.to_string();
        }
        self
    }

    /// Sets the whole text of one cell (`""` makes a continuation cell, several scalars a combined cell).
    pub fn cell(mut self, row: usize, col: usize, ch: &str) -> Self {
        self.snap.cells[row][col].ch = ch.to_string();
        self
    }

    pub fn style(mut self, row: usize, first: usize, last: usize, change: impl Fn(&mut Cell)) -> Self {
        for c in first..=last {
            change(&mut self.snap.cells[row][c]);
        }
        self
    }

    pub fn bold(self, row: usize, first: usize, last: usize) -> Self {
        self.style(row, first, last, |c| c.bold = true)
    }

    pub fn fg(self, row: usize, first: usize, last: usize, colour: &str) -> Self {
        let colour: vtee::Color = colour.parse().unwrap();
        self.style(row, first, last, move |c| c.fg = colour)
    }

    pub fn cursor(mut self, row: usize, col: usize) -> Self {
        self.snap.cursor.row = row;
        self.snap.cursor.col = col;
        self
    }

    pub fn shape(mut self, shape: CursorShape) -> Self {
        self.snap.cursor.shape = shape;
        self
    }

    pub fn modes(mut self, modes: &[&str]) -> Self {
        self.snap.modes = modes.iter().map(|m| m.to_string()).collect();
        self
    }

    pub fn title(mut self, title: Option<&str>) -> Self {
        self.snap.title = title.map(str::to_string);
        self
    }

    pub fn wrapped(mut self, rows: &[usize]) -> Self {
        self.snap.wrapped_rows = rows.to_vec();
        self
    }

    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn semantics(mut self, n: u32) -> Self {
        self.snap.semantics_version = n;
        self
    }

    /// The same screen with its row `row` replaced by `text`.
    pub fn row(mut self, row: usize, text: &str) -> Self {
        for cell in &mut self.snap.cells[row] {
            *cell = Cell::default();
        }
        self.at(row, 0, text)
    }
}

/// The JSON reply line a `vtee --script` run prints for a `snapshot` command.
fn reply(index: usize, screen: &Scr) -> String {
    let mut snap = screen.snap.clone();
    snap.byte_offset = 100 * (index as u64 + 1);
    snap.lines = snap.cells.iter().map(|row| row.iter().map(|c| c.ch.as_str()).collect()).collect();
    let json = vtee::ndjson::to_line(&snap).unwrap();
    match &screen.name {
        Some(name) => {
            format!("{{\"cmd\":\"snapshot\",\"name\":{},\"snapshot\":{json}}}\n", vtee::ndjson::to_line(name).unwrap())
        }
        None => format!("{{\"cmd\":\"snapshot\",\"snapshot\":{json}}}\n"),
    }
}

/// The saved output of a run that produced `screens`.
pub fn run(screens: &[Scr]) -> String {
    screens.iter().enumerate().map(|(i, s)| reply(i, s)).collect()
}

/// A `wait_for` reply that timed out, the way a runner that knows the script line prints it.
pub fn timed_out_wait(line: usize, text: &str, optional: bool) -> String {
    let optional = if optional { ",\"optional\":true" } else { "" };
    format!(
        "{{\"cmd\":\"wait_for\",\"matched\":false,\"line\":{line},\"text\":\"{text}\",\"timeout_ms\":5000{optional}}}\n"
    )
}

/// A golden as text, edited by splicing lines into its canonical layout.
#[derive(Clone)]
pub struct G {
    head: String,
    screens: Vec<String>,
}

/// The golden that records `screens`.
pub fn gold(screens: &[Scr]) -> G {
    let snaps: Vec<Snapshot> = screens
        .iter()
        .map(|s| {
            let mut snap = s.snap.clone();
            snap.lines = snap.cells.iter().map(|row| row.iter().map(|c| c.ch.as_str()).collect()).collect();
            snap
        })
        .collect();
    let names: Vec<Option<String>> = screens.iter().map(|s| s.name.clone()).collect();
    G::from_text(&Golden::record(&snaps, &names, RecordOptions::default()).to_text())
}

/// The portable golden of `screens` (the profile of design section 6.14).
pub fn gold_portable(screens: &[Scr]) -> G {
    let snaps: Vec<Snapshot> = screens.iter().map(|s| s.snap.clone()).collect();
    G::from_text(&Golden::record(&snaps, &[], RecordOptions { portable: true }).to_text())
}

impl G {
    pub fn from_text(text: &str) -> G {
        let mut parts = text.split("\n[screen ");
        let head = parts.next().unwrap().to_string();
        G { head, screens: parts.map(str::to_string).collect() }
    }

    pub fn text(&self) -> String {
        let mut out = self.head.clone();
        for screen in &self.screens {
            out.push_str("\n[screen ");
            out.push_str(screen);
        }
        out
    }

    /// An `ignore` line (without the keyword) in the file header.
    pub fn file_ignore(mut self, line: &str) -> Self {
        self.head.push_str(&format!("ignore {line}\n"));
        self
    }

    /// The shared `[masks]` section.
    pub fn shared(mut self, lines: &[&str]) -> Self {
        self.head.push_str(&format!("\n[masks]\n{}\n", lines.join("\n")));
        self
    }

    /// An `ignore` line in the header of screen `n` (1-based).
    pub fn screen_ignore(mut self, n: usize, line: &str) -> Self {
        let part = &mut self.screens[n - 1];
        *part = part.replacen("\n\n[text]\n", &format!("\nignore {line}\n\n[text]\n"), 1);
        self
    }

    pub fn masks(mut self, n: usize, lines: &[&str]) -> Self {
        self.screens[n - 1].push_str(&format!("\n[masks]\n{}\n", lines.join("\n")));
        self
    }

    pub fn asserts(mut self, n: usize, lines: &[&str]) -> Self {
        self.screens[n - 1].push_str(&format!("\n[assert]\n{}\n", lines.join("\n")));
        self
    }

    pub fn replace(mut self, from: &str, to: &str) -> Self {
        let text = self.text();
        assert!(text.contains(from), "{from:?} not in\n{text}");
        self = G::from_text(&text.replacen(from, to, 1));
        self
    }
}

/// What a vector must produce.
pub enum Expect {
    /// A report with this exit code, and these strings in the text report.
    Report { exit: i32, contains: &'static [&'static str] },
    /// An error (no report) with this exit code and this text in the message.
    Error { exit: i32, contains: &'static str },
}

pub struct Vector {
    pub dir: &'static str,
    pub golden: Vec<u8>,
    pub run: String,
    pub args: Vec<&'static str>,
    pub expect: Expect,
}

fn v(dir: &'static str, golden: &G, run: String, args: &[&'static str], expect: Expect) -> Vector {
    Vector { dir, golden: golden.text().into_bytes(), run, args: args.to_vec(), expect }
}

fn pass(dir: &'static str, golden: &G, run: String) -> Vector {
    v(dir, golden, run, &[], Expect::Report { exit: 0, contains: &["PASS golden.golden"] })
}

fn fail(dir: &'static str, golden: &G, run: String, contains: &'static [&'static str]) -> Vector {
    v(dir, golden, run, &[], Expect::Report { exit: 1, contains })
}

fn refused(dir: &'static str, golden: &G, run: String, contains: &'static [&'static str]) -> Vector {
    v(dir, golden, run, &[], Expect::Report { exit: 2, contains })
}

fn clock(screen: Scr, time: &str) -> Scr {
    screen.at(3, 15, time)
}

/// The three screens of a small session; row 3 ends in a clock.
fn session() -> [Scr; 3] {
    [
        clock(plain(&["welcome", "", "", ""]), "12:01"),
        clock(plain(&["connected", "hello", "", ""]), "12:02"),
        clock(plain(&["done", "world", "", ""]), "12:03"),
    ]
}

pub fn run_vectors() -> Vec<Vector> {
    let mut all = Vec::new();

    // ---- masks ---------------------------------------------------------------------------------------------------
    let a = plain(&["abc", "def"]);
    all.push(pass(
        "R01-mask-text-ignore-pass",
        &gold(&[a.clone()]).masks(1, &["mask row=0 text=ignore"]),
        run(&[plain(&["xyz", "def"])]),
    ));

    let g = gold(&[plain(&["head", "123", "4567", "tail"])]).masks(1, &["mask rows=1-2 text=/[0-9]+/"]);
    all.push(pass("R02-mask-rows-range-per-row-regex", &g, run(&[plain(&["head", "999", "12", "tail"])])));
    all.push(fail(
        "R03-mask-rows-range-one-row-fails",
        &g,
        run(&[plain(&["head", "999", "1x", "tail"])]),
        &["FAIL golden.golden: 1 mismatch in 1 of 1 screen", "row 2 col 0-19   actual '1x' does not match"],
    ));

    let load = gold(&[plain(&["load 12.5 up"])]).masks(1, &[r"mask find=/[0-9]+\.[0-9]+/ text=ignore"]);
    all.push(pass("R04-mask-find-pass", &load, run(&[plain(&["load 99.1 up"])])));
    all.push(fail(
        "R05-mask-find-stale",
        &load,
        run(&[plain(&["load n/a up"])]),
        &["matched nothing (stale)", "text       row 0 col 5-11"],
    ));

    let g = gold(&[plain(&["x", "a 1", "b", "c 2"])]).masks(1, &["mask rows=1-3 find=/[0-9]+/ text=ignore"]);
    all.push(pass("R06-mask-find-missing-in-one-row-ok", &g, run(&[plain(&["x", "a 9", "b", "c 8"])])));

    let g = gold(&[plain(&["abc12 xyz"])]).masks(1, &["mask find=/[0-9]+$/ cols=0-5 text=ignore"]);
    all.push(pass("R07-mask-find-cols-anchors-inside", &g, run(&[plain(&["abc99 xyz"])])));

    let g = gold(&[plain(&["baa"])]).masks(1, &["mask find=/a*/ text=ignore"]);
    all.push(fail(
        "R08-mask-find-empty-match-skipped",
        &g,
        run(&[plain(&["Baa"])]),
        &["FAIL golden.golden: 1 mismatch", "text       row 0 col 0   expected 'b'   actual 'B'"],
    ));

    let wide = |colour: bool| {
        let s = plain(&["", "x"]).cell(0, 0, "你").cell(0, 1, "").cell(0, 2, "x");
        if colour { s.fg(0, 0, 1, "idx:1") } else { s }
    };
    let g = gold(&[wide(true)]).masks(1, &["mask find=/你/ attrs=ignore"]);
    all.push(pass("R09-mask-find-wide-char-covers-continuation", &g, run(&[wide(false)])));

    let accented = |base: &str| plain(&["ab"]).cell(0, 1, &format!("{base}\u{301}")).cell(0, 2, "b");
    let g = gold(&[accented("e")]).masks(1, &["mask find=/[^a-z]/ text=ignore"]);
    all.push(pass("R10-mask-find-inside-appended-scalars", &g, run(&[accented("x")])));

    let g = gold(&[plain(&["abc"]).bold(0, 0, 0)]).masks(1, &["mask row=0 attrs=ignore"]);
    all.push(fail(
        "R11-mask-attrs-ignore-keeps-text",
        &g,
        run(&[plain(&["abd"])]),
        &["FAIL golden.golden: 1 mismatch", "text       row 0 col 2"],
    ));

    let g = gold(&[plain(&["abc"]).bold(0, 0, 0)]).masks(1, &["mask row=0 text=ignore"]);
    all.push(fail(
        "R12-mask-text-ignore-keeps-attrs",
        &g,
        run(&[plain(&["xyz"])]),
        &["FAIL golden.golden: 1 mismatch", "attrs      row 0 col 0 (1 cell)   expected bold   actual (none)"],
    ));

    let g = gold(&[plain(&["abcdefgh"]).bold(0, 1, 1).bold(0, 6, 6)])
        .masks(1, &["mask row=0 text=ignore", "mask row=0 cols=0-3 attrs=ignore"]);
    all.push(fail(
        "R13-mask-overlap-skips-if-any-covers",
        &g,
        run(&[plain(&["aXcdefgh"])]),
        &["FAIL golden.golden: 1 mismatch", "attrs      row 0 col 6 (1 cell)"],
    ));

    let g = gold(&[plain(&["abc"])]).masks(1, &["mask row=0 text=/[a-z]+/"]);
    all.push(pass("R14-mask-whole-row-regex-trims", &g, run(&[plain(&["xyz"])])));

    let g = gold(&[plain(&["abc"])]).masks(1, &["mask row=0 cols=0-5 text=/[a-z]+/"]);
    all.push(fail(
        "R15-mask-cols-range-keeps-blanks",
        &g,
        run(&[plain(&["xyz"])]),
        &["row 0 col 0-5   actual 'xyz   ' does not match"],
    ));

    // ---- assertions ----------------------------------------------------------------------------------------------
    let meadow = plain(&["", "A Quiet Meadow"]);
    all.push(pass(
        "R16-assert-exact-whole-row-trimmed",
        &gold(&[meadow.clone()]).asserts(1, &[r#"assert row=1 text="A Quiet Meadow""#]),
        run(&[meadow.clone()]),
    ));
    all.push(fail(
        "R17-assert-exact-cols-keeps-blanks",
        &gold(&[meadow.clone()]).asserts(1, &[r#"assert row=1 cols=0-14 text="A Quiet Meadow""#]),
        run(&[meadow]),
        &["assert", "actual 'A Quiet Meadow '"],
    ));

    let composed = plain(&["e"]).cell(0, 0, "e\u{301}");
    all.push(fail(
        "R18-assert-exact-scalar-sequence",
        &gold(&[composed.clone()]).asserts(1, &["assert row=0 cols=0-0 text=\"\u{e9}\""]),
        run(&[composed]),
        &["assert", "golden line"],
    ));

    let errors = plain(&["ERR a ERR b"]).fg(0, 0, 2, "idx:1");
    all.push(fail(
        "R19-assert-find-every-match",
        &gold(&[errors.clone()]).asserts(1, &["assert find=/ERR/ fg=idx:1"]),
        run(&[errors]),
        &["row 0 col 6   actual fg=default   (first failing cell of 3)"],
    ));

    let bold_abc = plain(&["abc"]).bold(0, 0, 2);
    all.push(fail(
        "R20-assert-find-no-match",
        &gold(&[bold_abc.clone()]).asserts(1, &["assert find=/zzz/ bold"]),
        run(&[bold_abc]),
        &["matched nothing"],
    ));

    let four = plain(&["abcd"]).bold(0, 0, 1).bold(0, 3, 3);
    all.push(fail(
        "R21-assert-attr-every-cell",
        &gold(&[four.clone()]).asserts(1, &["assert row=0 cols=0-3 bold"]),
        run(&[four]),
        &["row 0 col 2   actual !bold   (first failing cell of 1)"],
    ));

    let curly = plain(&["abcd"]).style(0, 2, 3, |c| c.underline = vtee::UnderlineStyle::Curly);
    all.push(fail(
        "R22-assert-underline-none",
        &gold(&[curly.clone()])
            .asserts(1, &["assert row=0 cols=0-1 underline=none", "assert row=0 cols=2-3 underline=none"]),
        run(&[curly]),
        &["golden line", "row 0 col 2   actual underline=curly   (first failing cell of 2)"],
    ));

    let bold_a = plain(&["ab"]).bold(0, 0, 0);
    all.push(fail(
        "R23-assert-not-flag",
        &gold(&[bold_a.clone()]).asserts(1, &["assert row=0 !bold"]),
        run(&[bold_a]),
        &["row 0 col 0   actual bold   (first failing cell of 1)"],
    ));

    let linked = plain(&["abcdef"]).style(0, 0, 2, |c| c.hyperlink = Some("https://example.org/a".to_string()));
    all.push(fail(
        "R24-assert-link-and-not-link",
        &gold(&[linked.clone()]).asserts(
            1,
            &[
                r#"assert row=0 cols=0-2 link="https://example.org/a""#,
                "assert row=0 cols=3-5 !link",
                "assert row=0 cols=0-2 !link",
            ],
        ),
        run(&[linked]),
        &["FAIL golden.golden: 1 mismatch", r#"actual link="https://example.org/a"   (first failing cell of 3)"#],
    ));

    let ul = plain(&["ab"]).style(0, 0, 0, |c| c.underline_color = "rgb:#102030".parse().unwrap());
    all.push(pass(
        "R25-assert-ulcolor-without-underline",
        &gold(&[ul.clone()]).asserts(1, &["assert row=0 cols=0-0 ulcolor=rgb:#102030 underline=none"]),
        run(&[ul]),
    ));

    let g = gold(&[plain(&["ab", "cd"]).cursor(1, 2)]).screen_ignore(1, "cursor.position");
    all.push(fail(
        "R26-header-ignore-cursor-position-only",
        &g,
        run(&[plain(&["ab", "cd"]).cursor(0, 5).shape(CursorShape::Bar)]),
        &["FAIL golden.golden: 1 mismatch", "cursor     shape      expected block   actual bar"],
    ));

    // ---- versions ------------------------------------------------------------------------------------------------
    let base = gold(&[plain(&["abc"])]);
    let same = || run(&[plain(&["abc"])]);
    let other = || run(&[plain(&["xyz"])]);
    all.push(refused(
        "R27-schema-lower-refused",
        &base.clone().replace("schema 1", "schema 0"),
        same(),
        &["REFUSED golden.golden: schema 0 in the golden, 1 in this vtee; nothing was compared", "--update=migrate"],
    ));

    let g = gold(&[plain(&["abc"]).wrapped(&[0])]);
    all.push(fail(
        "R28-wrapped-mismatch",
        &g,
        run(&[plain(&["abc"])]),
        &["wrapped    expected 0   actual (none)   missing 0   extra (none)"],
    ));

    let g = gold(&[plain(&["abc", "def"])])
        .masks(1, &["mask find=/zzz/ text=ignore"])
        .asserts(1, &[r#"assert row=0 text="nope""#]);
    all.push(fail(
        "R29-size-mismatch-skips-cells-masks-asserts",
        &g,
        run(&[scr(21, &["xyz", "def"])]),
        &["FAIL golden.golden: 1 mismatch in 1 of 1 screen", "size       expected 20x2   actual 21x2"],
    ));

    all.push(refused(
        "R30-semantics-mismatch-refused-though-screens-equal",
        &base.clone().replace("semantics 1", "semantics 2"),
        same(),
        &["REFUSED golden.golden: semantics 2 in the golden, 1 in this vtee; nothing was compared", "--update=restamp"],
    ));
    all.push(refused(
        "R31-semantics-mismatch-refused-with-differences",
        &base.clone().replace("semantics 1", "semantics 2"),
        other(),
        &["REFUSED golden.golden: semantics 2 in the golden, 1 in this vtee"],
    ));
    all.push(refused(
        "R32-semantics-unknown-refused",
        &base.clone().replace("semantics 1", "semantics unknown"),
        same(),
        &["REFUSED golden.golden: semantics unknown in the golden, 1 in this vtee"],
    ));
    all.push(refused(
        "R33-snapshot-semantics-differs-from-golden",
        &base.clone(),
        run(&[plain(&["abc"]).semantics(2)]),
        &["REFUSED golden.golden: semantics 1 in the golden, 2 in the snapshots"],
    ));
    all.push(refused(
        "R34-schema-higher-refused",
        &base.clone().replace("schema 1", "schema 2"),
        same(),
        &["REFUSED golden.golden: schema 2 in the golden, 1 in this vtee", "upgrade vtee"],
    ));

    let provisional = concat!(
        r#"{"cmd":"snapshot","snapshot":{"schema_version":1,"byte_offset":5,"cols":2,"rows":1,"lines":["ab"],"#,
        r#""cells":[[{"ch":"a","fg":"Default","bg":"Default","bold":false},{"ch":"b","fg":"Default","bg":"Default","#,
        r#""bold":false}]],"cursor":{"row":0,"col":0,"hidden":false,"shape":"Block"},"modes":[],"alt_screen":false,"#,
        r#""title":null,"scrollback":[]}}"#,
        "\n"
    );
    all.push(v(
        "R35-provisional-shape-snapshot-malformed",
        &base,
        provisional.to_string(),
        &[],
        Expect::Error {
            exit: 65,
            contains: "snapshot: line 1: it has no `semantics_version`: snapshots recorded before 0.1.0",
        },
    ));

    let mut crlf = Vec::new();
    crlf.extend(base.text().replace('\n', "\r\n").into_bytes());
    all.push(Vector {
        dir: "R36-crlf-accepted",
        golden: crlf,
        run: same(),
        args: vec![],
        expect: Expect::Report { exit: 0, contains: &["PASS golden.golden (1 screen)"] },
    });

    let linux = plain(&["abc"]).modes(&["?1", "?1000", "?2004", "DECKPAM"]).style(0, 0, 0, |c| {
        c.underline = vtee::UnderlineStyle::Curly;
        c.underline_color = "rgb:#c80a14".parse().unwrap();
    });
    let portable = gold_portable(&[linux.clone()]);
    all.push(pass("R37-portable-golden-passes-linux-shaped-screen", &portable, run(&[linux.clone()])));
    let conpty = plain(&["abc"])
        .modes(&["?2004"])
        .title(Some("C:\\windows\\System32\\WindowsPowerShell\\v1.0\\powershell.EXE"))
        .style(0, 0, 0, |c| c.underline = vtee::UnderlineStyle::Curly);
    all.push(pass("R39-portable-golden-passes-conpty-shaped-screen", &portable, run(&[conpty])));
    all.push(fail(
        "R41-portable-golden-still-compares-the-rest",
        &portable,
        run(&[linux.modes(&["?2004", "?1"]).at(0, 2, "d")]),
        &["FAIL golden.golden: 1 mismatch", "text       row 0 col 2"],
    ));

    all.push(v(
        "R38-grammar-0-golden-rejected",
        &base.clone().replace("vtee-golden 1", "vtee-golden 0"),
        same(),
        &[],
        Expect::Error { exit: 65, contains: "line 1: `vtee-golden 0` is the unreleased draft layout" },
    ));
    all.push(refused(
        "R40-grammar-newer-refused",
        &base.replace("vtee-golden 1", "vtee-golden 2"),
        same(),
        &["REFUSED golden.golden: grammar 2 in the golden, 1 in this vtee; nothing was compared", "upgrade vtee"],
    ));

    // ---- composites ----------------------------------------------------------------------------------------------
    let [s1, s2, s3] = session();
    all.push(pass(
        "C01-composite-three-screens-pass",
        &gold(&[s1.clone(), s2.clone(), s3.clone()]),
        run(&[s1.clone(), s2.clone(), s3.clone()]),
    ));

    let clock_mask = r"mask row=3 cols=15-19 text=/\d\d:\d\d/";
    all.push(pass(
        "C02-shared-mask-applies-to-every-screen",
        &gold(&[s1.clone(), s2.clone(), s3.clone()]).shared(&[clock_mask]),
        run(&[clock(s1.clone(), "13:45"), clock(s2.clone(), "13:46"), clock(s3.clone(), "13:47")]),
    ));

    let moved = |s: &Scr| s.clone().row(1, "zzz");
    all.push(fail(
        "C03-per-screen-mask-applies-to-its-screen-only",
        &gold(&[s1.clone(), s2.clone()]).masks(1, &["mask row=1 text=ignore"]),
        run(&[moved(&s1), moved(&s2)]),
        &["FAIL golden.golden: 1 mismatch in 1 of 2 screens", "screen 2:", "text       row 1 col 0-4"],
    ));

    let bolder = |s: &Scr| s.clone().bold(0, 0, 2);
    all.push(pass(
        "C04-shared-and-screen-masks-union",
        &gold(&[bolder(&s1), s2.clone()]).shared(&["mask row=0 attrs=ignore"]).masks(1, &["mask row=0 text=ignore"]),
        run(&[s1.clone().row(0, "other"), s2.clone()]),
    ));

    let digit_clock = [clock(plain(&["a", "", "", ""]), "12:01"), plain(&["b", "", "", ""]), plain(&["c", "", "", ""])];
    let find_clock = r"mask find=/\d\d:\d\d/ text=ignore";
    all.push(pass(
        "C05-shared-find-matches-in-one-screen-not-stale",
        &gold(&digit_clock).shared(&[find_clock]),
        run(&[clock(plain(&["a", "", "", ""]), "09:59"), digit_clock[1].clone(), digit_clock[2].clone()]),
    ));
    let no_clock = [plain(&["a"]), plain(&["b"]), plain(&["c"])];
    all.push(fail(
        "C06-shared-find-matches-in-no-screen-stale",
        &gold(&no_clock).shared(&[find_clock]),
        run(&no_clock),
        &["FAIL golden.golden: 1 mismatch in 0 of 3 screens", "matched nothing (stale)"],
    ));

    let digits = [plain(&["a1"]), plain(&["b"]), plain(&["c"])];
    all.push(fail(
        "C07-per-screen-find-stale-in-its-screen",
        &gold(&digits).masks(2, &["mask find=/[0-9]+/ text=ignore"]),
        run(&digits),
        &["FAIL golden.golden: 1 mismatch in 1 of 3 screens", "screen 2:", "matched nothing (stale)"],
    ));

    all.push(pass(
        "C08-shared-ignore-applies-to-every-screen",
        &gold(&[s1.clone(), s2.clone()]).file_ignore("title"),
        run(&[s1.clone().title(Some("one")), s2.clone().title(Some("two"))]),
    ));

    all.push(fail(
        "C09-screen-ignore-applies-to-one-screen",
        &gold(&[s1.clone(), s2.clone()]).screen_ignore(1, "wrapped"),
        run(&[s1.clone().wrapped(&[0]), s2.clone().wrapped(&[0])]),
        &["FAIL golden.golden: 1 mismatch in 1 of 2 screens", "screen 2:", "wrapped    expected (none)   actual 0"],
    ));

    let narrow =
        gold(&[plain(&["a", "", "", ""]), scr(10, &["b", "", "", ""])]).shared(&["mask row=3 cols=15-19 text=ignore"]);
    all.push(v(
        "C10-shared-region-mask-outside-a-screen-rejected",
        &narrow,
        run(&[plain(&["a", "", "", ""]), scr(10, &["b", "", "", ""])]),
        &[],
        Expect::Error {
            exit: 65,
            contains: "the shared mask does not fit screen 2 (10x4): column 19 is outside the 10-column screen",
        },
    ));

    // ---- pairing -------------------------------------------------------------------------------------------------
    let two = gold(&[s1.clone(), s2.clone()]);
    all.push(fail(
        "P01-pair-by-position",
        &two,
        run(&[s1.clone(), moved(&s2)]),
        &["FAIL golden.golden: 1 mismatch in 1 of 2 screens", "screen 2:"],
    ));

    let three = gold(&[s1.clone(), s2.clone(), s3.clone()]);
    all.push(fail(
        "P02-fewer-screens-than-golden",
        &three,
        run(&[s1.clone(), moved(&s2)]),
        &["screens    expected 3   actual 2   missing 3", "screen 2:"],
    ));
    all.push(fail(
        "P03-more-screens-than-golden",
        &two,
        run(&[s1.clone(), s2.clone(), s3.clone()]),
        &["screens    expected 2   actual 3   extra 3"],
    ));
    all.push(fail(
        "P04-inserted-checkpoint-shifts-everything",
        &three,
        run(&[s1.clone(), plain(&["inserted"]), s2.clone(), s3.clone()]),
        &["screens    expected 3   actual 4   extra 4", "screen 2:", "screen 3:"],
    ));

    let named = |a: &str, b: &str| [s1.clone().named(a), s2.clone().named(b)];
    all.push(pass("P05-names-equal-pass", &gold(&named("one", "two")), run(&named("one", "two"))));
    all.push(fail(
        "P06-names-differ",
        &gold(&named("one", "two")),
        run(&[named("one", "TWO")[0].clone(), moved(&named("one", "TWO")[1])]),
        &["screen 2 \"two\":", "name       expected \"two\"   actual \"TWO\"", "text       row 1"],
    ));
    all.push(pass("P07-name-on-one-side-only-ignored", &gold(&named("one", "two")), run(&[s1.clone(), s2.clone()])));

    all.push(fail(
        "P08-one-screen-run-against-composite",
        &three,
        run(&[s1.clone()]),
        &["screens    expected 3   actual 1   missing 2 3   (use --screen N to compare one screen)"],
    ));
    all.push(v(
        "P09-screen-selector",
        &three,
        run(&[s2.clone()]),
        &["--screen", "2"],
        Expect::Report { exit: 0, contains: &["PASS golden.golden (3 screens)"] },
    ));
    all.push(v(
        "P10-screen-selector-out-of-range",
        &three,
        run(&[s2.clone()]),
        &["--screen", "4"],
        Expect::Error { exit: 64, contains: "vtee: --screen 4 selects one screen, but the golden has 3 screens" },
    ));

    // ---- run-level problems --------------------------------------------------------------------------------------
    let wait_run = |optional: bool, second: &Scr| {
        let mut text = run(&[s1.clone()]);
        text.push_str(&timed_out_wait(7, "10/10mv>", optional));
        text.push_str(&reply(1, second));
        text
    };
    all.push(fail(
        "W01-wait-timeout-fails",
        &two,
        wait_run(false, &moved(&s2)),
        &["wait       script line 7   wait_for \"10/10mv>\" timed out after 5000 ms", "screen 2:"],
    ));
    all.push(pass("W02-wait-timeout-optional-passes", &two, wait_run(true, &s2)));
    all.push(fail(
        "W03-wait-timeout-with-matching-screens",
        &two,
        wait_run(false, &s2),
        &["FAIL golden.golden: 1 mismatch in 0 of 2 screens", "wait       script line 7"],
    ));
    let mut ff = run(&[s1.clone()]);
    ff.push_str(&timed_out_wait(7, "10/10mv>", false));
    ff.push_str(&reply(1, &moved(&s2)));
    ff.push_str(&reply(2, &moved(&s3)));
    all.push(v(
        "W04-fail-fast-stops-at-first-problem",
        &three,
        ff,
        &["--fail-fast"],
        Expect::Report {
            exit: 1,
            contains: &["FAIL golden.golden: 1 mismatch in 0 of 3 screens", "wait       script line 7"],
        },
    ));
    let mut other = run(&[s1.clone()]);
    other.push_str("{\"cmd\":\"wait_exit\",\"running\":false,\"code\":0,\"timed_out\":false}\n");
    other.push_str("{\"cmd\":\"exit_status\",\"running\":false,\"code\":0}\n");
    other.push_str("{\"cmd\":\"error\",\"message\":\"nope\"}\n");
    other.push_str(&reply(1, &s2));
    all.push(pass("W05-other-replies-are-not-screens", &two, other));

    all
}

/// The regex vectors of design appendix A.3: pattern, subject, operation, result.
pub const REGEX_VECTORS: &str = r"# Vectors for the regex dialect of design section 6.7.
# Columns, separated by a tab: pattern, subject, operation, result.
# The subject is text with U+H written as \u{H} (and nothing else escaped; a backslash is \u{5c}).
# Operations: search (leftmost-first: `match [a,b)` or `no match`), full (is the whole subject matched: `match` or
# `no match`), find (the scan loop of section 6.5, skipping empty matches: spans like `[1,3) [5,7)`, or `none`),
# reject (the pattern is not in the dialect: the result is `reject`), line (the pattern column is a whole `mask` line
# and the golden is rejected: `reject`).
# Positions are scalar offsets; [a,b) is start inclusive and end exclusive.
abc	xabcx	search	match [1,4)
a.c	abc	full	match
\d+	ab 123 x	search	match [3,6)
\d+	\u{663}\u{664}	search	no match
\w+	\u{e9}_x	search	match [1,3)
\s	a\u{a0}b	search	no match
[\d.]+	v1.25x	search	match [1,5)
[^a-c]	abcd	search	match [3,4)
^ab	ab	search	match [0,2)
b$	ab	search	match [1,2)
^$		full	match
a*	baa	find	[1,3)
a+?	aaa	search	match [0,1)
a|ab	ab	search	match [0,1)
(a|ab)(c|bcd)	abcd	search	match [0,4)
x{2,3}	xxxx	search	match [0,3)
x{2,3}?	xxxx	search	match [0,2)
(?:ab)+	ababab	search	match [0,6)
\+\d	a+1	search	match [1,3)
a\/b	a/b	search	match [0,3)
\.\.	a..b	search	match [1,3)
[ \d:.]+	  1:23.4 x	search	match [0,9)
[A-Z]	abC	search	match [2,3)
\D	12a	search	match [2,3)
\S+	  ab c	search	match [2,4)
\W	a-b	search	match [1,2)
.	\u{4f60}	search	match [0,1)
[a-c]{2}	xabcx	search	match [1,3)
(ab|cd)+	abcdab	search	match [0,6)
\d\d:\d\d	at 03:18 now	search	match [3,8)
\bfoo		reject	reject
(?i)abc		reject	reject
\x41		reject	reject
\p{L}		reject	reject
(?=a)		reject	reject
(a)\1		reject	reject
[[:alpha:]]		reject	reject
(?P<n>a)		reject	reject
\Aa		reject	reject
a\z		reject	reject
\u0041		reject	reject
\h		reject	reject
a**		reject	reject
mask row=0 text=/a/b/		line	reject
(a		reject	reject
[a		reject	reject
";
