//! `vtee` as a library: run a program in a pseudo-terminal, keep an emulator watching its
//! output, and read the screen back as structured data.
//!
//! Most people use the `vtee` command (see the project's `docs/guide/`); this crate is what the
//! command is built on, for Rust programs that want the same thing in-process.
//!
//! # Status
//!
//! Pre-1.0: the API may change in any `0.x` release. The crate is not published to crates.io
//! (`publish = false`); depend on it by git, ideally pinned to a commit or tag:
//!
//! ```toml
//! vtee = { git = "https://github.com/huntwj/vtee", tag = "v0.1.0" }
//! ```
//!
//! The JSON a [`Snapshot`] serialises to carries two numbers: [`SCHEMA_VERSION`] for the layout
//! (renames and removals change it, additive fields do not) and [`SEMANTICS_VERSION`] for what the
//! values mean (an emulation fix or a newly reported name changes it).
//!
//! # Example
//!
//! ```no_run
//! use std::time::Duration;
//! use vtee::{SpawnOptions, Tap};
//!
//! // No human terminal is involved: keep the child's input and output inside the emulator.
//! let opts = SpawnOptions { cols: 80, rows: 24, forward_input: false, forward_output: false, ..Default::default() };
//! let tap = Tap::spawn("sh", &["-c".to_string(), "echo hello; read line; echo got $line".to_string()], opts)?;
//!
//! let (found, snapshot) = tap.wait_for_text("hello", Duration::from_secs(5));
//! assert!(found, "screen was:\n{}", snapshot.lines.join("\n"));
//!
//! tap.send_text("world\r")?;
//! let (found, snapshot) = tap.wait_for_text("got world", Duration::from_secs(5));
//! assert!(found);
//! println!("{}", snapshot.lines.join("\n"));
//!
//! let status = tap.wait()?;
//! assert!(status.success());
//! # Ok::<(), std::io::Error>(())
//! ```
//!
//! # What is public
//!
//! - [`Tap`] and [`SpawnOptions`]: spawn a program, [`Tap::snapshot`], [`Tap::wait_for_text`],
//!   [`Tap::send_text`], [`Tap::send_key`] and [`Tap::resize`], plus [`Tap::exit_status`] and
//!   [`Tap::wait_exit`] for the child's fate.
//! - [`Snapshot`], [`Cell`], [`Cursor`], [`Color`], [`UnderlineStyle`] and [`redraw_sequence`]: the
//!   screen as data, and as the escape sequences that redraw it.
//! - [`Command`], [`CommandResult`], [`WritePolicy`]: the NDJSON protocol the command-line tool
//!   speaks over its sockets; [`Tap::serve`] and [`Tap::serve_tty`] serve a `Tap` with it.
//! - [`RawLog`] and [`replay`]: record a session's raw output (asciicast v2) and replay it into an
//!   emulator.
//! - [`session_paths`], [`session_is_live`], [`session_names`] and friends: where named sessions
//!   live, for tools that want to find them.
//!
//! Everything else (terminal-restore helpers, the socket servers' internals) is an implementation
//! detail of the command-line tool and may change without notice.

mod command;
pub mod golden;
mod keys;
mod log;
pub mod ndjson;
pub mod net;
mod rawmode;
mod serve;
mod session;
pub mod snapshot;
mod sockets;
mod tap;

pub use command::{Command, CommandResult, Denied, HumanPriority, WritePolicy};
pub use log::{RawLog, replay};
pub use rawmode::RawModeGuard;
#[cfg(unix)]
pub use rawmode::run_tty_guard;
pub use serve::ServeOptions;
pub use session::{
    SessionPaths, SessionRecord, default_log_path, session_is_live, session_names, session_paths, session_record,
    write_session_record,
};
pub use snapshot::{
    Cell, Color, Cursor, CursorShape, EMULATOR, Producer, SCHEMA_VERSION, SEMANTICS_VERSION, Snapshot, UnderlineStyle,
    redraw_sequence,
};
#[cfg(windows)]
pub use tap::restore_default_ctrl_c;
pub use tap::{MAX_TERMINAL_DIMENSION, SpawnOptions, Tap};

pub(crate) use tap::{ObserverListener, Size};
