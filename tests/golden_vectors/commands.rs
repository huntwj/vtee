//! The definitions of the command-line vectors (`cli/`) and the `--update` vectors (`update/`) of design appendix A.
//! As with the run vectors, the files under `tests/fixtures/golden-vectors/` are the frozen data, these definitions
//! only generate them, and each says what the command must do so that regenerating cannot write down a wrong answer.

use vtee::Producer;

use super::defs::{G, Scr, gold, plain, run, timed_out_wait};

/// A command line that is refused (or fails) before it compares anything.
pub struct CliVector {
    pub dir: &'static str,
    /// The whole command line after `vtee`.
    pub args: Vec<&'static str>,
    /// Environment variables set for the run; `CI` and `VTEE_ALLOW_UPDATE` are removed first.
    pub env: Vec<(&'static str, &'static str)>,
    /// Files the command finds in its working directory.
    pub files: Vec<(&'static str, Vec<u8>)>,
    pub exit: i32,
    /// The normative first clause of the first line of standard error. Empty when there is no message.
    pub first_line: &'static str,
}

/// A valid golden of one empty 3x1 screen, which the vectors that need "a golden" use.
pub const GOLDEN: &str = "vtee-golden 1\nschema 1\nsemantics 1\n\n[screen 1]\nsize 3x1\ncursor row=0 col=0 shape=block hidden=false\nmodes\nalt_screen false\ntitle null\n\n[text]\n0|\n";

const SNAPSHOT_SCRIPT: &str = "{\"cmd\":\"snapshot\"}\n";

fn cli(dir: &'static str, args: &[&'static str], first_line: &'static str) -> CliVector {
    CliVector { dir, args: args.to_vec(), env: vec![], files: vec![], exit: 64, first_line }
}

impl CliVector {
    fn exit(mut self, exit: i32) -> Self {
        self.exit = exit;
        self
    }

    fn env(mut self, env: &[(&'static str, &'static str)]) -> Self {
        self.env = env.to_vec();
        self
    }

    fn file(mut self, name: &'static str, content: &str) -> Self {
        self.files.push((name, content.as_bytes().to_vec()));
        self
    }
}

