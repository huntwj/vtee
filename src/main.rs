mod client;
mod output;
mod script;
mod session_cmds;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use clap_verbosity_flag::InfoLevel;
use serde::{Deserialize, Serialize};

use client::{ScriptOpts, run_connect, run_replay, run_script};
use output::{exit_code, fail, print_error, print_info};
use session_cmds::{run_attach, run_kill, run_ls};

/// Standing preferences, loaded with `confy` (TOML at its default XDG path).
#[derive(Serialize, Deserialize, Default)]
struct VteeConfig {
    /// Attach to a live same-named session instead of erroring, like tmux's `-A`.
    attach_if_exists: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Attach to a live session - bare resolves like tmux's own `attach-session`: the only one
    /// if there's exactly one, else the one attached to most recently.
    ///
    /// There is no detach key, and Ctrl-C goes to the program, not to attach. To leave, close the
    /// terminal or stop the attach process; the session keeps running.
    Attach { name: Option<String> },
    /// List sessions: name, holder pid and live or stale, plus sock=<path> for a --listen socket.
    Ls,
    /// End a session: signal its holder process, clean up both its sockets and its pidfile.
    Kill { name: String },
    /// Replay a recording made with `--log` and print the final screen (text rows by default).
    Replay {
        /// The asciicast v2 file to replay.
        file: PathBuf,
        /// Print the final snapshot as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// `--writable[=<MODE>]`; a `ValueEnum` so an unknown mode gets clap's own error message.
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

/// What a bare `--listen` (no path) parses to. clap treats an empty default as none, and a real
/// path can never contain NUL.
const BARE_LISTEN: &str = "\0";

/// A tee for terminal programs: watch, share, drive and test any TUI's screen. Run a program in a
/// virtual terminal that people and programs can watch, type into and read back as structured data.
///
/// `vtee [OPTIONS] [--] PROGRAM [ARGS]...` starts a session. `--script PATH` runs a batch of
/// JSON commands and exits; `--connect PATH` talks to a running session's command socket.
#[derive(Parser)]
#[command(name = "vtee", version)]
struct Cli {
    #[command(subcommand)]
    subcommand: Option<Commands>,

    #[command(flatten)]
    verbosity: clap_verbosity_flag::Verbosity<InfoLevel>,

    /// With `--script` or `--connect`, print only the screen rows of each result instead of the
    /// full JSON. Ignored otherwise.
    #[arg(long)]
    text: bool,

    /// Talk to a running session's command socket instead of starting a program: read JSON
    /// commands from stdin, one per line, and print the replies.
    #[arg(long, value_name = "PATH")]
    connect: Option<PathBuf>,

    /// Record the program's raw output to PATH as an asciicast v2 file. `vtee replay PATH`
    /// shows the final screen. (`-v` controls diagnostic logging, which is separate.)
    #[arg(long, value_name = "PATH")]
    log: Option<PathBuf>,

    /// Run the JSON commands in PATH (one per line) against the program, print each result, then
    /// stop the program. Cannot be combined with the session options below.
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with_all = [
            "listen", "no_listen", "writable", "no_stdin", "dump_on_exit", "headless", "name",
            "attach_if_exists", "no_attach_if_exists"
        ]
    )]
    script: Option<PathBuf>,

    /// Put the session's JSON command socket at PATH instead of in the per-user session
    /// directory. Write it as `--listen=PATH` (the `=` is required; with a space, PATH would be
    /// read as the program to run). Every session has a read-only socket by default.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = BARE_LISTEN, value_name = "PATH", conflicts_with = "no_listen")]
    listen: Option<PathBuf>,

    /// Create no socket, pid file or session name, so nothing is left in the session directory
    /// (useful for a CI run driven only through stdin and stdout).
    #[arg(long, conflicts_with = "listen")]
    no_listen: bool,

    /// Let clients of the command socket write (send text and keys, resize), not just read. MODE
    /// is `open` (anyone, no arbitration; the default), `exclusive` (one writer at a time, who
    /// takes access with `acquire_write`) or `exclusive-shared` (as `exclusive`, and typing at
    /// this session's own terminal takes access back). Write it as `--writable=MODE`.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "open", value_name = "MODE", conflicts_with = "no_listen")]
    writable: Option<WritableMode>,

    /// Show the program's output in this terminal but do not send this terminal's keystrokes to
    /// it (socket clients and `attach` can still type).
    #[arg(long)]
    no_stdin: bool,

    /// When the program exits, print the final screen to stderr.
    #[arg(long)]
    dump_on_exit: bool,

