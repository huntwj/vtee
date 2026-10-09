//! The command vocabulary shared by the batch CLI (script files) and the socket protocol, so
//! the two never drift apart.

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
    /// Stage 3b's "take the pen" handoff (`WritePolicy::Exclusive`, `docs/roadmap.md`): ask to
    /// become the one connection allowed to `Send`/`SendKey`/`Resize` right now. Meaningless
    /// (and rejected by `authorize`) under any other policy.
    AcquireWrite,
    /// Give up write access explicitly, rather than only ever losing it by disconnecting.
    ReleaseWrite,
}

/// One line on the wire per `snapshot`/`wait_for` command, nothing for the others. `Error` is
/// produced only by a driver such as the connection handler, never by `execute`: a long-lived
/// connection reports a bad command inline and keeps going, while the batch CLI treats one as
/// fatal.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum CommandResult {
    Snapshot {
        snapshot: Snapshot,
    },
    WaitFor {
        matched: bool,
        snapshot: Snapshot,
    },
    /// Reply to `AcquireWrite`/`ReleaseWrite` - `acquired` is the lock's state *after* this
    /// request, so a losing `AcquireWrite` (someone else already holds it) still gets a
    /// well-formed reply (`acquired: false`), not an `Error`.
    WriteAccess {
        acquired: bool,
    },
    Error {
        message: String,
    },
}

/// Under `WritePolicy::Exclusive`, whether a human's typing also revokes a remote holder.
/// Human keystrokes are never delayed either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanPriority {
    /// The lock only arbitrates among remote clients; a remote holder keeps access.
    AlwaysWins,
    /// Each keystroke revokes the current remote holder, whose next command then fails; it can
    /// reacquire afterwards. Reacts to a real event rather than guessing at idleness.
    Participant,
}

/// How a connection may write, if at all (`docs/roadmap.md`, "Full duplex"). Every policy is a
/// selectable mode rather than one fixed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// No `Send`/`SendKey`/`Resize` at all. The default.
    ReadOnly,
    /// Any connected writer may inject at any time, with no arbitration.
    Open,
    /// "Take the pen": only one connection may hold write access at a time, via
    /// `Command::AcquireWrite`/`ReleaseWrite`, auto-released on disconnect.
    Exclusive { human_priority: HumanPriority },
}

/// Why `Command::authorize` refused a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    NotExclusivePolicy,
    ResizeOwnedByTerminal,
    ReadOnly,
}

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Denied::NotExclusivePolicy => {
                "acquire_write/release_write only mean something under the exclusive write policy"
            }
            Denied::ResizeOwnedByTerminal => {
                "resize is controlled by the real terminal on this session - a remote resize would fight with it"
            }
            Denied::ReadOnly => "write commands aren't allowed on this connection - start the server with --writable",
        })
    }
}

impl Command {
    /// Whether `self` is allowed under `policy`. Under `Exclusive` it does not check that the
    /// caller holds the lock; this function doesn't know which connection is asking, so the
    /// connection handler does. `Resize` is rejected whenever a real terminal owns the pty's
    /// size, whatever the policy, since a remote resize would fight with it.
    pub fn authorize(&self, policy: WritePolicy, resize_owned_by_terminal: bool) -> Result<(), Denied> {
        match self {
            Command::Snapshot | Command::WaitFor { .. } | Command::WaitMs { .. } => Ok(()),
            Command::AcquireWrite | Command::ReleaseWrite => match policy {
                WritePolicy::Exclusive { .. } => Ok(()),
                WritePolicy::ReadOnly | WritePolicy::Open => Err(Denied::NotExclusivePolicy),
            },
            Command::Resize { .. } if resize_owned_by_terminal => Err(Denied::ResizeOwnedByTerminal),
            Command::Send { .. } | Command::SendKey { .. } | Command::Resize { .. } => match policy {
                WritePolicy::ReadOnly => Err(Denied::ReadOnly),
                WritePolicy::Open | WritePolicy::Exclusive { .. } => Ok(()),
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
            // The connection handler intercepts these; only it knows which connection is asking.
            Command::AcquireWrite | Command::ReleaseWrite => Ok(None),
        }
    }
}
