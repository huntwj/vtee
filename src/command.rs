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
/// every `snapshot` or `wait_for` command, nothing for the others.
#[derive(Debug, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum CommandResult {
    Snapshot { snapshot: Snapshot },
    WaitFor { matched: bool, snapshot: Snapshot },
}

impl Command {
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
