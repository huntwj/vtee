//! Terminal messages and exit codes shared by the CLI modules.

use std::process::ExitCode;

use owo_colors::{OwoColorize, Stream::Stderr};

/// `exitcode`'s sysexits constants are untyped `i32`; this is the one conversion point.
pub fn exit_code(code: exitcode::ExitCode) -> ExitCode {
    ExitCode::from(code as u8)
}

/// A fatal message, printed straight to stderr rather than through the file-backed logger,
/// since the user must see it even when logging is redirected.
pub fn print_error(msg: &str) {
    eprintln!("{}", format!("vtee: {msg}").if_supports_color(Stderr, |t| t.red().to_string()));
}

/// A pre-session notice; only safe on stderr before raw mode starts.
pub fn print_info(msg: &str) {
    eprintln!("{}", format!("vtee: {msg}").if_supports_color(Stderr, |t| t.dimmed().to_string()));
}

/// Print `msg` as an error and return the matching exit code.
pub fn fail(code: exitcode::ExitCode, msg: impl AsRef<str>) -> ExitCode {
    print_error(msg.as_ref());
    exit_code(code)
}
