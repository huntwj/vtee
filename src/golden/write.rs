//! Writing a golden in its one canonical form (design section 6.12), and building a golden from the screens of a run.

use std::collections::BTreeSet;
use std::fmt::{self, Write as _};

use super::escape;
use super::model::*;
use crate::snapshot::Snapshot;

/// How `Golden::record` builds a golden.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecordOptions {
    /// Write the portable profile of section 6.14 as file-level `ignore` lines, so the golden passes on Linux and under
    /// ConPTY for the known differences.
    pub portable: bool,
}

/// The `ignore` lines `--portable` writes. Frozen at 0.1.0 (section 6.14).
pub fn portable_profile() -> Ignore {
    Ignore {
        title: true,
        modes: ["?1", "?1000", "?1002", "?1003", "?1005", "?1006", "DECKPAM"].iter().map(|m| m.to_string()).collect(),
        attrs: BTreeSet::from([Attr::Ulcolor]),
        ..Ignore::default()
    }
}

impl GoldenScreen {
    /// The expected screen a snapshot describes: no masks, no assertions, no `ignore`.
    pub fn from_snapshot(snapshot: &Snapshot, name: Option<String>) -> GoldenScreen {
        let mut wrapped = snapshot.wrapped_rows.clone();
        wrapped.sort_unstable();
        wrapped.dedup();
        GoldenScreen {
            line: 0,
            cols: snapshot.cols,
            rows: snapshot.rows,
            cells: snapshot.cells.clone(),
            cursor_row: snapshot.cursor.row,
            cursor_col: snapshot.cursor.col,
            cursor_shape: snapshot.cursor.shape,
            cursor_hidden: snapshot.cursor.hidden,
            modes: sorted_modes(&snapshot.modes),
            alt_screen: snapshot.alt_screen,
            title: snapshot.title.clone(),
            wrapped,
            name,
            ignore: Ignore::default(),
            header_comments: Vec::new(),
            x_lines: Vec::new(),
            masks: Vec::new(),
            asserts: Vec::new(),
        }
    }
}

impl Golden {
    /// A new golden holding the screens of a run, stamped with the numbers the first snapshot carries. `names` pairs
    /// with `snapshots` by position (a shorter list means no name). Panics if `snapshots` is empty: a golden has at
    /// least one screen.
    pub fn record(snapshots: &[Snapshot], names: &[Option<String>], options: RecordOptions) -> Golden {
        let first = snapshots.first().expect("a golden holds at least one screen");
        let producer = &first.producer;
        let x_lines = if producer.vtee.is_empty() && producer.emulator.is_empty() {
            Vec::new()
        } else {
            vec![format!("x-producer vtee {} {}", producer.vtee, producer.emulator)]
        };
        Golden {
            grammar: GRAMMAR_VERSION,
            schema: first.schema_version,
            semantics: Semantics::Number(first.semantics_version),
            ignore: if options.portable { portable_profile() } else { Ignore::default() },
            header_comments: Vec::new(),
            x_lines,
            shared_masks: Vec::new(),
            screens: snapshots
                .iter()
                .enumerate()
                .map(|(i, s)| GoldenScreen::from_snapshot(s, names.get(i).cloned().flatten()))
                .collect(),
        }
    }

    /// The canonical text of the golden.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "vtee-golden {}", self.grammar);
        let _ = writeln!(out, "schema {}", self.schema);
        let _ = writeln!(out, "semantics {}", self.semantics);
        header_tail(&mut out, &self.ignore, &self.header_comments, &self.x_lines);
        if !self.shared_masks.is_empty() {
            out.push_str("\n[masks]\n");
            entries(&mut out, &self.shared_masks, |m| &m.raw);
        }
        for (i, screen) in self.screens.iter().enumerate() {
            let _ = writeln!(out, "\n[screen {}]", i + 1);
            screen.write(&mut out);
        }
        out
    }
}

impl fmt::Display for Golden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

fn header_tail(out: &mut String, ignore: &Ignore, comments: &[String], x_lines: &[String]) {
    for line in ignore.lines() {
        let _ = writeln!(out, "ignore {line}");
    }
    for line in comments.iter().chain(x_lines) {
        let _ = writeln!(out, "{line}");
    }
}

fn entries<T>(out: &mut String, list: &[Entry<T>], raw: impl Fn(&T) -> &String) {
    for entry in list {
        match entry {
            Entry::Comment(text) => out.push_str(text),
            Entry::Item(item) => out.push_str(raw(item)),
        }
        out.push('\n');
    }
}