    /// Run without using this terminal at all: no input or output forwarding, so the session is
    /// driven through its sockets and `vtee attach`.
    #[arg(long, conflicts_with = "no_listen")]
    headless: bool,

    /// Name this session (default: the program's file name) so `attach`, `ls` and `kill` can find
    /// it. A live session's name can't be reused unless `--attach-if-exists` is set.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,

    /// If a live session already has this name, attach to it instead of failing (like tmux's
    /// `new-session -A`). Overrides the config file's `attach_if_exists`.
    #[arg(long, conflicts_with = "no_attach_if_exists")]
    attach_if_exists: bool,

    /// Fail if the name is taken, even when the config file sets `attach_if_exists = true`.
    #[arg(long, conflicts_with = "attach_if_exists")]
    no_attach_if_exists: bool,

    /// Terminal width in columns, 1-1000. Default: this terminal's width, else 80. Applies to
    /// `--script` too.
    #[arg(long, value_name = "N")]
    cols: Option<u16>,

    /// Terminal height in rows, 1-1000. Default: this terminal's height, else 24. Applies to
    /// `--script` too.
    #[arg(long, value_name = "N")]
    rows: Option<u16>,

    /// Set an environment variable for the program, `KEY=VALUE`; repeat for more. They are added
    /// to the inherited environment, which is not cleared. Applies to `--script` too.
    #[arg(long = "env", value_name = "KEY=VALUE", value_parser = parse_env_pair)]
    env: Vec<(String, String)>,

    /// The program to run and its arguments. Put `--` before it if its name or first argument
    /// could be mistaken for an vtee option. Not needed with `--connect`.
    // Not `required_unless_present`: clap rejects naming a subcommand field there. `main()`
    // checks it instead, after subcommands and `--connect` have already returned.
    #[arg(trailing_var_arg = true, value_name = "PROGRAM")]
    command: Vec<String>,
}

fn main() -> ExitCode {
    // The terminal-restore watchdog re-execs this binary with a hidden flag; handle it before
    // clap so it never shows up in `--help`.
    #[cfg(unix)]
    if std::env::args().nth(1).as_deref() == Some("--internal-tty-guard") {
        vtee::run_tty_guard();
    }

    human_panic::setup_panic!();

    #[cfg(windows)]
    vtee::restore_default_ctrl_c();

    // `Cli::parse()` would exit(2); route usage errors through sysexits `USAGE` like every other
    // error path. `--help`/`--version` also arrive as errors and must still exit 0.
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
            Commands::Replay { file, json } => run_replay(file, *json),
        };
    }

    init_logging(&cli);

    if let Some(path) = &cli.connect {
        return run_connect(path, cli.text);
    }

    // The same bound `resize` enforces: the emulator allocates cols x rows cells.
    for (flag, value) in [("--cols", cli.cols), ("--rows", cli.rows)] {
        if let Some(n) = value
            && !(1..=vtee::MAX_TERMINAL_DIMENSION).contains(&n)
        {
            return fail(
                exitcode::USAGE,
                format!("{flag} {n} is out of range: it must be between 1 and {}", vtee::MAX_TERMINAL_DIMENSION),
            );
        }
    }

    let observed_size = observed_terminal_size();
    let (cols, rows) = (cli.cols.unwrap_or(observed_size.0), cli.rows.unwrap_or(observed_size.1));

    let Some((program, rest)) = cli.command.split_first() else {
        return fail(exitcode::USAGE, "a command is required");
    };

    // Return normally instead of calling `process::exit`, so a live `Tap` drops and kills its
    // child.
    if let Some(script_path) = &cli.script {
        return run_script(
            program,
            rest,
            ScriptOpts { log_path: cli.log.as_deref(), script_path, text_mode: cli.text, cols, rows, env: &cli.env },
        );
    }

    let session = match resolve_session(&cli, program) {
        Ok(session) => session,
        Err(code) => return code,
    };

    run_spawn(
        program,
        rest,
        SpawnArgs {
            pidfile: session.pidfile.as_deref(),
            listen_path: session.listen.as_deref(),
            tty_path: session.tty.as_deref(),
            log_path: cli.log.as_deref(),
            write_policy: cli.writable.map(Into::into),
            no_stdin: cli.no_stdin,
            dump_on_exit: cli.dump_on_exit,
            headless: cli.headless,
            cols,
            rows,
            observed_size,
            env: &cli.env,
        },
    )
}

