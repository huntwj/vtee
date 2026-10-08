#[cfg(unix)]
use std::io::{self, Read};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::InfoLevel;
use owo_colors::{OwoColorize, Stream::Stderr};
use serde::{Deserialize, Serialize};

/// Standing preference, not just a per-invocation flag (`docs/decisions.md`) - whether `vtee
/// <program>` attaches to an already-live same-named session instead of erroring, the way
/// tmux's own `-A` does for one invocation. Loaded once via `confy::load("vtee", None)`; TOML,
/// at `confy`'s own default XDG path - no custom location logic needed.
#[derive(Serialize, Deserialize, Default)]
struct VteeConfig {
    attach_if_exists: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Attach to a live session - bare resolves like tmux's own `attach-session`: the only one
    /// if there's exactly one, else the most-recently-active.
    Attach { name: Option<String> },
    /// List live sessions (name, pid, age) - and stale leftovers, clearly marked as such.
    Ls,
    /// End a session: signal its holder process, clean up both its sockets and its pidfile.
    Kill { name: String },
}

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
    #[command(subcommand)]
    subcommand: Option<Commands>,

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
        conflicts_with_all = [
            "listen", "no_listen", "writable", "no_stdin", "dump_on_exit", "headless", "name",
            "attach_if_exists", "no_attach_if_exists"
        ]
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

    /// AI-only session: spawn and bind both sockets, but don't also self-attach (no human
    /// stdin/stdout forwarding, no raw-mode guard/watchdog, and a remote `resize` is never
    /// rejected as "owned by the real terminal" since there isn't one) - declared rather than
    /// left to the accident of whatever stdin/stdout happen to be (`docs/decisions.md`).
    #[arg(long, conflicts_with = "no_listen")]
    headless: bool,

    /// Session name, for `vtee attach`/`ls`/`kill` to find it by later - defaults to the
    /// program's own basename (`docs/architecture.md`, "session naming"). Two live sessions
    /// can't share a name; starting one while another of the same name is still running is a
    /// usage error unless `--attach-if-exists` (or its config default) says otherwise.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// Attach to the already-live session instead of erroring, if `--name` (or the default
    /// name) is already taken - tmux's own `-A`. Overrides the config file's standing default
    /// either direction; `--no-attach-if-exists` forces the error even if the config default is
    /// set.
    #[arg(long, conflicts_with = "no_attach_if_exists")]
    attach_if_exists: bool,

    #[arg(long, conflicts_with = "attach_if_exists")]
    no_attach_if_exists: bool,

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
    // Not `required_unless_present` at the clap level - clap's debug assertions reject
    // referencing a `#[command(subcommand)]` field that way ("does not exist" as a plain
    // argument/group id). Enforced manually in `main()` instead, for exactly the one case left
    // once a subcommand or `--connect` has already returned early.
    #[arg(trailing_var_arg = true)]
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

    if let Some(subcommand) = &cli.subcommand {
        return match subcommand {
            Commands::Attach { name } => run_attach(name.as_deref()),
            Commands::Ls => run_ls(),
            Commands::Kill { name } => run_kill(name),
        };
    }

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

    let (cols, rows) = terminal_size(cli.cols, cli.rows);

    let Some((program, rest)) = cli.command.split_first() else {
        // The real enforcement point, not a backup - `command` isn't `required_unless_present`
        // at the clap level at all (see its own doc comment for why), and both other ways to
        // reach this point without a program (a subcommand, or `--connect`) already returned
        // above, so getting here means genuinely neither was given.
        print_error("a command is required");
        return exit_code(exitcode::USAGE);
    };

    // Both branches below return normally with their result rather than calling
    // `std::process::exit` themselves, specifically so every local (including a spawned `Tap`)
    // drops before this function's own return does - calling `std::process::exit` while a
    // `Tap` is still alive would skip its child-killing `Drop` and orphan the child.
    if let Some(script_path) = &cli.script {
        return run_script(
            program,
            rest,
            ScriptOpts { log_path: cli.log.as_deref(), script_path, text_mode: cli.text, cols, rows, env: &cli.env },
        );
    }

    // `--no-listen` means *no footprint in the session directory at all* (`docs/decisions.md`'s
    // original rationale for it - "would rather not have one appear in a shared /tmp at all" -
    // extends to the pidfile too, not just the two sockets): skip naming, the duplicate check,
    // and the pidfile entirely, rather than track a session with nothing to attach to anyway.
    // `--script` is a one-shot, ephemeral run with no persistent named session either way (and
    // `conflicts_with_all` above already keeps `--name`/`--attach-if-exists` from reaching
    // here regardless).
    let (listen_path, tty_path, pidfile): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) = if cli.no_listen {
        (None, None, None)
    } else {
        let name = cli.name.clone().unwrap_or_else(|| default_session_name(program));

        let attach_if_exists = if cli.attach_if_exists {
            true
        } else if cli.no_attach_if_exists {
            false
        } else {
            confy::load::<VteeConfig>("vtee", None).map(|c| c.attach_if_exists).unwrap_or(false)
        };

        match vtee::session_is_live(&name) {
            Ok(true) if attach_if_exists => return run_attach(Some(&name)),
            Ok(true) => {
                print_error(&format!("duplicate session: {name}"));
                return exit_code(exitcode::USAGE);
            }
            Ok(false) => {}
            Err(e) => {
                print_error(&format!("couldn't check for an existing session named {name:?}: {e}"));
                return exit_code(exitcode::OSERR);
            }
        }

        let session_paths = match vtee::session_paths(&name) {
            Ok(paths) => paths,
            Err(e) => {
                print_error(&format!("couldn't set up session {name:?}: {e}"));
                return exit_code(exitcode::OSERR);
            }
        };

        let listen_path = match cli.listen {
            // An explicit `--listen=<path>` is untouched by session naming entirely - still
            // the caller's own responsibility, same as before.
            Some(explicit) if explicit.as_os_str() != "\0" => explicit,
            // Bare `--listen`, or no `--listen` at all: the latter is read-only observation on
            // by default now (`docs/decisions.md`), at this session's own named NDJSON socket.
            _ => session_paths.sock.clone(),
        };
        (Some(listen_path), Some(session_paths.tty_sock.clone()), Some(session_paths.pidfile.clone()))
    };

    run_spawn(
        program,
        rest,
        InteractiveOpts {
            pidfile: pidfile.as_deref(),
            listen_path: listen_path.as_deref(),
            tty_path: tty_path.as_deref(),
            log_path: cli.log.as_deref(),
            write_policy: cli.writable.map(Into::into),
            no_stdin: cli.no_stdin,
            dump_on_exit: cli.dump_on_exit,
            headless: cli.headless,
            cols,
            rows,
            env: &cli.env,
        },
    )
}

