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
    /// Stage 3b's "take the pen" handoff (`WritePolicy::Exclusive`, `docs/roadmap.md`): ask to
    /// become the one connection allowed to `Send`/`SendKey`/`Resize` right now. Meaningless
    /// (and rejected by `authorize`) under any other policy.
    AcquireWrite,
    /// Give up write access explicitly, rather than only ever losing it by disconnecting.
    ReleaseWrite,
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

/// Under `WritePolicy::Exclusive`, whether the interactive stdin-forwarding thread's own
/// writes participate in the same lock remote `--connect` clients compete for
/// (`docs/roadmap.md`, "I think that's just one more setting/option, right?" - a setting, not
/// a fixed answer). Human keystrokes are never delayed or blocked by this either way - the
/// only question is whether typing *also* revokes whatever remote client currently holds
/// write access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HumanPriority {
    /// The human's typing always reaches the child, and the lock only arbitrates among remote
    /// clients - a remote holder keeps write access regardless of what the human does.
    AlwaysWins,
    /// The human's typing always reaches the child *and* revokes whatever remote client
    /// currently holds write access, the instant a keystroke arrives - no idle-time guessing
    /// (requirement 4 already rejected that shape of heuristic for output; this avoids it for
    /// input too by reacting to a real event, not an inferred one). A remote client finds out
    /// by its next command failing, and can reacquire once the human stops "holding" it.
    Participant,
}

/// How a `Tap::serve` connection may write, if at all (`docs/roadmap.md`, "Full duplex"). An
/// enum to grow into, not a single bool: the user decided the write policies should all be
/// selectable modes rather than this project picking one fixed answer, and the same for
/// `Exclusive`'s own `HumanPriority` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// No `Send`/`SendKey`/`Resize` at all - observation's (stage 2) original behaviour, and
    /// the default.
    ReadOnly,
    /// Any connected writer may inject at any time, no arbitration between writers. Sound
    /// whenever nothing else is also writing (stage 3a's only case - no human competing), and
    /// one of several modes once a human is also typing (stage 3b).
    Open,
    /// "Take the pen": only one connection may hold write access at a time, via
    /// `Command::AcquireWrite`/`ReleaseWrite`, auto-released on disconnect.
    Exclusive { human_priority: HumanPriority },
}

impl Command {
    /// Whether `self` is structurally reachable under `policy`, given whether the real
    /// terminal (if any) is actively managing the pty's size via `Tap::watch_resize`. This is
    /// the policy-shaped check only - `WritePolicy::Exclusive` additionally requires the
    /// *caller* to currently hold the lock for `Send`/`SendKey`/`Resize`, which isn't checked
    /// here at all (this function has no notion of which connection is asking) but by
    /// `Tap::serve`'s connection handler, which does.
    ///
    /// `Resize` is checked against `resize_owned_by_terminal` independently of `policy`: a
    /// remote resize would fight with a human's own terminal resizing the session regardless
    /// of which write policy is otherwise in effect, since there's only one real screen size
    /// that means anything once a human is looking at one.
    pub fn authorize(&self, policy: WritePolicy, resize_owned_by_terminal: bool) -> Result<(), String> {
        match self {
            Command::Snapshot | Command::WaitFor { .. } | Command::WaitMs { .. } => Ok(()),
            Command::AcquireWrite | Command::ReleaseWrite => match policy {
                WritePolicy::Exclusive { .. } => Ok(()),
                WritePolicy::ReadOnly | WritePolicy::Open => {
                    Err("acquire_write/release_write only mean something under the exclusive write policy".to_string())
                }
            },
            Command::Resize { .. } if resize_owned_by_terminal => {
                Err("resize is controlled by the real terminal on this session - a remote resize would fight with it"
                    .to_string())
            }
            Command::Send { .. } | Command::SendKey { .. } | Command::Resize { .. } => match policy {
                WritePolicy::ReadOnly => {
                    Err("write commands aren't allowed on this connection - start the server with --writable"
                        .to_string())
                }
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
            // Handled entirely by `Tap::serve`'s connection handler, which is the only place
            // that knows which connection is asking - `execute` never sees these reach here in
            // practice (`authorize` only lets them through under `Exclusive`, and the
            // connection handler intercepts them before calling `execute` at all), but they
            // need *some* arm, and silently doing nothing is more honest than a `todo!()` that
            // would panic a connection handler thread over a command that just means "this
            // path isn't how these are handled."
            Command::AcquireWrite | Command::ReleaseWrite => Ok(None),
        }
    }
}