impl GoldenScreen {
    fn write(&self, out: &mut String) {
        let _ = writeln!(out, "size {}x{}", self.cols, self.rows);
        let _ = writeln!(
            out,
            "cursor row={} col={} shape={} hidden={}",
            self.cursor_row,
            self.cursor_col,
            shape_name(self.cursor_shape),
            self.cursor_hidden
        );
        if self.modes.is_empty() {
            out.push_str("modes\n");
        } else {
            let _ = writeln!(out, "modes {}", self.modes.join(" "));
        }
        let _ = writeln!(out, "alt_screen {}", self.alt_screen);
        match &self.title {
            None => out.push_str("title null\n"),
            Some(title) => {
                let _ = writeln!(out, "title {}", escape::encode_string(title));
            }
        }
        if !self.wrapped.is_empty() {
            let rows: Vec<String> = self.wrapped.iter().map(usize::to_string).collect();
            let _ = writeln!(out, "wrapped {}", rows.join(" "));
        }
        if let Some(name) = &self.name {
            let _ = writeln!(out, "name {}", escape::encode_string(name));
        }
        header_tail(out, &self.ignore, &self.header_comments, &self.x_lines);

        let width = (self.rows - 1).to_string().len();
        out.push_str("\n[text]\n");
        for (r, row) in self.cells.iter().enumerate() {
            let _ = writeln!(out, "{r:>width$}|{}", escape::encode_row(row.iter().map(|c| c.ch.as_str())));
        }

        let none = BTreeSet::new();
        let mut runs = String::new();
        for (r, row) in self.cells.iter().enumerate() {
            let mut c = 0;
            while c < row.len() {
                let tokens = attr_tokens(&row[c], &none);
                if tokens.is_empty() {
                    c += 1;
                    continue;
                }
                let mut end = c;
                while end + 1 < row.len() && attr_tokens(&row[end + 1], &none) == tokens {
                    end += 1;
                }
                let span = if end == c { c.to_string() } else { format!("{c}-{end}") };
                let _ = writeln!(runs, "{r:>width$}: {span} {}", tokens.join(" "));
                c = end + 1;
            }
        }
        if !runs.is_empty() {
            out.push_str("\n[attrs]\n");
            out.push_str(&runs);
        }
        if !self.masks.is_empty() {
            out.push_str("\n[masks]\n");
            entries(out, &self.masks, |m| &m.raw);
        }
        if !self.asserts.is_empty() {
            out.push_str("\n[assert]\n");
            entries(out, &self.asserts, |a| &a.raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A2: &str = "vtee-golden 1
schema 1
semantics 1
ignore modes ?1 ?1000
ignore title
x-producer vtee 0.1.0 alacritty_terminal 0.26.0

[masks]
# the clock
mask row=0 cols=1-2 text=/\\d\\d/

[screen 1]
size 6x2
cursor row=0 col=0 shape=block hidden=false
modes ?1 DECKPAM
alt_screen false
title \"a \\\"quoted\\\" title\"
wrapped 0
name \"first\"
ignore wrapped

[text]
0|ab 12
1|\\u{1}\\+{301}\\u{e000}\\\\

[attrs]
0: 3-4 underline=curly bold
1: 0 fg=idx:1 bg=rgb:#c80a14 link=\"https://example.org/a b\"

[masks]
mask row=1 text=ignore

[assert]
assert row=0 text=\"ab 12\" !bold

[screen 2]
size 3x1
cursor row=0 col=2 shape=bar hidden=true
modes
alt_screen true
title null

[text]
0|\u{4f60}\\.x
";

    #[test]
    fn a_canonical_file_survives_a_read_and_a_write_unchanged() {
        let golden = Golden::parse_str(A2).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(golden.to_text(), A2);
    }

    #[test]
    fn recording_a_snapshot_gives_a_golden_that_reads_back_the_same() {
        let mut snapshot: Snapshot = serde_json::from_str(
            r#"{"schema_version":1,"semantics_version":1,"byte_offset":9,"cols":4,"rows":2,"lines":["a   ",""],
            "cells":[[{"ch":"a","fg":"idx:2","bold":true},{},{},{}],[{},{},{},{}]],"wrapped_rows":[1,0,1],
            "cursor":{"row":1,"col":3,"hidden":false,"shape":"bar"},"modes":["DECKPAM","?1000","?1"],
            "alt_screen":false,"title":"t","scrollback":[],"producer":{"vtee":"0.1.0","emulator":"alacritty_terminal 0.26.0"}}"#,
        )
        .unwrap();
        snapshot.lines = vec!["a   ".into(), "    ".into()];
        let golden = Golden::record(&[snapshot], &[Some("one".to_string())], RecordOptions { portable: true });
        let text = golden.to_text();
        assert_eq!(
            text,
            "vtee-golden 1\nschema 1\nsemantics 1\nignore modes ?1 ?1000 ?1002 ?1003 ?1005 ?1006 DECKPAM\nignore attrs ulcolor\nignore title\n\
             x-producer vtee 0.1.0 alacritty_terminal 0.26.0\n\n[screen 1]\nsize 4x2\ncursor row=1 col=3 shape=bar hidden=false\n\
             modes ?1 ?1000 DECKPAM\nalt_screen false\ntitle \"t\"\nwrapped 0 1\nname \"one\"\n\n[text]\n0|a\n1|\n\n[attrs]\n0: 0 fg=idx:2 bold\n"
        );
        assert_eq!(Golden::parse_str(&text).unwrap().to_text(), text);
    }
}
