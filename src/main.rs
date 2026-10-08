use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use clap_verbosity_flag::InfoLevel;
use owo_colors::{OwoColorize, Stream::Stderr};

/// `--writable[=<MODE>]` (`docs/roadmap.md`, "I think those should all be options") - a
/// `clap::ValueEnum` rather than a hand-parsed string so an unknown mode gets clap's own
/// "invalid value ... [possible values: ...]" message for free.
#[derive(Clone, Copy, clap::ValueEnum)]
enum WritableMode {
    Open,
    Exclusive,
    ExclusiveShared,
}

impl From<WritableMode> for vtee::WritePolicy {
    fn from(mode: WritableMode) -> Self {
        match mode {
            WritableMode::Open => vtee::WritePolicy::Open,
            WritableMode::Exclusive => vtee::WritePolicy::Exclusive { human_priority: vtee::HumanPriority::AlwaysWins },
            WritableMode::ExclusiveShared => {
                vtee::WritePolicy::Exclusive { human_priority: vtee::HumanPriority::Participant }
            }
        }
    }
}

/// `vtee`: a terminal multiplexer for AI and testing - tee for a TUI, with queryable terminal
/// state (`docs/architecture.md`). Three primary modes, selected by which flags are given
/// rather than a subcommand, so existing invocations keep working as flags are added:
/// `--script <path>` for a batch test run, `--connect <path>` to attach to someone else's
/// `--listen`, and plain `[--] <command> [args...]` for everything else (interactive, with
/// `--headless` for an AI-only session - `docs/decisions.md`).
#[derive(Parser)]
#[command(name = "vtee", version)]
struct Cli {
    #[command(flatten)]
    verbosity: clap_verbosity_flag::Verbosity<InfoLevel>,

    /// `CommandResult.snapshot.lines` only, for a human skimming output, instead of the full
    /// JSON - requested by TinyFugue as a quicker alternative while developing a script.
    /// Shared by `--script` and `--connect`; meaningless (and ignored) otherwise.
    #[arg(long)]
    text: bool,

    /// Attach to someone else's `--listen` socket instead of spawning anything - the observation
    /// client (`docs/roadmap.md`, "connect and watch").
    #[arg(long, value_name = "PATH")]
    connect: Option<PathBuf>,

    /// Record the child's raw byte stream (asciicast-compatible) for later `replay()` - distinct
    /// from the leveled diagnostic logging `-v` controls.
    #[arg(long, value_name = "PATH")]
    log: Option<PathBuf>,

