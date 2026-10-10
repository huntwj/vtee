//! `--expect GOLDEN`: run a script (or read saved output) and compare the screens it produced with a golden file
//! (design section 6.11). The options, the table of illegal combinations and the exit codes are the design's; the
//! comparison and the reports are `vtee::golden`'s.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vtee::golden::{
    Build, CheckError, CheckOptions, CompareOptions, DEFAULT_MAX_MISMATCHES, Golden, Invocation, LoadError,
    RecordOptions, Refusal, Report, Run, RunError, Semantics, Verdict, check_saved, describe_wait, display_name,
    plan_record, plan_screen, read_run_for_build, refuse_golden, refuse_unread, restamped,
};

use crate::output::{exit_code, fail, print_error};
use crate::script::{self, Failure, Flow, ReadError, Reply, Script, ScriptLine, Sink, SocketDriver, TapDriver};

/// `--format`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
}

/// What the command line said about `--expect`, before any file is looked at.
#[derive(Default)]
pub struct Args {
    pub expect: Option<PathBuf>,
    /// The text after `--update=`; `record` for a bare `--update`.
    pub update: Option<String>,
    pub screen: Option<usize>,
    pub format: Option<Format>,
    pub portable: bool,
    pub fail_fast: bool,
    pub max_mismatches: Option<usize>,
    pub snapshot: Option<PathBuf>,
    pub text: bool,
    /// Which of `--connect`, `--script` and a program were given.
    pub connect: bool,
    pub script: bool,
    pub program: bool,
    /// Whether any of `--cols`, `--rows`, `--env` and `--log` was given.
    pub session_options: bool,
}

/// How `--update` was asked to rewrite the golden.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// No `--update`: compare, write nothing.
    Check,
    /// `--update` and `--update=record`.
    Record,
    /// `--update=restamp`.
    Restamp,
}

/// Where the screens of a run come from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    /// A program after `--`, driven by `--script`.
    Program,
    /// `--connect` with `--script`: the script against a live session.
    ConnectScript,
    /// `--connect` alone: the session's screen now.
    ConnectNow,
    /// `--snapshot FILE`, or `-` for standard input: finished output.
    Snapshot(PathBuf),
}

/// A valid `--expect` request.
#[derive(Debug)]
pub struct Plan {
    pub golden: PathBuf,
    pub mode: Mode,
    pub source: Source,
    pub screen: Option<usize>,
    pub format: Format,
    pub portable: bool,
    pub fail_fast: bool,
    pub max_mismatches: usize,
}