pub fn cli_vectors() -> Vec<CliVector> {
    let all = vec![
        // The table of illegal combinations (design section 6.11), one vector per row.
        cli(
            "L01-expect-without-source",
            &["--expect", "g.golden"],
            "vtee: --expect needs a source of screens: a program after --, --connect SOCK or --snapshot FILE",
        ),
        cli(
            "L02-program-without-script",
            &["--expect", "g.golden", "--", "sh"],
            "vtee: a program needs --script FILE to say what to type; to look at it by hand use --connect",
        ),
        cli("L03-script-without-program", &["--script", "s.ndjson"], "vtee: --script needs a program to run or --connect SOCK"),
        cli(
            "L04-script-and-expect-without-program",
            &["--expect", "g.golden", "--script", "s.ndjson"],
            "vtee: --script needs a program to run or --connect SOCK",
        ),
        cli(
            "L05-connect-with-program",
            &["--connect", "x.sock", "--", "sh"],
            "vtee: --connect talks to a running session; it cannot be combined with a program",
        ),
        cli(
            "L06-snapshot-with-connect",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--connect", "x.sock"],
            "vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options",
        ),
        cli(
            "L07-snapshot-with-script",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--script", "x.ndjson"],
            "vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options",
        ),
        cli(
            "L08-snapshot-with-program",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--", "sh"],
            "vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options",
        ),
        cli(
            "L09-snapshot-with-cols",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--cols", "20"],
            "vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options",
        ),
        cli(
            "L10-snapshot-with-env",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--env", "A=b"],
            "vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options",
        ),
        cli(
            "L11-snapshot-with-log",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--log", "x.cast"],
            "vtee: --snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options",
        ),
        cli("L12-update-without-expect", &["--update"], "vtee: --update needs --expect GOLDEN"),
        cli("L13-screen-without-expect", &["--screen", "2"], "vtee: --screen needs --expect GOLDEN"),
        cli("L14-format-without-expect", &["--format", "json"], "vtee: --format needs --expect GOLDEN"),
        cli("L15-portable-without-expect", &["--portable"], "vtee: --portable needs --expect GOLDEN"),
        cli("L16-fail-fast-without-expect", &["--fail-fast"], "vtee: --fail-fast needs --expect GOLDEN"),
        cli("L17-max-mismatches-without-expect", &["--max-mismatches", "3"], "vtee: --max-mismatches needs --expect GOLDEN"),
        cli(
            "L18-text-with-expect",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--text"],
            "vtee: --text has no effect with --expect, which prints a report; use --format text or --format json",
        ),
        cli(
            "L19-migrate-with-script",
            &["--expect", "g.golden", "--update=migrate", "--script", "s.ndjson", "--", "sh"],
            "vtee: --update=migrate rewrites the file in place and runs nothing; give only --expect GOLDEN",
        ),
        cli(
            "L20-migrate-with-screen",
            &["--expect", "g.golden", "--update=migrate", "--screen", "1"],
            "vtee: --update=migrate rewrites the file in place and runs nothing; give only --expect GOLDEN",
        ),
        cli(
            "L21-migrate-is-not-available",
            &["--expect", "g.golden", "--update=migrate"],
            "vtee: --update=migrate is not available in this release",
        ),
        cli(
            "L22-portable-without-update",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--portable"],
            "vtee: --portable applies when a golden is created; use it with --update",
        ),
        cli(
            "L23-portable-with-restamp",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--update=restamp", "--portable"],
            "vtee: --portable applies when a golden is created; use it with --update",
        ),
        cli(
            "L24-screen-with-restamp",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--update=restamp", "--screen", "1"],
            "vtee: --screen applies to checking and to --update (record)",
        ),
        cli(
            "L25-unknown-update-mode",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--update=frobnicate"],
            "vtee: unknown --update mode 'frobnicate'; expected record, restamp or migrate",
        ),
        cli(
            "L26-screen-update-needs-an-existing-golden",
            &["--expect", "new.golden", "--snapshot", "s.ndjson", "--update", "--screen", "1"],
            "vtee: --screen 1 updates one screen of an existing golden; new.golden does not exist",
        ),
        // The guard on --update (OD18).
        cli(
            "L27-ci-refuses-update",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--update"],
            "vtee: --update is refused when CI is set (it would make a pipeline pass by rewriting its expectations); set VTEE_ALLOW_UPDATE=1 to allow it",
        )
        .env(&[("CI", "true")]),
        cli(
            "L28-ci-refuses-restamp-too",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--update=restamp"],
            "vtee: --update is refused when CI is set",
        )
        .env(&[("CI", "true")]),
        cli(
            "L29-ci-allowed-by-an-explicit-opt-in",
            &["--expect", "new.golden", "--snapshot", "missing.ndjson", "--update"],
            "vtee: failed to read missing.ndjson",
        )
        .env(&[("CI", "true"), ("VTEE_ALLOW_UPDATE", "1")])
        .exit(66),
        cli(
            "L30-an-empty-ci-is-not-ci",
            &["--expect", "new.golden", "--snapshot", "missing.ndjson", "--update"],
            "vtee: failed to read missing.ndjson",
        )
        .env(&[("CI", "")])
        .exit(66),
        cli(
            "L31-ci-never-guards-a-check",
            &["--expect", "none.golden", "--snapshot", "s.ndjson"],
            "vtee: none.golden: no such file (give --update to record it)",
        )
        .env(&[("CI", "true")])
        .exit(66),
        // Files and the program.
        cli(
            "L32-missing-golden",
            &["--expect", "none.golden", "--snapshot", "s.ndjson"],
            "vtee: none.golden: no such file (give --update to record it)",
        )
        .exit(66),
        cli("L33-malformed-golden", &["--expect", "bad.golden", "--snapshot", "s.ndjson"], "bad.golden: line 1:")
            .file("bad.golden", "hello\n")
            .exit(65),
        cli(
            "L34-missing-snapshot-file",
            &["--expect", "g.golden", "--snapshot", "missing.ndjson"],
            "vtee: failed to read missing.ndjson",
        )
        .file("g.golden", GOLDEN)
        .exit(66),
        cli(
            "L35-missing-script",
            &["--script", "nope.ndjson", "--expect", "g.golden", "--", "never-started"],
            "vtee: failed to open script nope.ndjson",
        )
        .file("g.golden", GOLDEN)
        .exit(66),
        cli(
            "L36-malformed-script-line",
            &["--script", "bad.ndjson", "--expect", "g.golden", "--", "never-started"],
            "script line 2: ",
        )
        .file("g.golden", GOLDEN)
        .file("bad.ndjson", "{\"cmd\":\"snapshot\"}\nnot json\n")
        .exit(65),
        cli(
            "L37-program-cannot-start",
            &["-qqq", "--script", "ok.ndjson", "--expect", "g.golden", "--", "never-started"],
            "vtee: failed to spawn never-started",
        )
        .file("g.golden", GOLDEN)
        .file("ok.ndjson", SNAPSHOT_SCRIPT)
        .exit(71),
        cli(
            "L38-session-unreachable",
            &["--connect", "none.sock", "--expect", "g.golden"],
            "vtee: failed to connect to none.sock",
        )
        .file("g.golden", GOLDEN)
        .exit(69),
        // Parser errors are 64, not clap's 2.
        cli("L39-unknown-option-exits-64", &["--frobnicate"], "error: unexpected argument '--frobnicate' found"),
        cli(
            "L40-bad-format-value",
            &["--expect", "g.golden", "--snapshot", "s.ndjson", "--format", "yaml"],
            "error: invalid value 'yaml' for '--format <FORMAT>'",
        ),
    ];
    all
}

