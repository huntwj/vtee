use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let log_path = take_path_flag(&mut args, "--log");
    let script_path = take_path_flag(&mut args, "--script");

    let args: &[String] = if args.first().map(String::as_str) == Some("--") { &args[1..] } else { &args[..] };

    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: vtee [--log <path>] [--script <path>] [--] <command> [args...]");
        std::process::exit(2);
    };

    match script_path {
        Some(script_path) => run_script(program, rest, log_path.as_deref(), &script_path),
        None => run_interactive(program, rest, log_path.as_deref()),
    }
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

fn run_interactive(program: &str, rest: &[String], log_path: Option<&std::path::Path>) {
    let (cols, rows) = terminal_size();
    let opts = vtee::SpawnOptions { cols, rows, log_path, interactive: true };

    let mut tap = match vtee::Tap::spawn(program, rest, opts) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("vtee: failed to spawn {program}: {e}");
            std::process::exit(1);
        }
    };

    let _raw_guard = vtee::RawModeGuard::new();
    tap.watch_resize();

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
        }
        Err(e) => eprintln!("vtee: error waiting for child: {e}"),
    }
}

/// The batch CLI TinyFugue asked for (`docs/roadmap.md`): run a newline-delimited-JSON script
/// of `Command`s against a non-interactive tap - no stdin forwarding, no raw passthrough to
/// our own stdout, nothing the human-terminal case needs - and write one `CommandResult` line
/// to stdout per `snapshot`/`wait_for` command.
fn run_script(program: &str, rest: &[String], log_path: Option<&std::path::Path>, script_path: &std::path::Path) {
    let (cols, rows) = terminal_size();
    let opts = vtee::SpawnOptions { cols, rows, log_path, interactive: false };

    let tap = match vtee::Tap::spawn(program, rest, opts) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("vtee: failed to spawn {program}: {e}");
            std::process::exit(1);
        }
    };

    let script = match std::fs::File::open(script_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("vtee: failed to open script {}: {e}", script_path.display());
            std::process::exit(1);
        }
    };

    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for (lineno, line) in std::io::BufReader::new(script).lines().enumerate() {
        let line = line.unwrap_or_else(|e| {
            eprintln!("vtee: error reading script line {}: {e}", lineno + 1);
            std::process::exit(1);
        });
        if line.trim().is_empty() {
            continue;
        }
        let cmd: vtee::Command = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("vtee: bad command on script line {}: {e}", lineno + 1);
                std::process::exit(1);
            }
        };
        match cmd.execute(&tap) {
            Ok(Some(result)) => {
                let _ = writeln!(out, "{}", serde_json::to_string(&result).unwrap());
                let _ = out.flush();
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("vtee: command on script line {} failed: {e}", lineno + 1);
                std::process::exit(1);
            }
        }
    }
}

/// Removes `flag <value>` from `args` if present and returns `value` as a path, exiting with
/// a usage error if the flag is given with no value.
fn take_path_flag(args: &mut Vec<String>, flag: &str) -> Option<PathBuf> {
    let pos = args.iter().position(|a| a == flag)?;
    if pos + 1 >= args.len() {
        eprintln!("vtee: {flag} needs a path argument");
        std::process::exit(2);
    }
    let value = args.remove(pos + 1);
    args.remove(pos);
    Some(PathBuf::from(value))
}