    /// Run a newline-delimited-JSON script of `Command`s non-interactively (`docs/roadmap.md`,
    /// the batch CLI TinyFugue asked for) instead of the plain interactive mode. Conflicts with
    /// every interactive-only flag below rather than silently ignoring them - `run_script` never
    /// looks at any of them, so accepting them without comment would be misleading, not lenient.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = ["listen", "no_listen", "writable", "no_stdin", "dump_on_exit", "headless"]
    )]
    script: Option<PathBuf>,

    /// Observation/control socket (`docs/decisions.md`). Read-only observation is on by default
    /// now even without this flag at all; give it explicitly (bare, or `--listen=<path>`) to pick
    /// your own path instead of the hardened per-user default, or pair with `--writable` for
    /// write access. The `=` is required for an explicit path (POSIX's own rule for an optional-
    /// value long option, same as `grep --color[=WHEN]`) - `--listen <path>` with a space is not
    /// the same thing and will be read as `<path>` starting the command instead.
    // `default_missing_value`'s sentinel for "bare, no path" can't be an empty string - clap
    // (measured, not assumed: an empty string was silently rejected, falling back to demanding
    // a value) treats that as no default at all. `"\0"` is used instead since a real path can
    // never contain a NUL byte, on any platform this runs on.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "\0", value_name = "PATH", conflicts_with = "no_listen")]
    listen: Option<PathBuf>,

    /// Opt out of the default-on observation socket entirely - for a CI script driving `vtee`
    /// through plain stdin/stdout, which has no use for an attachable socket and would rather
    /// not have one appear in a shared `/tmp` at all.
    #[arg(long, conflicts_with = "listen")]
    no_listen: bool,

    /// Allow write commands on the observation socket (`docs/roadmap.md`, "Full duplex").
    /// Meaningless without a socket to allow them on - rejected alongside `--no-listen` for the
    /// same reason `--headless` is, rather than being silently accepted and silently ignored.
    /// `open`: any connected writer, no arbitration - fine when nothing else is also writing.
    /// `exclusive`/`exclusive-shared`: "take the pen" - one holder at a time via
    /// `acquire_write`/`release_write`; the two differ only in whether a human's own typing
    /// also revokes whoever currently holds it.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "open", value_name = "MODE", conflicts_with = "no_listen")]
    writable: Option<WritableMode>,

    /// Don't forward our own stdin to the child - a human can watch (output still forwarded)
    /// without being able to type, leaving the pty to a `--connect --writable` client instead.
    #[arg(long)]
    no_stdin: bool,

    /// Print the final screen state to stderr when the child exits. Off by default
    /// (`docs/decisions.md`): redundant for a human who already watched the real output render
    /// live, and never what `--script`/`--listen` are for.
    #[arg(long)]
    dump_on_exit: bool,

    /// AI-only session, declared rather than left to the accident of whatever stdin/stdout
    /// happen to be (`docs/decisions.md`): no human stdin/stdout forwarding, no raw-mode
    /// guard/watchdog, and a remote `resize` is never rejected as "owned by the real terminal"
    /// since there isn't one.
    #[arg(long, conflicts_with = "no_listen")]
    headless: bool,

    /// Virtual tty width, overriding the real terminal's own (`docs/roadmap.md`, "virtual tty
    /// specs" - the first of more to follow: terminal emulation mode, colour support, ...).
    /// Falls back to the real terminal's size, or 80 if there isn't one.
    #[arg(long, value_name = "N")]
    cols: Option<u16>,

    /// Virtual tty height - see `--cols`. Falls back to the real terminal's size, or 24.
    #[arg(long, value_name = "N")]
    rows: Option<u16>,

    /// Set (or override) an environment variable on the spawned child, `KEY=VALUE` - repeatable.
    /// Applied on top of `vtee`'s own inherited environment, not in place of it (requirement
    /// 6, "Deterministic": `TERM` is just one entry here, not a separate flag). Shared by every
    /// mode, same as `--cols`/`--rows` - unlike those, `run_script` does read this one too.
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    env: Vec<(String, String)>,

    /// The program to run, and its own arguments - everything after the last recognized flag
    /// (or after an explicit `--`, needed if the program's own name or first argument could
    /// otherwise be mistaken for one of ours). Not required with `--connect`, which has nothing
    /// to spawn.
    #[arg(trailing_var_arg = true, required_unless_present = "connect")]
    command: Vec<String>,
}