/// Decides whether the command line is a legal `--expect` request, with the messages of design section 6.11. `None`
/// means `--expect` was not given. The first clause of each message is normative. `env` reads the environment (`CI`
/// and `VTEE_ALLOW_UPDATE`).
pub fn plan(args: &Args, env: &dyn Fn(&str) -> Option<String>) -> Result<Option<Plan>, String> {
    // Rules that hold with or without `--expect`.
    // (`--snapshot` has its own, which comes first when it is in the mix.)
    if args.snapshot.is_none() {
        if args.connect && args.program {
            return Err("--connect talks to a running session; it cannot be combined with a program".to_string());
        }
        if args.script && !args.connect && !args.program {
            return Err("--script needs a program to run or --connect SOCK".to_string());
        }
    }

    let mode = match args.update.as_deref() {
        None => None,
        Some("record") => Some(Mode::Record),
        Some("restamp") => Some(Mode::Restamp),
        Some("migrate") => None,
        Some(other) => {
            return Err(format!("unknown --update mode '{other}'; expected record, restamp or migrate"));
        }
    };
    let migrate = args.update.as_deref() == Some("migrate");

    let Some(golden) = &args.expect else {
        let needs = [
            (args.update.is_some(), "--update"),
            (args.screen.is_some(), "--screen"),
            (args.format.is_some(), "--format"),
            (args.portable, "--portable"),
            (args.fail_fast, "--fail-fast"),
            (args.max_mismatches.is_some(), "--max-mismatches"),
            (args.snapshot.is_some(), "--snapshot"),
        ];
        return match needs.iter().find(|(given, _)| *given) {
            Some((_, option)) => Err(format!("{option} needs --expect GOLDEN")),
            None => Ok(None),
        };
    };

    if args.text {
        return Err(
            "--text has no effect with --expect, which prints a report; use --format text or --format json".to_string()
        );
    }
    if migrate {
        let others = args.program
            || args.script
            || args.connect
            || args.snapshot.is_some()
            || args.screen.is_some()
            || args.portable
            || args.fail_fast
            || args.max_mismatches.is_some();
        return Err(if others {
            "--update=migrate rewrites the file in place and runs nothing; give only --expect GOLDEN".to_string()
        } else {
            "--update=migrate is not available in this release".to_string()
        });
    }
    if args.snapshot.is_some() {
        if args.connect || args.script || args.program || args.session_options {
            return Err("--snapshot reads finished output; it cannot be combined with --connect, --script, a program or session options".to_string());
        }
    } else if !args.connect && !args.program {
        return Err(
            "--expect needs a source of screens: a program after --, --connect SOCK or --snapshot FILE".to_string()
        );
    } else if args.program && !args.script {
        return Err(
            "a program needs --script FILE to say what to type; to look at it by hand use --connect".to_string()
        );
    }
    if args.portable && mode != Some(Mode::Record) {
        return Err("--portable applies when a golden is created; use it with --update".to_string());
    }
    if args.screen.is_some() && mode == Some(Mode::Restamp) {
        return Err("--screen applies to checking and to --update (record)".to_string());
    }
    if mode.is_some() {
        let set = |name: &str| env(name).is_some_and(|v| !v.is_empty());
        if set("CI") && env("VTEE_ALLOW_UPDATE").as_deref() != Some("1") {
            return Err("--update is refused when CI is set (it would make a pipeline pass by rewriting its expectations); set VTEE_ALLOW_UPDATE=1 to allow it".to_string());
        }
    }

    let source = match &args.snapshot {
        Some(path) => Source::Snapshot(path.clone()),
        None if args.program => Source::Program,
        None if args.script => Source::ConnectScript,
        None => Source::ConnectNow,
    };
    Ok(Some(Plan {
        golden: golden.clone(),
        mode: mode.unwrap_or(Mode::Check),
        source,
        screen: args.screen,
        format: args.format.unwrap_or(Format::Text),
        portable: args.portable,
        fail_fast: args.fail_fast,
        max_mismatches: args.max_mismatches.unwrap_or(DEFAULT_MAX_MISMATCHES),
    }))
}

/// What the run needs from the command line beyond the plan.
pub struct Context<'a> {
    pub program: Option<(&'a str, &'a [String])>,
    pub script: Option<&'a Path>,
    pub connect: Option<&'a Path>,
    pub log_path: Option<&'a Path>,
    pub cols: u16,
    pub rows: u16,
    pub env: &'a [(String, String)],
    /// The command line as typed, without the program name.
    pub argv: Vec<String>,
}

/// A line for standard error in the shape of a data error: no `vtee: ` prefix, because its first words say what is
/// wrong with which file (design section 6.10).
fn data_error(code: exitcode::ExitCode, message: impl AsRef<str>) -> ExitCode {
    use owo_colors::{OwoColorize, Stream::Stderr};
    eprintln!("{}", message.as_ref().if_supports_color(Stderr, |t| t.red().to_string()));
    exit_code(code)
}

fn failure(failure: Failure) -> ExitCode {
    fail(failure.code, failure.message)
}