/// `vtee tf` -> session `tf`; strips a leading path if `program` was given as one (e.g.
/// `vtee ./bin/tf` still names the session `tf`, not `./bin/tf`).
fn default_session_name(program: &str) -> String {
    Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program).to_string()
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

/// `run_spawn`'s own flags, bundled rather than positional - this is the mode `Cli` keeps
/// adding options to (`--headless` just now, more virtual-tty-spec flags expected per
/// `docs/roadmap.md`), and a positional arg list was already at `clippy::too_many_arguments`.
struct InteractiveOpts<'a> {
    /// `None` under `--no-listen` - no session bookkeeping at all then, not even a pidfile
    /// (`docs/decisions.md`: "no footprint in a shared /tmp" extends to this too).
    pidfile: Option<&'a Path>,
    log_path: Option<&'a Path>,
    listen_path: Option<&'a Path>,
    /// The raw-TTY socket (`docs/architecture.md`, "two peer protocols") - purely additive, for
    /// a later `vtee attach` or a different human; this invocation's own display/input stays
    /// direct `forward_input`/`forward_output` below, completely unchanged, so the already-
    /// tested `HumanPriority`/exclusive-handoff machinery keeps meaning exactly what it always
    /// has. `None` under `--no-listen`, same as `listen_path`.
    tty_path: Option<&'a Path>,
    write_policy: Option<vtee::WritePolicy>,
    no_stdin: bool,
    dump_on_exit: bool,
    headless: bool,
    cols: u16,
    rows: u16,
    env: &'a [(String, String)],
}

/// Removes a named session's files on the way out, whichever way `run_spawn` returns - the
/// same "a guard's `Drop` is the cleanup path, not a manually-called function at every return
/// site" shape `RawModeGuard`/`Tap`'s own `Drop` already use. Harmless if a path was never
/// created (e.g. `--no-listen`, no socket files at all) - `remove_file` on a missing path is
/// swallowed, not propagated.
struct SessionFiles<'a> {
    pidfile: Option<&'a Path>,
    listen_path: Option<&'a Path>,
    tty_path: Option<&'a Path>,
}