fn main() -> ExitCode {
    // The watchdog re-execs this same binary with this hidden flag (`vtee::spawn_tty_watchdog`
    // via `RawModeGuard`) - handled before anything else, including `clap`, since it's a
    // different process entirely, not a mode of the CLI below, and has no business being
    // declared (and thus shown in `--help`) as one of its flags.
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--internal-tty-guard") {
        vtee::run_tty_guard();
    }

    human_panic::setup_panic!();

    // `Cli::parse()` would exit(2) itself on a bad argument - clap's own convention, not this
    // project's sysexits one (`exitcode`, adopted deliberately for every other error path).
    // `--help`/`--version` are errors in clap's own model too but exit 0 and belong on stdout;
    // `Error::print()`/`exit_code()` already know which is which, so this just routes a real
    // usage error through the same `exitcode::USAGE` everything else uses instead of clap's 2.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return if e.exit_code() == 0 { ExitCode::SUCCESS } else { exit_code(exitcode::USAGE) };
        }
    };

    // Interactive, non-headless, attached to a real terminal: the one case where something
    // else (the child) owns the actual screen, so none of our own diagnostic output can safely
    // land on the same stderr without interleaving with it (`docs/decisions.md`) - route it to
    // a file next to the session's own socket instead. Every other mode (`--script`,
    // `--connect`, `--headless`) has nothing competing for the terminal, so stderr is fine.
    let mux_shows_child_live =
        cli.script.is_none() && cli.connect.is_none() && !cli.headless && std::io::stdin().is_terminal();

    let mut builder = env_logger::Builder::new();
    builder.filter_level(cli.verbosity.log_level_filter());
    if mux_shows_child_live {
        match vtee::default_log_path().and_then(std::fs::File::create) {
            Ok(file) => {
                print_info(&format!("diagnostic log at {}", vtee_log_path_hint()));
                builder.write_style(env_logger::WriteStyle::Never).target(env_logger::Target::Pipe(Box::new(file)));
            }
            // Couldn't get a log file for a session that otherwise can't safely log at all -
            // not worth refusing to run the program over (same reasoning as the implicit
            // `--listen` default below); diagnostics are just unavailable this run.
            Err(_) => {
                builder.filter_level(log::LevelFilter::Off);
            }
        }
    }
    builder.init();

    if let Some(path) = &cli.connect {
        return run_connect(path, cli.text);
    }

    let listen_path: Option<PathBuf> = if cli.no_listen {
        None
    } else {
        match cli.listen {
            Some(explicit) if explicit.as_os_str() != "\0" => Some(explicit),
            // Bare `--listen`, or no `--listen` at all: the latter is read-only observation on
            // by default now (`docs/decisions.md`) since attaching later is the common case
            // this project actually has - but only because `default_socket_path` makes that
            // safe. An explicit `--listen` failing to set up its default directory is still a
            // hard error (the user asked for it specifically); the *implicit* default failing
            // (e.g. not Unix) just means this run isn't attachable, same as `--no-listen` -
            // not worth refusing to run the program over.
            Some(_) => match vtee::default_socket_path() {
                Ok(path) => Some(path),
                Err(e) => {
                    print_error(&format!("--listen (no path given) couldn't set up a default socket directory: {e}"));
                    return exit_code(exitcode::OSERR);
                }
            },
            None => vtee::default_socket_path().ok(),
        }
    };

    let (cols, rows) = terminal_size(cli.cols, cli.rows);

    let Some((program, rest)) = cli.command.split_first() else {
        // Unreachable in practice: `required_unless_present` already enforced this unless
        // `--connect` was given, and that path already returned above.
        print_error("a command is required");
        return exit_code(exitcode::USAGE);
    };

    // Both branches below return normally with their result rather than calling
    // `std::process::exit` themselves, specifically so every local (including a spawned `Tap`)
    // drops before this function's own return does - calling `std::process::exit` while a
    // `Tap` is still alive would skip its child-killing `Drop` and orphan the child.
    match &cli.script {
        Some(script_path) => run_script(
            program,
            rest,
            ScriptOpts { log_path: cli.log.as_deref(), script_path, text_mode: cli.text, cols, rows, env: &cli.env },
        ),
        None => run_interactive(
            program,
            rest,
            InteractiveOpts {
                log_path: cli.log.as_deref(),
                listen_path: listen_path.as_deref(),
                write_policy: cli.writable.map(Into::into),
                no_stdin: cli.no_stdin,
                dump_on_exit: cli.dump_on_exit,
                headless: cli.headless,
                cols,
                rows,
                env: &cli.env,
            },
        ),
    }
}

/// Where the diagnostic log file ended up, for the one-time notice printed before raw mode
/// starts - best-effort only; if this can't be computed again for some reason, the notice just
/// says less, which beats failing a run over a status message.
fn vtee_log_path_hint() -> String {
    vtee::default_log_path().map(|p| p.display().to_string()).unwrap_or_else(|_| "?".to_string())
}

/// `exitcode`'s constants are untyped `i32` (sysexits.h values, all small and non-negative),
/// not the `std::process::ExitCode` our functions actually return - this is the one place that
/// conversion happens.
fn exit_code(code: exitcode::ExitCode) -> ExitCode {
    ExitCode::from(code as u8)
}

/// `--env KEY=VALUE` - splits on the *first* `=` only, so a value containing `=` (e.g.
/// `--env PATH=/a=b`) still parses as intended rather than erroring or truncating.
fn parse_env_pair(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) => Ok((key.to_string(), value.to_string())),
        None => Err(format!("expected KEY=VALUE, got {s:?}")),
    }
}

/// A fatal, always-visible message - printed directly to stderr (never through the file-backed
/// logger above) because the process is aborting right here, before or instead of ever handing
/// the terminal to the child; hiding this in a log file the user doesn't know to check would be
/// strictly worse than today's plain `eprintln!`. Coloured when the stream supports it.
fn print_error(msg: &str) {
    eprintln!("{}", format!("vtee: {msg}").if_supports_color(Stderr, |t| t.red().to_string()));
}