/// Quotes one word of a command line for the commands a refusal suggests.
fn quote(word: &str) -> String {
    let plain = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        word.to_string()
    } else if cfg!(windows) {
        format!("\"{}\"", word.replace('"', "\\\""))
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// The command line as `Invocation` wants it: the options, then (kept last) the program.
fn invocation(plan: &Plan, ctx: &Context) -> Invocation {
    let program_words = ctx.program.map_or(0, |(_, rest)| rest.len() + 1);
    let split = ctx.argv.len().saturating_sub(program_words);
    let mut head: Vec<&String> = ctx.argv[..split].iter().collect();
    if program_words > 0 && head.last().is_some_and(|w| w.as_str() == "--") {
        head.pop();
    }
    let mut command = String::from("vtee");
    for word in head {
        command.push(' ');
        command.push_str(&quote(word));
    }
    let tail = if program_words > 0 {
        let words: Vec<String> = ctx.argv[split..].iter().map(|w| quote(w)).collect();
        format!("-- {}", words.join(" "))
    } else {
        String::new()
    };
    Invocation { golden: plan.golden.display().to_string(), command, tail }
}

/// The saved output of a run: the replies of its `snapshot` and `wait_for` commands, one JSON document per line.
struct Collect {
    saved: String,
    fail_fast: bool,
}

impl Sink for Collect {
    fn reply(&mut self, line: &ScriptLine, reply: Reply) -> Flow {
        self.saved.push_str(&script::saved_line(line, &reply));
        self.saved.push('\n');
        if self.fail_fast && script::timed_out_wait(line, &reply).is_some() { Flow::Stop } else { Flow::Continue }
    }
}

/// Reads a script for a run with `--expect`: all of it, before anything is started.
fn read_whole_script(path: &Path) -> Result<Script, ExitCode> {
    let script = match script::read_script(path) {
        Ok(script) => script,
        Err(e) => return Err(fail(exitcode::NOINPUT, format!("failed to open script {}: {e}", path.display()))),
    };
    match &script.error {
        None => Ok(script),
        Some(ReadError::Bad { line, message }) => {
            Err(data_error(exitcode::DATAERR, format!("script line {line}: {message}")))
        }
        Some(ReadError::Io { line, source }) => {
            Err(data_error(exitcode::IOERR, format!("script line {line}: could not be read: {source}")))
        }
    }
}

/// Runs the source of the plan and returns the saved output of the run.
fn collect(plan: &Plan, ctx: &Context) -> Result<String, ExitCode> {
    let mut sink = Collect { saved: String::new(), fail_fast: plan.fail_fast };
    match &plan.source {
        Source::Snapshot(path) => return read_snapshot_input(path),
        Source::Program => {
            let (program, rest) = ctx.program.expect("a program source has a program");
            let script = read_whole_script(ctx.script.expect("a program source has a script"))?;
            let spawn_opts = vtee::SpawnOptions {
                cols: ctx.cols,
                rows: ctx.rows,
                log_path: ctx.log_path,
                forward_input: false,
                forward_output: false,
                env: ctx.env,
            };
            let tap = match vtee::Tap::spawn(program, rest, spawn_opts) {
                Ok(t) => t,
                Err(e) => return Err(fail(exitcode::OSERR, format!("failed to spawn {program}: {e}"))),
            };
            tap.watch_sigterm();
            let outcome = script::drive(&mut TapDriver(&tap), &script.lines, &mut sink);
            tap.kill();
            let _ = tap.wait();
            outcome.map_err(failure)?;
        }
        Source::ConnectScript => {
            let script = read_whole_script(ctx.script.expect("a connect-script source has a script"))?;
            let mut driver =
                SocketDriver::connect(ctx.connect.expect("a connect source has a socket")).map_err(failure)?;
            script::drive(&mut driver, &script.lines, &mut sink).map_err(failure)?;
        }
        Source::ConnectNow => {
            let mut driver =
                SocketDriver::connect(ctx.connect.expect("a connect source has a socket")).map_err(failure)?;
            let now = ScriptLine {
                number: 1,
                raw: r#"{"cmd":"snapshot"}"#.to_string(),
                command: vtee::Command::Snapshot,
                name: None,
                optional: false,
            };
            script::drive(&mut driver, &[now], &mut sink).map_err(failure)?;
        }
    }
    Ok(sink.saved)
}

/// Finished output: a file, or standard input for `-`.
fn read_snapshot_input(path: &Path) -> Result<String, ExitCode> {
    let mut bytes = Vec::new();
    let read = if path.as_os_str() == "-" {
        std::io::stdin().lock().read_to_end(&mut bytes).map(|_| ())
    } else {
        std::fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)).map(|_| ())
    };
    if let Err(e) = read {
        let code = if path.as_os_str() != "-" && e.kind() == std::io::ErrorKind::NotFound {
            exitcode::NOINPUT
        } else {
            exitcode::IOERR
        };
        return Err(fail(code, format!("failed to read {}: {e}", path.display())));
    }
    String::from_utf8(bytes).map_err(|_| data_error(exitcode::DATAERR, "snapshot: the input is not valid UTF-8"))
}