// ---- --update --------------------------------------------------------------------------------------------------

/// An `--update` run on saved output: the golden before (if any), the run, the options, and what must happen.
pub struct UpdateVector {
    pub dir: &'static str,
    pub before: Option<Vec<u8>>,
    pub run: String,
    pub args: Vec<&'static str>,
    pub exit: i32,
    /// Strings that must appear in standard output.
    pub contains: &'static [&'static str],
}

fn producer(mut screen: Scr, version: &str) -> Scr {
    screen.snap.producer = Producer { vtee: version.to_string(), emulator: "alacritty_terminal 0.26.0".to_string() };
    screen
}

fn update(
    dir: &'static str,
    before: Option<&G>,
    run: String,
    args: &[&'static str],
    exit: i32,
    contains: &'static [&'static str],
) -> UpdateVector {
    UpdateVector { dir, before: before.map(|g| g.text().into_bytes()), run, args: args.to_vec(), exit, contains }
}

pub fn update_vectors() -> Vec<UpdateVector> {
    let mut all = Vec::new();

    let greeting = plain(&["hello", "world"]).named("greeting");
    all.push(update(
        "N01-create",
        None,
        run(std::slice::from_ref(&greeting)),
        &["--update"],
        0,
        &["CREATED golden.golden: 1 screen\n"],
    ));
    all.push(update(
        "N02-create-portable",
        None,
        run(std::slice::from_ref(&greeting)),
        &["--update", "--portable"],
        0,
        &["CREATED golden.golden: 1 screen\n"],
    ));

    // A golden with everything a person adds: ignore lines, a shared mask, a screen mask, an assertion, comments.
    let by_hand = gold(&[plain(&["clock 12:00", "status ok"])])
        .file_ignore("title")
        .shared(&["# any time of day", "mask row=0 cols=6-10 text=/\\d\\d:\\d\\d/"])
        .masks(1, &["# this row varies", "mask row=1 cols=7-8 text=ignore"])
        .asserts(1, &["# the greeting is there", "assert row=0 text=\"clock 12:00\""]);
    all.push(update(
        "N03-rerecord-keeps-what-was-written-by-hand",
        Some(&by_hand),
        run(&[plain(&["clock 13:45", "status ok"]).cursor(1, 0)]),
        &["--update"],
        0,
        &["UPDATED golden.golden: 1 screen\n  screen 1: 3 cells changed (text 3, attrs 0), cursor moved\n"],
    ));
    all.push(update(
        "N04-rerecord-as-json",
        Some(&by_hand),
        run(&[plain(&["clock 13:45", "status ok"]).cursor(1, 0)]),
        &["--update", "--format", "json"],
        0,
        &["\"result\":\"updated\"", "\"cells_changed\":3"],
    ));

    let old_producer = gold(&[producer(plain(&["same"]), "0.1.0")]);
    all.push(update(
        "N05-rerecord-that-changes-nothing-leaves-the-producer",
        Some(&old_producer),
        run(&[producer(plain(&["same"]), "0.9.0")]),
        &["--update"],
        0,
        &["UNCHANGED golden.golden: 1 screen\n"],
    ));

    all.push(update(
        "N06-rerecord-adds-a-screen",
        Some(&gold(&[plain(&["one"])])),
        run(&[plain(&["one"]), plain(&["two"])]),
        &["--update"],
        0,
        &["UPDATED golden.golden: 2 screens\n  screen 2: new (no masks, no asserts: add them by hand)\n"],
    ));

    let two = gold(&[plain(&["one"]), plain(&["two"])])
        .masks(2, &["mask row=0 text=ignore", "mask row=0 cols=0 attrs=ignore"])
        .asserts(2, &["assert row=0 text=\"two\""]);
    all.push(update(
        "N07-rerecord-drops-a-screen",
        Some(&two),
        run(&[plain(&["one"])]),
        &["--update"],
        0,
        &["UPDATED golden.golden: 1 screen\n  screen 2: dropped (it had 2 masks and 1 assert, which are lost)\n"],
    ));

    all.push(update(
        "N08-a-timed-out-wait-writes-nothing",
        Some(&gold(&[plain(&["one"])])),
        format!("{}{}", timed_out_wait(2, "ready", false), run(&[plain(&["changed"])])),
        &["--update"],
        1,
        &[
            "NOT WRITTEN golden.golden: a wait_for timed out",
            "wait       script line 2   wait_for \"ready\" timed out after 5000 ms",
        ],
    ));

    let three = gold(&[plain(&["a1"]), plain(&["b1"]), plain(&["c1"])]).masks(2, &["# keep", "mask row=0 text=ignore"]);
    all.push(update(
        "N09-one-screen-is-rewritten-alone",
        Some(&three),
        run(&[plain(&["b2"])]),
        &["--update", "--screen", "2"],
        0,
        &["UPDATED golden.golden: 3 screens\n  screen 2: 1 cell changed (text 1, attrs 0)\n"],
    ));

    // Restamp: the golden's semantics number is `unknown`, which a restamp always replaces.
    let unknown = gold(&[plain(&["steady"])]).replace("semantics 1\n", "semantics unknown\n");
    all.push(update(
        "N10-restamp-succeeds-when-nothing-differs",
        Some(&unknown),
        run(&[plain(&["steady"])]),
        &["--update=restamp"],
        0,
        &["RESTAMPED golden.golden: semantics unknown -> 1\n"],
    ));
    all.push(update(
        "N11-restamp-refuses-when-something-differs",
        Some(&unknown),
        run(&[plain(&["moved!"])]),
        &["--update=restamp"],
        1,
        &["NOT RESTAMPED golden.golden: 2 differences that would change what the golden checks"],
    ));
    all.push(update(
        "N12-restamp-has-nothing-to-do-when-current",
        Some(&gold(&[plain(&["steady"])])),
        run(&[plain(&["steady"])]),
        &["--update=restamp"],
        0,
        &["UNCHANGED golden.golden: semantics 1 is already the number of this vtee\n"],
    ));
    let old_stamp = gold(&[plain(&["steady"])]).replace("semantics 1\n", "semantics 7\n");
    all.push(update(
        "N13-rerecord-restates-the-stamp",
        Some(&old_stamp),
        run(&[plain(&["steady"])]),
        &["--update"],
        0,
        &["UPDATED golden.golden: 1 screen\n  stamp: schema 1 semantics 7 -> schema 1 semantics 1\n"],
    ));

    all.push(UpdateVector {
        dir: "N14-a-draft-is-replaced",
        before: Some(b"vtee-golden 0\nwhatever the draft said\n".to_vec()),
        run: run(&[plain(&["fresh"])]),
        args: vec!["--update"],
        exit: 0,
        contains: &["CREATED golden.golden: 1 screen\n  note: replaced a vtee-golden 0 draft"],
    });
    let canonical = gold(&[plain(&["steady"])]).text();
    all.push(UpdateVector {
        dir: "N15-a-file-is-normalised-to-canonical-form",
        before: Some(canonical.replace('\n', "\r\n").into_bytes()),
        run: run(&[plain(&["steady"])]),
        args: vec!["--update"],
        exit: 0,
        contains: &["UNCHANGED golden.golden: 1 screen\n  note: rewrote the file in canonical form"],
    });
    all.push(update(
        "N16-portable-does-not-touch-an-existing-golden",
        Some(&gold(&[plain(&["steady"])])),
        run(&[plain(&["steady"])]),
        &["--update", "--portable"],
        0,
        &["UNCHANGED golden.golden: 1 screen\n  note: --portable only applies when a golden is created"],
    ));
    all.push(UpdateVector {
        dir: "N17-a-golden-of-a-newer-grammar-is-never-rewritten",
        before: Some(gold(&[plain(&["steady"])]).text().replacen("vtee-golden 1", "vtee-golden 2", 1).into_bytes()),
        run: run(&[plain(&["steady"])]),
        args: vec!["--update"],
        exit: 2,
        contains: &["REFUSED golden.golden: grammar 2 in the golden, 1 in this vtee"],
    });
    all
}
