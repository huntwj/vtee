use std::io::IsTerminal;
use std::path::PathBuf;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut log_path: Option<PathBuf> = None;
    if let Some(pos) = args.iter().position(|a| a == "--log") {
        if pos + 1 >= args.len() {
            eprintln!("vtee: --log needs a path argument");
            std::process::exit(2);
        }
        let path = args.remove(pos + 1);
        args.remove(pos);
        log_path = Some(PathBuf::from(path));
    }

    let args: &[String] = if args.first().map(String::as_str) == Some("--") { &args[1..] } else { &args[..] };

    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: vtee [--log <path>] [--] <command> [args...]");
        std::process::exit(2);
    };

    // crossterm can report a real tty's size as 0x0 when nothing has actually negotiated a
    // size yet (observed via `docker run -t` without `-i`); that's never a usable screen size,
    // so it gets the same fallback as not having a tty at all.
    let (cols, rows) = if std::io::stdout().is_terminal() {
        match crossterm::terminal::size() {
            Ok((cols, rows)) if cols > 0 && rows > 0 => (cols, rows),
            _ => (80, 24),
        }
    } else {
        (80, 24)
    };

    let mut tap = match vtee::Tap::spawn(program, rest, cols, rows, log_path.as_deref()) {
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