/// What reading the golden gave.
enum Loaded {
    Golden(Box<Golden>),
    /// The report is already printed; this is its exit code.
    Done(ExitCode),
}

fn print_report(report: &Report, format: Format) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = match format {
        Format::Text => write!(out, "{}", report.to_text()),
        Format::Json => writeln!(out, "{}", report.to_json()),
    };
    let _ = out.flush();
}

fn check_options(plan: &Plan) -> CheckOptions {
    CheckOptions {
        compare: CompareOptions { screen: plan.screen, fail_fast: plan.fail_fast },
        max_mismatches: plan.max_mismatches,
    }
}

/// Reads and checks the header of the golden: a missing file is 66, a malformed one 65, and one this build will not
/// compare is a refusal (2) decided before anything is run (design section 6.8, steps 1 and 2).
fn load_golden(plan: &Plan, inv: &Invocation) -> Loaded {
    let path = &plan.golden;
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded::Done(fail(
                exitcode::NOINPUT,
                format!("{}: no such file (give --update to record it)", path.display()),
            ));
        }
        Err(e) => return Loaded::Done(fail(exitcode::NOINPUT, format!("{}: {e}", path.display()))),
    };
    let options = check_options(plan);
    let golden = match Golden::parse(&bytes) {
        Ok(golden) => golden,
        Err(LoadError::Malformed(e)) => {
            return Loaded::Done(data_error(exitcode::DATAERR, format!("{}: {e}", path.display())));
        }
        Err(LoadError::Refused(refusal)) => {
            let report = refuse_unread(inv, refusal, &options);
            print_report(&report, plan.format);
            return Loaded::Done(exit_code(report.exit_code()));
        }
    };
    if let Err(refusal) = golden.check_build(Build::current()) {
        let report = refuse_golden(&golden, inv, refusal, &options);
        print_report(&report, plan.format);
        return Loaded::Done(exit_code(report.exit_code()));
    }
    Loaded::Golden(Box::new(golden))
}

/// Runs a valid request.
pub fn run(plan: &Plan, ctx: &Context) -> ExitCode {
    let inv = invocation(plan, ctx);
    match plan.mode {
        Mode::Check => run_check(plan, ctx, &inv),
        Mode::Record => run_record(plan, ctx, &inv),
        Mode::Restamp => run_restamp(plan, ctx, &inv),
    }
}

fn run_check(plan: &Plan, ctx: &Context, inv: &Invocation) -> ExitCode {
    let golden = match load_golden(plan, inv) {
        Loaded::Golden(golden) => golden,
        Loaded::Done(code) => return code,
    };
    let saved = match collect(plan, ctx) {
        Ok(saved) => saved,
        Err(code) => return code,
    };
    match check_saved(&golden, inv, &saved, &check_options(plan)) {
        Ok(report) => {
            print_report(&report, plan.format);
            exit_code(report.exit_code())
        }
        Err(CheckError::Run(message)) => data_error(exitcode::DATAERR, message),
        Err(CheckError::Usage(usage)) => fail(exitcode::USAGE, usage.to_string()),
    }
}

// ---- --update ----------------------------------------------------------------------------------------------------

/// The report name of the golden: its file name.
fn name_of(plan: &Plan) -> String {
    display_name(&plan.golden.display().to_string())
}

fn print_line(plan: &Plan, text: &str, json: &str) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = match plan.format {
        Format::Text => write!(out, "{text}"),
        Format::Json => writeln!(out, "{json}"),
    };
    let _ = out.flush();
}

/// A JSON string literal.
fn json_str(text: &str) -> String {
    vtee::ndjson::to_line(text).expect("a string serializes")
}

/// Replaces the golden's file with `text` in one step, so a crash cannot leave half a golden.
fn write_golden(path: &Path, text: &str) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".vtee-new");
    let temp = PathBuf::from(temp);
    let written = std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

fn cannot_write(path: &Path, e: std::io::Error) -> ExitCode {
    fail(exitcode::IOERR, format!("cannot write {}: {e}", path.display()))
}