/// A one-time, pre-session notice (e.g. where the diagnostic log landed) - safe on stderr
/// because it only ever fires before raw mode is entered, same reasoning as `print_error`.
fn print_info(msg: &str) {
    eprintln!("{}", format!("vtee: {msg}").if_supports_color(Stderr, |t| t.dimmed().to_string()));
}

/// Resolve the real terminal size, falling back to 80x24 when there isn't a usable one -
/// either no tty at all, or a tty that hasn't negotiated a real size yet (`crossterm` can
/// report `0x0` for that, observed via `docker run -t` without `-i`; never a usable size).
/// `--cols`/`--rows` (`Cli`) override either dimension independently of the other - the first of
/// what's meant to grow into a fuller virtual tty spec (`docs/roadmap.md`): terminal emulation
/// mode, colour support, and whatever else turns out to matter once something other than a real
/// terminal's own size needs specifying, headless sessions being the obvious case since there's
/// no real terminal to detect a size from at all.
fn terminal_size(cols_override: Option<u16>, rows_override: Option<u16>) -> (u16, u16) {
    let (auto_cols, auto_rows) = if std::io::stdout().is_terminal() {
        match crossterm::terminal::size() {
            Ok((cols, rows)) if cols > 0 && rows > 0 => (cols, rows),
            _ => (80, 24),
        }
    } else {
        (80, 24)
    };
    (cols_override.unwrap_or(auto_cols), rows_override.unwrap_or(auto_rows))
}

/// `run_interactive`'s own flags, bundled rather than positional - this is the mode `Cli` keeps
/// adding options to (`--headless` just now, more virtual-tty-spec flags expected per
/// `docs/roadmap.md`), and a positional arg list was already at `clippy::too_many_arguments`.
struct InteractiveOpts<'a> {
    log_path: Option<&'a Path>,
    listen_path: Option<&'a Path>,
    write_policy: Option<vtee::WritePolicy>,
    no_stdin: bool,
    dump_on_exit: bool,
    headless: bool,
    cols: u16,
    rows: u16,
    env: &'a [(String, String)],
}

fn run_interactive(program: &str, rest: &[String], cli: InteractiveOpts) -> ExitCode {
    // `--headless` forces both off outright, not just input: there's no human stdin to forward
    // and no point forwarding to a stdout nobody's watching either, since the AI side is meant
    // to use `--listen`/snapshot instead (`docs/decisions.md`).
    let opts = vtee::SpawnOptions {
        cols: cli.cols,
        rows: cli.rows,
        log_path: cli.log_path,
        forward_input: !cli.no_stdin && !cli.headless,
        forward_output: !cli.headless,
        env: cli.env,
    };

    // `Arc` even when `--listen` isn't given: `Tap::serve` needs one, and every method it
    // would otherwise need `&Tap` for already takes `&self`, so this costs one allocation and
    // avoids branching the rest of this function on whether observation is in use.
    let tap = match vtee::Tap::spawn(program, rest, opts) {
        Ok(t) => Arc::new(t),
        Err(e) => {
            print_error(&format!("failed to spawn {program}: {e}"));
            return exit_code(exitcode::OSERR);
        }
    };

    // The same check `RawModeGuard`/`watch_resize` act on: if there's a real terminal here,
    // it's the one real screen size that means anything, so a remote `Resize` would just fight
    // with it (`Command::authorize`, `docs/roadmap.md`). `no_stdin` doesn't change this - the
    // terminal can still be real and still own resize even if we're not forwarding typing to it.
    // `--headless` forces this false outright - a deliberate "no real terminal, full stop"
    // rather than leaving it to whatever `is_terminal()` happens to detect.
    let resize_owned_by_terminal = !cli.headless && std::io::stdin().is_terminal();

    // Likewise skipped outright under `--headless` rather than left to `RawModeGuard`'s own
    // (already correct) no-op when there's no tty - a declared mode shouldn't still depend on
    // the accident of whatever stdin/stdout happen to be.
    let mut raw_guard = if cli.headless { None } else { Some(vtee::RawModeGuard::new()) };
    if !cli.headless {
        tap.watch_resize();
    }
    tap.watch_sigterm();

    if let Some(path) = cli.listen_path {
        let write_policy = cli.write_policy.unwrap_or(vtee::WritePolicy::ReadOnly);
        let serve_opts = vtee::ServeOptions { write_policy, resize_owned_by_terminal };
        if let Err(e) = vtee::Tap::serve(tap.clone(), path, serve_opts) {
            // Drop the guard (restoring cooked mode) *before* printing - this is a fatal path
            // that can be reached after raw mode is already active and the child may already
            // be drawing, unlike the spawn-failure path above which can't (`docs/decisions.md`).
            raw_guard.take();
            print_error(&format!("--listen failed: {e}"));
            return exit_code(exitcode::OSERR);
        }
        log::info!("listening for observers ({write_policy:?}) at {}", path.display());
    }

    let status = tap.wait();
    drop(raw_guard);

    match status {
        Ok(status) => {
            if cli.dump_on_exit {
                let snap = tap.snapshot();
                eprintln!(
                    "\n--- vtee: child exited (success={}, exit_code={}), {} bytes seen ---",
                    status.success(),
                    status.exit_code(),
                    snap.byte_offset
                );
                for (i, line) in snap.lines.iter().enumerate() {
                    eprintln!("{i:2}: {line:?}");
                }
                eprintln!(
                    "cursor={:?} modes={:?} alt_screen={} title={:?}",
                    snap.cursor, snap.modes, snap.alt_screen, snap.title
                );
            }
            // Propagate the child's own exit code rather than collapsing every failure to a
            // fixed value - a caller scripting around vtee (CI, TF's harness) likely cares
            // which code the wrapped program actually exited with, the way `env`/`time` do.
            if status.success() { ExitCode::SUCCESS } else { ExitCode::from((status.exit_code() & 0xff) as u8) }
        }
        Err(e) => {
            print_error(&format!("error waiting for child: {e}"));
            exit_code(exitcode::OSERR)
        }
    }
}

