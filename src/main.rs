use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    // The watchdog re-execs this same binary with this hidden flag (`vtee::spawn_tty_watchdog`
    // via `RawModeGuard`) - handle it before anything else, since it's a different process
    // entirely, not a mode of the CLI below.
    #[cfg(unix)]
    if args.first().map(String::as_str) == Some("--internal-tty-guard") {
        vtee::run_tty_guard();
    }

    let text_mode = take_switch(&mut args, "--text");

    // `--connect` has no `<program>` to spawn - it's a client for someone else's `--listen`,
    // not a variant of running a program - so it's handled before the usual argument shape is
    // even assumed.
    if let Some(path) = take_path_flag(&mut args, "--connect") {
        return run_connect(&path, text_mode);
    }

    let log_path = take_path_flag(&mut args, "--log");
    let script_path = take_path_flag(&mut args, "--script");
    let listen_path = take_path_flag(&mut args, "--listen");
    // Stage 3a ("Full duplex", `docs/roadmap.md`): opt-in, default off, matching this project's
    // pattern everywhere else of defaulting to the narrower/safer behaviour. Meaningless
    // without `--listen`, so no separate validation for that - `--writable` alone just does
    // nothing.
    let writable = take_switch(&mut args, "--writable");

    let args: &[String] = if args.first().map(String::as_str) == Some("--") { &args[1..] } else { &args[..] };

    let Some((program, rest)) = args.split_first() else {
        eprintln!(
            "usage: vtee [--log <path>] [--listen <path> [--writable]] [--] <command> [args...]\n       \
             vtee --script <path> [--text] [--log <path>] [--] <command> [args...]\n       \
             vtee --connect <path> [--text]"
        );
        return exit_code(exitcode::USAGE);
    };

    // Both functions below return normally with their result rather than calling
    // `std::process::exit` themselves, specifically so every local (including a spawned `Tap`)
    // drops before this function's own return does - calling `std::process::exit` while a
    // `Tap` is still alive would skip its child-killing `Drop` and orphan the child.
    match script_path {
        Some(script_path) => run_script(program, rest, log_path.as_deref(), &script_path, text_mode),
        None => run_interactive(program, rest, log_path.as_deref(), listen_path.as_deref(), writable),
    }
}

/// `exitcode`'s constants are untyped `i32` (sysexits.h values, all small and non-negative),
/// not the `std::process::ExitCode` our functions actually return - this is the one place that
/// conversion happens.
fn exit_code(code: exitcode::ExitCode) -> ExitCode {
    ExitCode::from(code as u8)
}

/// Resolve the real terminal size, falling back to 80x24 when there isn't a usable one -
/// either no tty at all, or a tty that hasn't negotiated a real size yet (`crossterm` can
/// report `0x0` for that, observed via `docker run -t` without `-i`; never a usable size).
fn terminal_size() -> (u16, u16) {
    if std::io::stdout().is_terminal() {
        match crossterm::terminal::size() {
            Ok((cols, rows)) if cols > 0 && rows > 0 => return (cols, rows),
            _ => {}
        }
    }
    (80, 24)
}