impl Drop for SessionFiles<'_> {
    fn drop(&mut self) {
        for p in [self.pidfile, self.listen_path, self.tty_path].into_iter().flatten() {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn run_spawn(program: &str, rest: &[String], cli: InteractiveOpts) -> ExitCode {
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

    // Named session bookkeeping (`docs/architecture.md`, "session naming") - written now that
    // the child is confirmed alive, so `vtee ls`/`kill`/`attach` can find this process.
    // `None` under `--no-listen`, which skips this entirely. The guard cleans up on every
    // return path below, success or failure alike.
    if let Some(pidfile) = cli.pidfile
        && let Err(e) = std::fs::write(pidfile, std::process::id().to_string())
    {
        print_error(&format!("couldn't write pidfile {}: {e}", pidfile.display()));
        return exit_code(exitcode::OSERR);
    }
    let _session_files = SessionFiles { pidfile: cli.pidfile, listen_path: cli.listen_path, tty_path: cli.tty_path };

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

    if let Some(path) = cli.tty_path {
        // Always `Open` (`docs/architecture.md`) - the raw byte protocol has no framing for an
        // `acquire_write`/`release_write` handshake, so `Exclusive` isn't a meaningful policy
        // here; a remote TTY attacher can always type, same as this invocation's own local
        // typing already can via `forward_input`, independent of this policy entirely.
        if let Err(e) = vtee::Tap::serve_tty(tap.clone(), path, vtee::WritePolicy::Open) {
            raw_guard.take();
            print_error(&format!("couldn't bind the attach socket: {e}"));
            return exit_code(exitcode::OSERR);
        }
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

/// `vtee attach [name]` (`docs/architecture.md`): a pure client, no `Tap` access at all - it
/// connects to an existing session's raw-TTY socket and relays, nothing more. Can't propagate
/// the wrapped program's real exit code this way (the raw byte protocol has no framing for it,
/// unlike the originating `vtee <program>` invocation's own direct `tap.wait()`) - a known,
/// documented v1 limitation, not an oversight; this always returns `SUCCESS` once the session
/// ends (socket EOF) rather than claim an exit code it has no way to know.
#[cfg(unix)]
fn run_attach(name: Option<&str>) -> ExitCode {
    let name = match name {
        Some(n) => n.to_string(),
        None => match resolve_default_session() {
            Ok(n) => n,
            Err(message) => {
                print_error(&message);
                return exit_code(exitcode::USAGE);
            }
        },
    };
    let paths = match vtee::session_paths(&name) {
        Ok(p) => p,
        Err(e) => {
            print_error(&format!("couldn't resolve session {name:?}: {e}"));
            return exit_code(exitcode::OSERR);
        }
    };
    let stream = match std::os::unix::net::UnixStream::connect(&paths.tty_sock) {
        Ok(s) => s,
        Err(e) => {
            print_error(&format!("failed to attach to {name:?}: {e}"));
            return exit_code(exitcode::NOHOST);
        }
    };
    let _raw_guard = vtee::RawModeGuard::new();
    match relay_tty(stream) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            print_error(&format!("attach session {name:?} ended with an error: {e}"));
            exit_code(exitcode::OSERR)
        }
    }
}

#[cfg(not(unix))]
fn run_attach(_name: Option<&str>) -> ExitCode {
    print_error("attach needs Unix (requirement 8 defers Windows)");
    exit_code(exitcode::OSERR)
}

/// Bare `vtee attach` (no name): tmux's own `attach-session` resolution - the only live
/// session if there's exactly one, else the most-recently-active. "Most-recently-active" is
/// approximated by the pidfile's own mtime, since nothing richer (a real last-activity
/// timestamp) is tracked yet - a reasonable, documented proxy, not an exact reproduction of
/// tmux's own bookkeeping.
#[cfg(unix)]
fn resolve_default_session() -> Result<String, String> {
    let names = vtee::session_names().map_err(|e| format!("couldn't list sessions: {e}"))?;
    let mut live = Vec::new();
    for name in names {
        if vtee::session_is_live(&name).unwrap_or(false) {
            let mtime = vtee::session_paths(&name)
                .ok()
                .and_then(|p| std::fs::metadata(&p.pidfile).ok())
                .and_then(|m| m.modified().ok())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            live.push((name, mtime));
        }
    }
    match live.len() {
        0 => Err("no live sessions to attach to".to_string()),
        1 => Ok(live.into_iter().next().unwrap().0),
        _ => {
            live.sort_by_key(|(_, mtime)| *mtime);
            Ok(live.pop().expect("just checked len() > 1").0)
        }
    }
}

/// Raw bidirectional byte forwarding between `stream` and our own real stdin/stdout - the raw-
/// TTY protocol's client side, shared by `vtee attach` here. Mirrors `Tap::forward_input`'s
/// own raw (not line-buffered) read loop deliberately: arbitrary control bytes/escape sequences
/// must pass through untouched, the same reason that function doesn't use `.lines()` either.
#[cfg(unix)]
fn relay_tty(stream: std::os::unix::net::UnixStream) -> io::Result<()> {
    let reader_stream = stream.try_clone()?;
    let mut writer = stream;
    let reader_thread = std::thread::spawn(move || {
        let mut reader = reader_stream;
        let mut stdout = std::io::stdout();
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stdout.write_all(&buf[..n]).is_err() {
                        break;
                    }
                    let _ = stdout.flush();
                }
            }
        }
    });

    let mut stdin = std::io::stdin();
    let mut buf = [0u8; 4096];
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if writer.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    // Same reasoning as `run_connect`'s own `shutdown` call: a plain `drop(writer)` wouldn't
    // signal EOF to the server here either, for the same `try_clone`-duplicates-the-fd reason.
    let _ = writer.shutdown(std::net::Shutdown::Write);
    let _ = reader_thread.join();
    Ok(())
}