/// The batch CLI TinyFugue asked for (`docs/roadmap.md`): run a newline-delimited-JSON script
/// of `Command`s against a non-interactive tap - no stdin forwarding, no raw passthrough to
/// our own stdout, nothing the human-terminal case needs - and write one `CommandResult` line
/// to stdout per `snapshot`/`wait_for` command. Always kills the child at the end rather than
/// hoping a script leaves it in a state where it exits on its own - deterministic beats
/// "probably fine" for something a test harness depends on not hanging.
struct ScriptOpts<'a> {
    log_path: Option<&'a Path>,
    script_path: &'a Path,
    text_mode: bool,
    cols: u16,
    rows: u16,
    env: &'a [(String, String)],
}

fn run_script(program: &str, rest: &[String], opts: ScriptOpts) -> ExitCode {
    let spawn_opts = vtee::SpawnOptions {
        cols: opts.cols,
        rows: opts.rows,
        log_path: opts.log_path,
        forward_input: false,
        forward_output: false,
        env: opts.env,
    };

    let tap = match vtee::Tap::spawn(program, rest, spawn_opts) {
        Ok(t) => t,
        Err(e) => {
            print_error(&format!("failed to spawn {program}: {e}"));
            return exit_code(exitcode::OSERR);
        }
    };

    tap.watch_sigterm();

    let code = run_script_body(&tap, opts.script_path, opts.text_mode);

    tap.kill();
    let _ = tap.wait();
    code
}

fn run_script_body(tap: &vtee::Tap, script_path: &Path, text_mode: bool) -> ExitCode {
    let script = match std::fs::File::open(script_path) {
        Ok(f) => f,
        Err(e) => {
            print_error(&format!("failed to open script {}: {e}", script_path.display()));
            return exit_code(exitcode::NOINPUT);
        }
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for (lineno, line) in std::io::BufReader::new(script).lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                print_error(&format!("error reading script line {}: {e}", lineno + 1));
                return exit_code(exitcode::IOERR);
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let cmd: vtee::Command = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                print_error(&format!("bad command on script line {}: {e}", lineno + 1));
                return exit_code(exitcode::DATAERR);
            }
        };
        match cmd.execute(tap) {
            Ok(Some(result)) => {
                let printed = if text_mode {
                    print_text(&mut out, &result);
                    Ok(())
                } else {
                    serde_json::to_string(&result).map(|text| {
                        let _ = writeln!(out, "{text}");
                    })
                };
                if let Err(e) = printed {
                    // `Snapshot`'s fields (plain strings/ints/bools/simple enums) make this
                    // realistically unreachable, but "realistically unreachable" still isn't
                    // "impossible," and silently unwrapping here would be the one place in
                    // this function that doesn't report a failure the way every other one does.
                    print_error(&format!("failed to serialize result on script line {}: {e}", lineno + 1));
                    return exit_code(exitcode::SOFTWARE);
                }
                let _ = out.flush();
            }
            Ok(None) => {}
            Err(e) => {
                print_error(&format!("command on script line {} failed: {e}", lineno + 1));
                // `send_key` reports an unknown key name as `InvalidInput` - that's bad data in
                // the script, not a software fault, and sysexits draws exactly that distinction.
                let code =
                    if e.kind() == std::io::ErrorKind::InvalidInput { exitcode::DATAERR } else { exitcode::SOFTWARE };
                return exit_code(code);
            }
        }
    }
    ExitCode::SUCCESS
}