fn run_interactive(
    program: &str,
    rest: &[String],
    log_path: Option<&Path>,
    listen_path: Option<&Path>,
    writable: bool,
) -> ExitCode {
    let (cols, rows) = terminal_size();
    let opts = vtee::SpawnOptions { cols, rows, log_path, interactive: true };

    // `Arc` even when `--listen` isn't given: `Tap::serve` needs one, and every method it
    // would otherwise need `&Tap` for already takes `&self`, so this costs one allocation and
    // avoids branching the rest of this function on whether observation is in use.
    let tap = match vtee::Tap::spawn(program, rest, opts) {
        Ok(t) => Arc::new(t),
        Err(e) => {
            eprintln!("vtee: failed to spawn {program}: {e}");
            return exit_code(exitcode::OSERR);
        }
    };

    // The same check `RawModeGuard`/`watch_resize` act on: if there's a real terminal here,
    // it's the one real screen size that means anything, so a remote `Resize` would just fight
    // with it (`Command::authorize`, `docs/roadmap.md`).
    let resize_owned_by_terminal = std::io::stdin().is_terminal();

    let _raw_guard = vtee::RawModeGuard::new();
    tap.watch_resize();
    tap.watch_sigterm();

    if let Some(path) = listen_path {
        let write_policy = if writable { vtee::WritePolicy::Open } else { vtee::WritePolicy::ReadOnly };
        let serve_opts = vtee::ServeOptions { write_policy, resize_owned_by_terminal };
        if let Err(e) = vtee::Tap::serve(tap.clone(), path, serve_opts) {
            eprintln!("vtee: --listen failed: {e}");
            return exit_code(exitcode::OSERR);
        }
        eprintln!(
            "vtee: listening for {} at {}",
            if writable { "observers (writable)" } else { "observers (read-only)" },
            path.display()
        );
    }

    let status = tap.wait();
    drop(_raw_guard);

    match status {
        Ok(status) => {
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
            // Propagate the child's own exit code rather than collapsing every failure to a
            // fixed value - a caller scripting around vtee (CI, TF's harness) likely cares
            // which code the wrapped program actually exited with, the way `env`/`time` do.
            if status.success() { ExitCode::SUCCESS } else { ExitCode::from((status.exit_code() & 0xff) as u8) }
        }
        Err(e) => {
            eprintln!("vtee: error waiting for child: {e}");
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
fn run_script(
    program: &str,
    rest: &[String],
    log_path: Option<&Path>,
    script_path: &Path,
    text_mode: bool,
) -> ExitCode {
    let (cols, rows) = terminal_size();
    let opts = vtee::SpawnOptions { cols, rows, log_path, interactive: false };

    let tap = match vtee::Tap::spawn(program, rest, opts) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("vtee: failed to spawn {program}: {e}");
            return exit_code(exitcode::OSERR);
        }
    };

    tap.watch_sigterm();

    let code = run_script_body(&tap, script_path, text_mode);

    tap.kill();
    let _ = tap.wait();
    code
}

fn run_script_body(tap: &vtee::Tap, script_path: &Path, text_mode: bool) -> ExitCode {
    let script = match std::fs::File::open(script_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("vtee: failed to open script {}: {e}", script_path.display());
            return exit_code(exitcode::NOINPUT);
        }
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for (lineno, line) in std::io::BufReader::new(script).lines().enumerate() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                eprintln!("vtee: error reading script line {}: {e}", lineno + 1);
                return exit_code(exitcode::IOERR);
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let cmd: vtee::Command = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("vtee: bad command on script line {}: {e}", lineno + 1);
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
                    eprintln!("vtee: failed to serialize result on script line {}: {e}", lineno + 1);
                    return exit_code(exitcode::SOFTWARE);
                }
                let _ = out.flush();
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("vtee: command on script line {} failed: {e}", lineno + 1);
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
            eprintln!("vtee: failed to connect to {}: {e}", path.display());
            return exit_code(exitcode::NOHOST);
        }
    };
    let mut writer = match stream.try_clone() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("vtee: {e}");
            return exit_code(exitcode::OSERR);
        }
    };
    let Ok(reader_side) = stream.try_clone() else {
        eprintln!("vtee: failed to clone the connection for reading");
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
                    Err(e) => eprintln!("vtee: malformed response from server: {e}"),
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
    eprintln!("vtee: --connect needs Unix (requirement 8 defers Windows)");
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
    };
    let _ = writeln!(out, "--- {label} (byte_offset={}) ---", snapshot.byte_offset);
    for line in &snapshot.lines {
        let _ = writeln!(out, "{}", line.trim_end());
    }
}

/// Removes `flag <value>` from `args` if present and returns `value` as a path, exiting with
/// a usage error if the flag is given with no value. Runs before any `Tap` exists, so a direct
/// `process::exit` here (rather than returning up to `main`'s own `ExitCode`) is fine - there's
/// nothing yet that needs its `Drop` to run first.
fn take_path_flag(args: &mut Vec<String>, flag: &str) -> Option<PathBuf> {
    let pos = args.iter().position(|a| a == flag)?;
    if pos + 1 >= args.len() {
        eprintln!("vtee: {flag} needs a path argument");
        std::process::exit(exitcode::USAGE);
    }
    let value = args.remove(pos + 1);
    args.remove(pos);
    Some(PathBuf::from(value))
}

/// Removes a no-value flag from `args` if present, returning whether it was there.
fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    match args.iter().position(|a| a == flag) {
        Some(pos) => {
            args.remove(pos);
            true
        }
        None => false,
    }
}