/// The `vtee-golden 0` draft is the one malformed golden that `--update` replaces: the message that rejects it says to
/// record it again.
fn is_draft(bytes: &[u8]) -> bool {
    bytes.split(|b| *b == b'\n').next().is_some_and(|first| first.trim_ascii_end() == b"vtee-golden 0")
}

/// The golden that `--update` starts from.
enum Existing {
    /// There is no file, or it is the unsupported `vtee-golden 0` draft (`true`), which is replaced.
    Fresh {
        replaces_draft: bool,
    },
    Golden(Box<Golden>, Vec<u8>),
}

fn load_existing(plan: &Plan, inv: &Invocation) -> Result<Existing, ExitCode> {
    let path = &plan.golden;
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if let Some(n) = plan.screen {
                return Err(fail(
                    exitcode::USAGE,
                    format!("--screen {n} updates one screen of an existing golden; {} does not exist", path.display()),
                ));
            }
            return Ok(Existing::Fresh { replaces_draft: false });
        }
        Err(e) => return Err(fail(exitcode::NOINPUT, format!("{}: {e}", path.display()))),
    };
    match Golden::parse(&bytes) {
        Ok(golden) => Ok(Existing::Golden(Box::new(golden), bytes)),
        Err(LoadError::Malformed(_)) if is_draft(&bytes) && plan.screen.is_none() => {
            Ok(Existing::Fresh { replaces_draft: true })
        }
        Err(LoadError::Malformed(e)) => {
            let code = data_error(exitcode::DATAERR, format!("{}: {e}", path.display()));
            print_error(&format!(
                "{} cannot be updated while it is malformed; fix it or delete it first",
                path.display()
            ));
            Err(code)
        }
        Err(LoadError::Refused(refusal)) => {
            let report = refuse_unread(inv, refusal, &check_options(plan));
            print_report(&report, plan.format);
            Err(exit_code(report.exit_code()))
        }
    }
}

/// The run of an update, with its snapshots required to be this build's own: a golden made from other numbers could
/// not be read back by this build (exit 2, nothing written).
fn read_run_for_update(plan: &Plan, saved: &str) -> Result<Run, ExitCode> {
    match read_run_for_build(saved, Build::current()) {
        Ok(run) => Ok(run),
        Err(RunError::Malformed(message)) => Err(data_error(exitcode::DATAERR, message)),
        Err(RunError::Refused(refusal)) => {
            let build = Build::current();
            let what = match refusal {
                Refusal::SnapshotSchema { snapshot, .. } => {
                    format!("schema {snapshot}, this vtee has schema {}", build.schema)
                }
                Refusal::SnapshotSemantics { snapshot, .. } => {
                    format!("semantics {snapshot}, this vtee has semantics {}", build.semantics)
                }
                _ => refusal.reason(),
            };
            Err(fail(
                2,
                format!(
                    "the snapshots carry {what}; {} was not written. Record with the vtee that produced them",
                    plan.golden.display()
                ),
            ))
        }
    }
}

/// `--update` found the run unfit to record: nothing is written (design section 6.11).
fn not_written(plan: &Plan, run: &Run) -> ExitCode {
    let name = name_of(plan);
    let reason = if run.problems.is_empty() { "the run produced no screens" } else { "a wait_for timed out" };
    let mut text = format!("NOT WRITTEN {name}: {reason}; the golden was left as it was\n");
    for problem in &run.problems {
        text.push_str(&format!("  wait       {}\n", describe_wait(problem)));
    }
    let problems: Vec<String> = run
        .problems
        .iter()
        .map(|p| {
            format!(
                "{{\"kind\":\"wait\",\"line\":{},\"text\":{},\"timeout_ms\":{}}}",
                p.line,
                p.text.as_deref().map_or("null".to_string(), json_str),
                p.timeout_ms.map_or("null".to_string(), |ms| ms.to_string())
            )
        })
        .collect();
    let json = format!(
        "{{\"golden\":{},\"result\":\"not_written\",\"reason\":{},\"run\":[{}]}}",
        json_str(&name),
        json_str(reason),
        problems.join(",")
    );
    print_line(plan, &text, &json);
    exit_code(1)
}