/// Diagnostics go to stderr, except when this process is showing the child live on a real
/// terminal: stderr would interleave with the child's screen, so they go to a file beside the
/// session's socket instead (`docs/decisions.md`).
fn init_logging(cli: &Cli) {
    let mux_shows_child_live =
        cli.script.is_none() && cli.connect.is_none() && !cli.headless && std::io::stdin().is_terminal();

    let mut builder = env_logger::Builder::new();
    builder.filter_level(cli.verbosity.log_level_filter());
    if mux_shows_child_live {
        match vtee::default_log_path() {
            Ok(path) => match std::fs::File::create(&path) {
                Ok(file) => {
                    print_info(&format!("diagnostic log at {}", path.display()));
                    builder.write_style(env_logger::WriteStyle::Never).target(env_logger::Target::Pipe(Box::new(file)));
                }
                Err(_) => {
                    builder.filter_level(log::LevelFilter::Off);
                }
            },
            // Not worth refusing to run the program; diagnostics are just unavailable.
            Err(_) => {
                builder.filter_level(log::LevelFilter::Off);
            }
        }
    }
    builder.init();
}

/// The files this invocation's session owns; all `None` under `--no-listen`.
#[derive(Default)]
struct SessionAddrs {
    listen: Option<PathBuf>,
    tty: Option<PathBuf>,
    pidfile: Option<PathBuf>,
}

/// Decide the session's name and paths, rejecting a duplicate name. `Err` carries the exit code
/// to return (including the result of attaching, under `--attach-if-exists`).
fn resolve_session(cli: &Cli, program: &str) -> Result<SessionAddrs, ExitCode> {
    // `--no-listen` means no footprint in the session directory: no name, pidfile or sockets.
    if cli.no_listen {
        return Ok(SessionAddrs::default());
    }
    let name = cli.name.clone().unwrap_or_else(|| default_session_name(program));

    match vtee::session_is_live(&name) {
        Ok(true) if attach_if_exists(cli) => return Err(run_attach(Some(&name))),
        Ok(true) => return Err(fail(exitcode::USAGE, format!("duplicate session: {name}"))),
        Ok(false) => {}
        Err(e) => {
            return Err(fail(exitcode::OSERR, format!("couldn't check for an existing session named {name:?}: {e}")));
        }
    }

    let paths = vtee::session_paths(&name)
        .map_err(|e| fail(exitcode::OSERR, format!("couldn't set up session {name:?}: {e}")))?;

    // An explicit `--listen=<path>` replaces the session's own NDJSON socket.
    let explicit_listen = cli.listen.clone().filter(|p| p.as_os_str() != BARE_LISTEN);
    Ok(SessionAddrs {
        listen: Some(explicit_listen.unwrap_or(paths.sock)),
        tty: Some(paths.tty_sock),
        pidfile: Some(paths.pidfile),
    })
}

/// `--attach-if-exists` / `--no-attach-if-exists`, falling back to the config file's default.
fn attach_if_exists(cli: &Cli) -> bool {
    if cli.attach_if_exists {
        return true;
    }
    if cli.no_attach_if_exists {
        return false;
    }
    match confy::load::<VteeConfig>("vtee", None) {
        Ok(config) => config.attach_if_exists,
        Err(e) => {
            print_error(&format!("ignoring unreadable config file: {e}"));
            false
        }
    }
}

/// `vtee ./bin/tf` names the session `tf`, not `./bin/tf`.
fn default_session_name(program: &str) -> String {
    Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program).to_string()
}

/// Splits on the first `=` only, so `--env PATH=/a=b` keeps its `=`.
fn parse_env_pair(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((key, value)) => Ok((key.to_string(), value.to_string())),
        None => Err(format!("expected KEY=VALUE, got {s:?}")),
    }
}

/// The real terminal's size, or 80x24 when there is none or it reports 0x0 (seen under
/// `docker run -t` before a size is negotiated).
fn observed_terminal_size() -> (u16, u16) {
    if std::io::stdout().is_terminal() {
        match crossterm::terminal::size() {
            Ok((cols, rows)) if cols > 0 && rows > 0 => (cols, rows),
            _ => (80, 24),
        }
    } else {
        (80, 24)
    }
}

struct SpawnArgs<'a> {
    /// `None` under `--no-listen`: no session files at all.
    pidfile: Option<&'a Path>,
    log_path: Option<&'a Path>,
    listen_path: Option<&'a Path>,
    /// The raw-TTY socket for `vtee attach`. This process's own display and input still go
    /// through `forward_input`/`forward_output` directly.
    tty_path: Option<&'a Path>,
    write_policy: Option<vtee::WritePolicy>,
    no_stdin: bool,
    dump_on_exit: bool,
    headless: bool,
    cols: u16,
    rows: u16,
    /// The real terminal's size when `cols`/`rows` were chosen; `--cols/--rows` may override it.
    observed_size: (u16, u16),
    env: &'a [(String, String)],
}

