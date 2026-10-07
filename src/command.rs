//! The shared command vocabulary (`docs/roadmap.md`, "Now: a shared command core"). The batch
//! CLI (`src/main.rs`) drives these from a script file; a later live protocol would drive the
//! same enum from a socket instead - this is deliberately the one place that vocabulary is
//! defined, so the two never drift apart.

use std::io;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Snapshot, Tap};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Raw text, sent as-is (e.g. `"hello\r"`). Encoded as a JSON string, so arbitrary bytes
    /// are reachable via `\u00XX` escapes; this covers everything a test script realistically
    /// needs to type or send as a control sequence.
    Send {
        text: String,
    },
    /// A named key (`src/keys.rs`) - arrows, Home/End/PgUp/PgDn, F1-F12, `Ctrl-x`, `Alt-x`.
    SendKey {
        key: String,
    },
    Resize {
        cols: u16,
        rows: u16,
    },
    WaitMs {
        ms: u64,
    },
    /// Blocks until the screen's text contains `text`, or `timeout_ms` elapses - the "text
    /// appears" case of wait-for-predicate that TinyFugue actually asked for (`roadmap.md`).
    WaitFor {
        text: String,
        timeout_ms: u64,
    },
    Snapshot,
}

/// What a command that produces output looks like on the wire - one of these per line for
/// every `snapshot` or `wait_for` command, nothing for the others. `Error` is only ever
/// produced by a driver (e.g. `Tap::serve`'s connection handler), never by `execute` itself -
/// a long-lived connection needs to report a bad command inline and keep going, where the
/// batch CLI's `execute` contract is "the caller decides what a failure means" (it treats one
/// as fatal to the whole script, `main.rs`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum CommandResult {
    Snapshot { snapshot: Snapshot },
    WaitFor { matched: bool, snapshot: Snapshot },
    Error { message: String },
}

/// How a `Tap::serve` connection may write, if at all (`docs/roadmap.md`, "Full duplex"). An
/// enum to grow into, not a single bool: the user decided the remaining policies (exclusive
/// "take the pen" handoff, mutually-exclusive-with-the-human) should all be selectable modes
/// rather than this project picking one fixed answer, so this type is where those land later -
/// only the modes stage 3a actually needs (no human competing) exist so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// No `Send`/`SendKey`/`Resize` at all - observation's (stage 2) original behaviour, and
    /// the default.
    ReadOnly,
    /// Any connected writer may inject at any time, no arbitration between writers. Only sound
    /// when nothing else is also writing (stage 3a) - once a human is also typing (stage 3b),
    /// this is one of several modes to choose from, not the only option.
    Open,
}

impl Command {
    /// Whether `self` is allowed under `policy`, given whether the real terminal (if any) is
    /// actively managing the pty's size via `Tap::watch_resize`. `Resize` is checked against
    /// that independently of `policy`: a remote resize would fight with a human's own terminal
    /// resizing the session regardless of which write policy is otherwise in effect, since
    /// there's only one real screen size that means anything once a human is looking at one.
    pub fn authorize(&self, policy: WritePolicy, resize_owned_by_terminal: bool) -> Result<(), String> {
        match self {
            Command::Snapshot | Command::WaitFor { .. } | Command::WaitMs { .. } => Ok(()),
            Command::Resize { .. } if resize_owned_by_terminal => {
                Err("resize is controlled by the real terminal on this session - a remote resize would fight with it"
                    .to_string())
            }
            Command::Send { .. } | Command::SendKey { .. } | Command::Resize { .. } => match policy {
                WritePolicy::ReadOnly => {
                    Err("write commands aren't allowed on this connection - start the server with --writable"
                        .to_string())
                }
                WritePolicy::Open => Ok(()),
            },
        }
    }

    pub fn execute(&self, tap: &Tap) -> io::Result<Option<CommandResult>> {
        match self {
            Command::Send { text } => {
                tap.send_text(text)?;
                Ok(None)
            }
            Command::SendKey { key } => {
                tap.send_key(key)?;
                Ok(None)
            }
            Command::Resize { cols, rows } => {
                tap.resize(*cols, *rows)?;
                Ok(None)
            }
            Command::WaitMs { ms } => {
                std::thread::sleep(Duration::from_millis(*ms));
                Ok(None)
            }
            Command::WaitFor { text, timeout_ms } => {
                let (matched, snapshot) = tap.wait_for_text(text, Duration::from_millis(*timeout_ms));
                Ok(Some(CommandResult::WaitFor { matched, snapshot }))
            }
            Command::Snapshot => Ok(Some(CommandResult::Snapshot { snapshot: tap.snapshot() })),
        }
    }
}