/// Observation client (`docs/roadmap.md`, "connect and watch"): connect to a `--listen` socket,
/// relay our own stdin to it line by line - the exact same `Command` NDJSON the batch CLI reads
/// from a script file, just typed or piped in live instead - and print whatever `CommandResult`
/// lines come back.
#[cfg(unix)]
fn run_connect(path: &Path, text_mode: bool) -> ExitCode {
    let stream = match std::os::unix::net::UnixStream::connect(path) {
        Ok(s) => s,
        Err(e) => {
            print_error(&format!("failed to connect to {}: {e}", path.display()));
            return exit_code(exitcode::NOHOST);
        }
    };
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => {
            print_error(&format!("{e}"));
            return exit_code(exitcode::OSERR);
        }
    };
    let Ok(reader_side) = stream.try_clone() else {
        print_error("failed to clone the connection for reading");
        return exit_code(exitcode::OSERR);
    };
    drop(stream); // see the comment on `shutdown` below - this alone would not be enough

    let reader_thread = std::thread::spawn(move || {
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        for line in std::io::BufReader::new(reader_side).lines() {
            let Ok(line) = line else { break };
            if text_mode {
                match serde_json::from_str::<vtee::CommandResult>(&line) {
                    Ok(result) => print_text(&mut out, &result),
                    Err(e) => print_error(&format!("malformed response from server: {e}")),
                }
            } else {
                let _ = writeln!(out, "{line}");
            }
            let _ = out.flush();
        }
    });

    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        if writeln!(writer, "{line}").is_err() {
            break;
        }
    }
    // Measured, not assumed: plain `drop(writer)` does not signal EOF to the server here.
    // `UnixStream::try_clone` duplicates the underlying fd - all three handles (`stream`,
    // `writer`, `reader_side`) refer to the *same* socket, and a stream socket only sees EOF
    // when every fd referencing it is closed, or `shutdown` is called explicitly; dropping one
    // of several duplicates does neither by itself for the others still open. `shutdown(Write)`
    // acts on the socket itself, not just this one fd, so it reaches the server's read side
    // regardless of `reader_side` still being open on this end.
    let _ = writer.shutdown(std::net::Shutdown::Write);
    let _ = reader_thread.join();
    ExitCode::SUCCESS
}

#[cfg(not(unix))]
fn run_connect(_path: &Path, _text_mode: bool) -> ExitCode {
    print_error("--connect needs Unix (requirement 8 defers Windows)");
    exit_code(exitcode::OSERR)
}

/// `--text`: `CommandResult.snapshot.lines` only, for a human skimming output - requested by
/// TinyFugue as a quicker alternative to the full JSON while developing a script, and reused
/// as-is by `--connect`'s client for the same reason.
fn print_text(out: &mut impl Write, result: &vtee::CommandResult) {
    let (label, snapshot) = match result {
        vtee::CommandResult::Snapshot { snapshot } => ("snapshot".to_string(), snapshot),
        vtee::CommandResult::WaitFor { matched, snapshot } => (format!("wait_for matched={matched}"), snapshot),
        vtee::CommandResult::Error { message } => {
            let _ = writeln!(out, "--- error: {message} ---");
            return;
        }
        vtee::CommandResult::WriteAccess { acquired } => {
            let _ = writeln!(out, "--- write_access acquired={acquired} ---");
            return;
        }
    };
    let _ = writeln!(out, "--- {label} (byte_offset={}) ---", snapshot.byte_offset);
    for line in &snapshot.lines {
        let _ = writeln!(out, "{}", line.trim_end());
    }
}