fn run_record(plan: &Plan, ctx: &Context, inv: &Invocation) -> ExitCode {
    let existing = match load_existing(plan, inv) {
        Ok(existing) => existing,
        Err(code) => return code,
    };
    let saved = match collect(plan, ctx) {
        Ok(saved) => saved,
        Err(code) => return code,
    };
    let run = match read_run_for_update(plan, &saved) {
        Ok(run) => run,
        Err(code) => return code,
    };
    if !run.problems.is_empty() || run.screens.is_empty() {
        return not_written(plan, &run);
    }

    let (old, old_bytes) = match &existing {
        Existing::Golden(golden, bytes) => (Some(golden.as_ref()), Some(bytes.as_slice())),
        Existing::Fresh { .. } => (None, None),
    };
    let mut update = match (plan.screen, old) {
        (Some(n), Some(golden)) => {
            if run.screens.len() != 1 {
                return fail(
                    exitcode::USAGE,
                    format!("--screen {n} selects one screen, but the run produced {}", run.screens.len()),
                );
            }
            if n == 0 || n > golden.screens.len() {
                return fail(
                    exitcode::USAGE,
                    format!("--screen {n} selects one screen, but the golden has {} screens", golden.screens.len()),
                );
            }
            let snapshot = &run.screens[0];
            if golden.schema != snapshot.schema_version
                || golden.semantics != Semantics::Number(snapshot.semantics_version)
            {
                return fail(
                    2,
                    format!(
                        "{} was recorded under schema {} semantics {} and the run carries schema {} semantics {}; \
                         one screen cannot be re-recorded into it. Re-record every screen with --update",
                        plan.golden.display(),
                        golden.schema,
                        golden.semantics,
                        snapshot.schema_version,
                        snapshot.semantics_version
                    ),
                );
            }
            plan_screen(golden, n, snapshot, run.names.first().cloned().flatten())
        }
        _ => plan_record(old, &run, RecordOptions { portable: plan.portable }),
    };
    if let Existing::Fresh { replaces_draft: true } = existing {
        update.notes.push(
            "replaced a vtee-golden 0 draft; the masks and assertions it may have had are not carried over".to_string(),
        );
    }

    let text = update.golden.to_text();
    let canonical_on_disk = old_bytes == Some(text.as_bytes());
    if update.unchanged && !canonical_on_disk {
        update.notes.push("rewrote the file in canonical form (nothing it says changed)".to_string());
    }
    if !canonical_on_disk && let Err(e) = write_golden(&plan.golden, &text) {
        return cannot_write(&plan.golden, e);
    }
    let name = name_of(plan);
    print_line(plan, &update.to_text(&name), &update.to_json(&name));
    ExitCode::SUCCESS
}