/// `vtee ls`: every session name found in the default session directory, each marked `live`
/// (its socket actually answers) or `stale` (a leftover from a session that ended without
/// cleaning up after itself - e.g. `SIGKILL`, `docs/decisions.md`'s documented gap).
#[cfg(unix)]
fn run_ls() -> ExitCode {
    let names = match vtee::session_names() {
        Ok(n) => n,
        Err(e) => {
            print_error(&format!("couldn't list sessions: {e}"));
            return exit_code(exitcode::OSERR);
        }
    };
    if names.is_empty() {
        println!("no sessions");
        return ExitCode::SUCCESS;
    }
    for name in names {
        let live = vtee::session_is_live(&name).unwrap_or(false);
        let pid = vtee::session_paths(&name).ok().and_then(|p| std::fs::read_to_string(&p.pidfile).ok());
        let status = if live { "live" } else { "stale" };
        match pid {
            Some(pid) => println!("{name}\tpid={}\t{status}", pid.trim()),
            None => println!("{name}\t{status}"),
        }
    }
    ExitCode::SUCCESS
}

#[cfg(not(unix))]
fn run_ls() -> ExitCode {
    print_error("ls needs Unix (requirement 8 defers Windows)");
    exit_code(exitcode::OSERR)
}

/// `vtee kill <name>`: signal the holder process and clean up its files directly, rather than
/// relying on the holder's own `SessionFiles` guard - `watch_sigterm`'s handler calls
/// `std::process::exit` directly, which skips every destructor including that guard's, so
/// without this the files would be left behind despite the process actually stopping.
#[cfg(unix)]
fn run_kill(name: &str) -> ExitCode {
    let paths = match vtee::session_paths(name) {
        Ok(p) => p,
        Err(e) => {
            print_error(&format!("couldn't resolve session {name:?}: {e}"));
            return exit_code(exitcode::OSERR);
        }
    };
    // Confirmed, not assumed, by an adversarial review: this used to trust the pidfile's raw
    // contents unconditionally, with no liveness check at all - a stale pidfile (left behind
    // by a `SIGKILL`'d holder, `docs/decisions.md`'s documented gap) pointing at a since-reused
    // PID would have `SIGTERM`'d some unrelated process, immediately, every time, not just in
    // some narrow race window. Verifying the socket actually answers first - the same check
    // `ls` already uses - closes that: a stale name just gets its leftover files cleaned up,
    // with nothing signaled, since there is nothing real to signal.
    let pidfile_existed = paths.pidfile.exists();
    match vtee::session_is_live(name) {
        Ok(true) => {
            if let Some(pid) = std::fs::read_to_string(&paths.pidfile).ok().and_then(|s| s.trim().parse::<i32>().ok()) {
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
        // A stale leftover (a pidfile existed but nothing answers) just gets cleaned up below,
        // nothing to signal - but a name with *no* pidfile at all never had a session, which
        // stays a real error rather than a silent no-op success.
        Ok(false) if pidfile_existed => {}
        Ok(false) => {
            print_error(&format!("no session named {name:?}"));
            return exit_code(exitcode::NOHOST);
        }
        Err(e) => {
            print_error(&format!("couldn't check session {name:?}: {e}"));
            return exit_code(exitcode::OSERR);
        }
    }
    let _ = std::fs::remove_file(&paths.sock);
    let _ = std::fs::remove_file(&paths.tty_sock);
    let _ = std::fs::remove_file(&paths.pidfile);
    ExitCode::SUCCESS
}

#[cfg(not(unix))]
fn run_kill(_name: &str) -> ExitCode {
    print_error("kill needs Unix (requirement 8 defers Windows)");
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