/// Removes a session's files when dropped, whichever way `run_spawn` returns.
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

fn run_spawn(program: &str, rest: &[String], cli: SpawnArgs) -> ExitCode {
    // `--headless` turns both forwards off: no human is attached, the AI uses the sockets.
    let opts = vtee::SpawnOptions {
        cols: cli.cols,
        rows: cli.rows,
        log_path: cli.log_path,
        forward_input: !cli.no_stdin && !cli.headless,
        forward_output: !cli.headless,
        env: cli.env,
    };

    // Raw mode *before* the spawn, not after: `Tap::spawn` starts the stdin-forwarding thread,
    // whose first blocking read must not begin while the console is still in cooked mode. On
    // Windows a cooked read stays cooked after the mode changes, and it eats the terminal's
    // `ESC[row;colR` reply to ConPTY's startup cursor query - a hang on roughly every other run
    // (`docs/decisions.md`, 2026-10-08). Dropped again before any error is printed.
    let mut raw_guard = if cli.headless { None } else { Some(vtee::RawModeGuard::new()) };

    let tap = match vtee::Tap::spawn(program, rest, opts) {
        Ok(t) => Arc::new(t),
        Err(e) => {
            raw_guard.take();
            return fail(exitcode::OSERR, format!("failed to spawn {program}: {e}"));
        }
    };

    // Built before anything is bound, so any early return below still removes what got created.
    let _session_files = SessionFiles { pidfile: cli.pidfile, listen_path: cli.listen_path, tty_path: cli.tty_path };

    // With a real terminal here, it alone owns the pty's size, so a remote resize would fight it.
    let resize_owned_by_terminal = !cli.headless && std::io::stdin().is_terminal();

    if !cli.headless {
        tap.watch_resize(cli.observed_size);
    }
    let session_files: Vec<PathBuf> =
        [cli.pidfile, cli.listen_path, cli.tty_path].into_iter().flatten().map(Path::to_path_buf).collect();
    tap.watch_sigterm_removing(&session_files);

    if let Some(path) = cli.listen_path {
        let write_policy = cli.write_policy.unwrap_or(vtee::WritePolicy::ReadOnly);
        let serve_opts = vtee::ServeOptions { write_policy, resize_owned_by_terminal };
        if let Err(e) = vtee::Tap::serve(tap.clone(), path, serve_opts) {
            // Restore the terminal before printing, since the child may already be drawing.
            raw_guard.take();
            return fail(exitcode::OSERR, format!("--listen failed: {e}"));
        }
        log::info!("listening for observers ({write_policy:?}) at {}", path.display());
    }

    if let Some(path) = cli.tty_path {
        // Always `Open`: the raw protocol has no framing for an acquire/release handshake.
        if let Err(e) = vtee::Tap::serve_tty(tap.clone(), path, vtee::WritePolicy::Open) {
            raw_guard.take();
            return fail(exitcode::OSERR, format!("couldn't bind the attach socket: {e}"));
        }
    }

    // Written last, so `ls`/`kill`/`attach` find this process only once its sockets exist, and
    // with the paths actually bound: `--listen=<path>` puts the NDJSON socket elsewhere.
    if let Some(pidfile) = cli.pidfile
        && let (Some(sock), Some(tty)) = (cli.listen_path, cli.tty_path)
        && let Err(e) = vtee::write_session_record(pidfile, sock, tty)
    {
        raw_guard.take();
        return fail(exitcode::OSERR, format!("couldn't write pidfile {}: {e}", pidfile.display()));
    }
    if let Some(pidfile) = cli.pidfile {
        tap.set_activity_file(pidfile);
    }

    let status = tap.wait();
    drop(raw_guard);

    match status {
        Ok(status) => {
            if cli.dump_on_exit {
                dump_final_state(&tap, &status);
            }
            // Propagate the child's own exit code, the way `env`/`time` do.
            if status.success() { ExitCode::SUCCESS } else { ExitCode::from((status.exit_code() & 0xff) as u8) }
        }
        Err(e) => fail(exitcode::OSERR, format!("error waiting for child: {e}")),
    }
}

fn dump_final_state(tap: &vtee::Tap, status: &portable_pty::ExitStatus) {
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
    eprintln!("cursor={:?} modes={:?} alt_screen={} title={:?}", snap.cursor, snap.modes, snap.alt_screen, snap.title);
}