fn run_restamp(plan: &Plan, ctx: &Context, inv: &Invocation) -> ExitCode {
    let build = Build::current();
    let name = name_of(plan);
    let bytes = match std::fs::read(&plan.golden) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return fail(exitcode::NOINPUT, format!("{}: no such file", plan.golden.display()));
        }
        Err(e) => return fail(exitcode::NOINPUT, format!("{}: {e}", plan.golden.display())),
    };
    let golden = match Golden::parse(&bytes) {
        Ok(golden) => golden,
        Err(LoadError::Malformed(e)) => {
            return data_error(exitcode::DATAERR, format!("{}: {e}", plan.golden.display()));
        }
        Err(LoadError::Refused(refusal)) => {
            let report = refuse_unread(inv, refusal, &check_options(plan));
            print_report(&report, plan.format);
            return exit_code(report.exit_code());
        }
    };
    // A different layout needs a migration or a re-record, not a new stamp.
    if golden.schema != build.schema {
        let refusal = Refusal::Schema { golden: golden.schema, tool: build.schema };
        let report = refuse_golden(&golden, inv, refusal, &check_options(plan));
        print_report(&report, plan.format);
        return exit_code(report.exit_code());
    }
    if golden.semantics == Semantics::Number(build.semantics) {
        let text = format!("UNCHANGED {name}: semantics {} is already the number of this vtee\n", build.semantics);
        let json =
            format!("{{\"golden\":{},\"result\":\"unchanged\",\"semantics\":{}}}", json_str(&name), build.semantics);
        print_line(plan, &text, &json);
        return ExitCode::SUCCESS;
    }

    // The check, with the version numbers set aside: does the golden still say what a run produces?
    let probe = Golden { semantics: Semantics::Number(build.semantics), ..golden.clone() };
    let saved = match collect(plan, ctx) {
        Ok(saved) => saved,
        Err(code) => return code,
    };
    let report = match check_saved(&probe, inv, &saved, &check_options(plan)) {
        Ok(report) => report,
        Err(CheckError::Run(message)) => return data_error(exitcode::DATAERR, message),
        Err(CheckError::Usage(usage)) => return fail(exitcode::USAGE, usage.to_string()),
    };
    match report.verdict {
        Verdict::Refused => {
            print_report(&report, plan.format);
            exit_code(report.exit_code())
        }
        Verdict::Fail => {
            let differences = report.comparison.as_ref().map_or(0, |c| c.total());
            let text = format!(
                "NOT RESTAMPED {name}: {} that would change what the golden checks; it was left as it was\n{}",
                if differences == 1 { "1 difference".to_string() } else { format!("{differences} differences") },
                report.to_text()
            );
            let json = format!(
                "{{\"golden\":{},\"result\":\"not_restamped\",\"differences\":{differences},\"report\":{}}}",
                json_str(&name),
                report.to_json()
            );
            print_line(plan, &text, &json);
            exit_code(1)
        }
        Verdict::Pass => {
            let first = match read_run_for_update(plan, &saved) {
                Ok(run) => run.screens.into_iter().next(),
                Err(code) => return code,
            };
            let out = restamped(&golden, build.semantics, first.as_ref());
            if let Err(e) = write_golden(&plan.golden, &out.to_text()) {
                return cannot_write(&plan.golden, e);
            }
            let text = format!("RESTAMPED {name}: semantics {} -> {}\n", golden.semantics, build.semantics);
            let from = match golden.semantics {
                Semantics::Number(n) => n.to_string(),
                Semantics::Unknown => "\"unknown\"".to_string(),
            };
            let json = format!(
                "{{\"golden\":{},\"result\":\"restamped\",\"from\":{from},\"to\":{}}}",
                json_str(&name),
                build.semantics
            );
            print_line(plan, &text, &json);
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn base() -> Args {
        Args { expect: Some("g.golden".into()), program: true, script: true, ..Args::default() }
    }

    fn message(args: &Args) -> String {
        plan(args, &no_env).expect_err("this combination is illegal")
    }

    #[test]
    fn no_expect_means_no_plan_and_the_expect_only_options_need_it() {
        assert!(plan(&Args::default(), &no_env).unwrap().is_none());
        let cases: [(Args, &str); 7] = [
            (Args { update: Some("record".into()), ..Args::default() }, "--update"),
            (Args { screen: Some(1), ..Args::default() }, "--screen"),
            (Args { format: Some(Format::Json), ..Args::default() }, "--format"),
            (Args { portable: true, ..Args::default() }, "--portable"),
            (Args { fail_fast: true, ..Args::default() }, "--fail-fast"),
            (Args { max_mismatches: Some(3), ..Args::default() }, "--max-mismatches"),
            (Args { snapshot: Some("s".into()), ..Args::default() }, "--snapshot"),
        ];
        for (args, option) in cases {
            assert_eq!(message(&args), format!("{option} needs --expect GOLDEN"));
        }
    }

    #[test]
    fn each_source_is_recognised() {
        assert_eq!(plan(&base(), &no_env).unwrap().unwrap().source, Source::Program);
        let connect_script = Args { program: false, connect: true, ..base() };
        assert_eq!(plan(&connect_script, &no_env).unwrap().unwrap().source, Source::ConnectScript);
        let connect_now = Args { program: false, connect: true, script: false, ..base() };
        assert_eq!(plan(&connect_now, &no_env).unwrap().unwrap().source, Source::ConnectNow);
        let snapshot = Args { program: false, script: false, snapshot: Some("-".into()), ..base() };
        assert_eq!(plan(&snapshot, &no_env).unwrap().unwrap().source, Source::Snapshot("-".into()));
    }

    #[test]
    fn the_illegal_combinations_have_the_designs_messages() {
        let none = Args { program: false, script: false, ..base() };
        assert_eq!(
            message(&none),
            "--expect needs a source of screens: a program after --, --connect SOCK or --snapshot FILE"
        );
        let no_script = Args { script: false, ..base() };
        assert_eq!(
            message(&no_script),
            "a program needs --script FILE to say what to type; to look at it by hand use --connect"
        );
        let lone_script = Args { program: false, ..base() };
        assert_eq!(message(&lone_script), "--script needs a program to run or --connect SOCK");
        let connect_program = Args { connect: true, ..base() };
        assert_eq!(
            message(&connect_program),
            "--connect talks to a running session; it cannot be combined with a program"
        );
        for other in [
            Args { connect: true, program: false, ..snapshot_args() },
            Args { script: true, ..snapshot_args() },
            Args { program: true, ..snapshot_args() },
            Args { session_options: true, ..snapshot_args() },
        ] {
            assert!(message(&other).starts_with("--snapshot reads finished output; it cannot be combined with"));
        }
        assert_eq!(
            message(&Args { text: true, ..base() }),
            "--text has no effect with --expect, which prints a report; use --format text or --format json"
        );
        assert_eq!(
            message(&Args { portable: true, ..base() }),
            "--portable applies when a golden is created; use it with --update"
        );
    }

    fn snapshot_args() -> Args {
        Args { expect: Some("g.golden".into()), snapshot: Some("s.ndjson".into()), ..Args::default() }
    }

    #[test]
    fn update_modes_are_parsed_and_migrate_is_not_available() {
        assert_eq!(
            message(&Args { update: Some("frobnicate".into()), ..base() }),
            "unknown --update mode 'frobnicate'; expected record, restamp or migrate"
        );
        let migrate = |args: Args| message(&Args { update: Some("migrate".into()), ..args });
        let alone = Args { expect: Some("g.golden".into()), ..Args::default() };
        assert_eq!(migrate(alone), "--update=migrate is not available in this release");
        assert_eq!(
            migrate(base()),
            "--update=migrate rewrites the file in place and runs nothing; give only --expect GOLDEN"
        );
        assert!(
            migrate(Args { expect: Some("g.golden".into()), screen: Some(2), ..Args::default() })
                .starts_with("--update=migrate rewrites the file in place")
        );
    }

    #[test]
    fn update_is_refused_when_ci_is_set_unless_allowed() {
        let update = Args { update: Some("record".into()), ..base() };
        let ci = |name: &str| (name == "CI").then(|| "true".to_string());
        assert!(plan(&update, &ci).unwrap_err().starts_with("--update is refused when CI is set"));
        let allowed = |name: &str| match name {
            "CI" => Some("true".to_string()),
            "VTEE_ALLOW_UPDATE" => Some("1".to_string()),
            _ => None,
        };
        assert_eq!(plan(&update, &allowed).unwrap().unwrap().mode, Mode::Record);
        let empty = |name: &str| (name == "CI").then(String::new);
        assert_eq!(plan(&update, &empty).unwrap().unwrap().mode, Mode::Record);
        // A check is never guarded.
        assert!(plan(&base(), &ci).unwrap().is_some());
    }

    #[test]
    fn the_suggested_command_keeps_the_program_last() {
        let plan = Plan {
            golden: "flow.golden".into(),
            mode: Mode::Check,
            source: Source::Program,
            screen: None,
            format: Format::Text,
            portable: false,
            fail_fast: false,
            max_mismatches: 20,
        };
        let rest = vec!["-n".to_string(), "a b".to_string()];
        let ctx = Context {
            program: Some(("tf", &rest)),
            script: None,
            connect: None,
            log_path: None,
            cols: 80,
            rows: 24,
            env: &[],
            argv: ["--script", "flow.ndjson", "--expect", "flow.golden", "--", "tf", "-n", "a b"]
                .map(String::from)
                .to_vec(),
        };
        let inv = invocation(&plan, &ctx);
        assert_eq!(inv.command, "vtee --script flow.ndjson --expect flow.golden");
        assert!(inv.tail.starts_with("-- tf -n "), "{}", inv.tail);
        assert!(inv.tail.ends_with("'a b'") || inv.tail.ends_with("\"a b\""), "{}", inv.tail);
    }
}
