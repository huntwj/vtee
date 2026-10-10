//! Definitions of the `reject/` and `roundtrip/` vectors (see `defs.rs`).

use super::defs::{G, Scr, gold, gold_portable, plain, scr};

/// A small canonical golden every reject vector is a corruption of.
pub const BASE: &str = r#"vtee-golden 1
schema 1
semantics 1

[screen 1]
size 6x2
cursor row=0 col=0 shape=block hidden=false
modes
alt_screen false
title null

[text]
0|ab 12
1|

[attrs]
0: 3-4 bold

[masks]
mask row=0 cols=3-4 text=/[0-9]+/

[assert]
assert row=0 text="ab 12"
"#;

fn edit(from: &str, to: &str) -> Vec<u8> {
    assert!(BASE.contains(from), "{from:?} is not in the base golden");
    BASE.replacen(from, to, 1).into_bytes()
}

/// Goldens a reader must reject (exit 65), one per rule of design section 6.13: name and file content.
pub fn rejects() -> Vec<(&'static str, Vec<u8>)> {
    let mut invalid_utf8 = BASE.as_bytes().to_vec();
    invalid_utf8.splice(5..5, [0xff, 0xfe]);
    vec![
        ("bom", format!("\u{feff}{BASE}").into_bytes()),
        ("lone-cr", edit("0|ab 12", "0|ab\r 12")),
        ("nul-byte", edit("0|ab 12", "0|ab\0 12")),
        ("invalid-utf8", invalid_utf8),
        ("empty-file", Vec::new()),
        ("first-line-not-vtee-golden", edit("vtee-golden 1", "golden 1")),
        ("first-line-without-number", edit("vtee-golden 1", "vtee-golden")),
        ("first-line-grammar-zero", edit("vtee-golden 1", "vtee-golden 0")),
        ("missing-schema", edit("schema 1\n", "")),
        ("missing-semantics", edit("semantics 1\n", "")),
        ("missing-size", edit("size 6x2\n", "")),
        ("missing-cursor", edit("cursor row=0 col=0 shape=block hidden=false\n", "")),
        ("missing-modes", edit("modes\n", "")),
        ("missing-alt-screen", edit("alt_screen false\n", "")),
        ("missing-title", edit("title null\n", "")),
        ("duplicate-schema", edit("schema 1\n", "schema 1\nschema 1\n")),
        ("duplicate-size", edit("size 6x2\n", "size 6x2\nsize 6x2\n")),
        ("duplicate-title", edit("title null\n", "title null\ntitle null\n")),
        ("unknown-file-header-key", edit("semantics 1\n", "semantics 1\nsurprise 3\n")),
        ("unknown-screen-header-key", edit("title null\n", "title null\nsurprise 3\n")),
        ("size-in-the-file-header", edit("semantics 1\n", "semantics 1\nsize 6x2\n")),
        ("schema-in-a-screen-header", edit("title null\n", "title null\nschema 1\n")),
        ("key-without-value", edit("title null\n", "title\n")),
        ("unknown-section", edit("[assert]", "[bogus]")),
        ("duplicate-text-section", edit("[attrs]", "[text]\n0|\n1|\n\n[attrs]")),
        ("duplicate-masks-section", edit("[assert]", "[masks]\nmask row=1 text=ignore\n\n[assert]")),
        (
            "two-shared-masks-sections",
            edit("[screen 1]", "[masks]\nmask row=0 text=ignore\n\n[masks]\nmask row=1 text=ignore\n\n[screen 1]"),
        ),
        ("no-screen-block", b"vtee-golden 1\nschema 1\nsemantics 1\n".to_vec()),
        ("screen-number-wrong", edit("[screen 1]", "[screen 2]")),
        ("screen-without-number", edit("[screen 1]", "[screen]")),
        ("text-before-the-first-screen", edit("[screen 1]", "[text]\n0|ab\n\n[screen 1]")),
        ("attrs-before-the-first-screen", edit("[screen 1]", "[attrs]\n0: 0 bold\n\n[screen 1]")),
        ("assert-before-the-first-screen", edit("[screen 1]", "[assert]\nassert row=0 bold\n\n[screen 1]")),
        ("exit-section", edit("[assert]", "[exit]\n\n[assert]")),
        ("wrong-number-of-text-rows", edit("1|\n", "")),
        ("text-row-with-the-wrong-label", edit("1|\n", "7|\n")),
        ("text-row-without-a-bar", edit("1|\n", "1\n")),
        ("text-row-with-too-many-cells", edit("0|ab 12", "0|ab 12xyz")),
        ("unknown-backslash-escape", edit("0|ab 12", "0|ab\\q12")),
        ("surrogate-escape", edit("0|ab 12", "0|ab\\u{d800}")),
        ("escape-beyond-unicode", edit("0|ab 12", "0|ab\\u{110000}")),
        ("escape-without-digits", edit("0|ab 12", "0|ab\\u{}")),
        ("continuation-at-the-start-of-a-row", edit("0|ab 12", "0|\\.ab")),
        ("continuation-after-a-continuation", edit("0|ab 12", "0|a\\.\\.b")),
        ("append-at-the-start-of-a-row", edit("0|ab 12", "0|\\+{301}ab")),
        ("append-after-a-continuation", edit("0|ab 12", "0|a\\.\\+{301}")),
        ("overlapping-attr-runs", edit("0: 3-4 bold", "0: 3-4 bold\n0: 4-5 dim")),
        ("attr-run-outside-the-screen", edit("0: 3-4 bold", "0: 3-9 bold")),
        ("attr-run-without-tokens", edit("0: 3-4 bold", "0: 3-4")),
        ("attr-run-with-only-defaults", edit("0: 3-4 bold", "0: 3-4 fg=default")),
        ("attr-run-repeating-a-token", edit("0: 3-4 bold", "0: 3-4 bold bold")),
        ("attribute-outside-the-vocabulary", edit("0: 3-4 bold", "0: 3-4 blink")),
        ("comment-in-text", edit("1|\n", "# note\n1|\n")),
        ("comment-in-attrs", edit("[attrs]\n", "[attrs]\n# note\n")),
        ("region-outside-the-screen", edit("mask row=0 cols=3-4 text=/[0-9]+/", "mask row=9 text=ignore")),
        ("range-backwards", edit("cols=3-4", "cols=4-3")),
        ("mask-with-nothing-to-do", edit("mask row=0 cols=3-4 text=/[0-9]+/", "mask row=0")),
        ("mask-without-a-region", edit("mask row=0 cols=3-4 text=/[0-9]+/", "mask cols=0-1 text=ignore")),
        ("exact-text-on-several-rows", edit(r#"assert row=0 text="ab 12""#, r#"assert rows=0-1 text="x""#)),
        ("exact-text-with-find", edit(r#"assert row=0 text="ab 12""#, r#"assert find=/ab/ text="x""#)),
        ("assert-without-a-constraint", edit(r#"assert row=0 text="ab 12""#, "assert row=0")),
        ("regex-outside-the-dialect", edit("text=/[0-9]+/", r"text=/\bab/")),
        ("pattern-ended-by-an-unescaped-slash", edit("text=/[0-9]+/", "text=/a/b/")),
        ("unterminated-string", edit("title null\n", "title \"open\n")),
        ("invalid-string-escape", edit("title null\n", "title \"\\ud800\"\n")),
        ("repeated-token-in-a-mask", edit("mask row=0 cols=3-4", "mask row=0 row=1 cols=3-4")),
        ("ignore-modes-without-a-token", edit("semantics 1\n", "semantics 1\nignore modes\n")),
        ("mode-token-outside-the-vocabulary", edit("modes\n", "modes ?1004\n")),
        ("ignore-of-an-unknown-attribute", edit("semantics 1\n", "semantics 1\nignore attrs blink\n")),
        ("ignore-of-an-unknown-part", edit("semantics 1\n", "semantics 1\nignore colour\n")),
        ("ignore-title-with-an-argument", edit("semantics 1\n", "semantics 1\nignore title now\n")),
        ("cursor-outside-the-screen", edit("cursor row=0", "cursor row=5")),
        ("cursor-with-a-missing-field", edit(" hidden=false", "")),
        ("size-with-a-leading-zero", edit("size 6x2", "size 06x2")),
        ("size-too-large", edit("size 6x2", "size 6x1001")),
        ("wrapped-row-outside-the-screen", edit("title null\n", "title null\nwrapped 5\n")),
        ("shared-mask-outside-a-screen", edit("[screen 1]", "[masks]\nmask row=7 text=ignore\n\n[screen 1]")),
    ]
}

/// Canonical goldens: reading and writing them must give the same bytes.
pub fn roundtrips() -> Vec<(&'static str, String)> {
    let session: Vec<Scr> = vec![
        plain(&["welcome", "", "", ""]).at(3, 15, "12:01").named("startup"),
        plain(&["connected", "hello", "", ""]).at(3, 15, "12:02").title(Some("tf \"mud\"")).modes(&["?1", "DECKPAM"]),
        plain(&["done", "world", "", ""]).at(3, 15, "12:03").cursor(2, 3).wrapped(&[1]).named("end"),
    ];
    let composite: G = gold(&session)
        .file_ignore("title")
        .shared(&[
            "# The clock at the right end of the last row, on every screen.",
            r"mask row=3 cols=15-19 text=/\d\d:\d\d/",
        ])
        .masks(1, &["# These lines change with the build.", "mask row=1 text=ignore"])
        .asserts(2, &[r#"assert row=1 text="hello" !bold"#, "assert find=/hello/ fg=default"]);

    let text_cells = scr(12, &["", "x"])
        .cell(0, 0, "你")
        .cell(0, 1, "")
        .cell(0, 2, "e\u{301}")
        .cell(0, 3, "\u{e000}")
        .cell(0, 4, "\\")
        .cell(0, 5, "\u{1f468}\u{200d}")
        .cell(0, 6, "")
        .cell(0, 7, " \u{301}")
        .cell(0, 8, "|")
        .cell(0, 9, "\u{a0}")
        .bold(0, 0, 1)
        .style(0, 5, 6, |c| c.hyperlink = Some("https://example.org/a b".to_string()))
        .style(0, 3, 3, |c| {
            c.underline = vtee::UnderlineStyle::Dotted;
            c.underline_color = "idx:9".parse().unwrap();
            c.bg = "rgb:#0a0b0c".parse().unwrap();
        });

    let portable = gold_portable(&[plain(&["abc"]).modes(&["?2004"]).title(Some("t"))]);
    vec![
        ("simple", BASE.to_string()),
        ("composite-with-masks-asserts-comments-and-names", composite.text()),
        ("wide-combining-private-use-and-links", gold(&[text_cells]).text()),
        ("portable-profile", portable.text()),
    ]
}
