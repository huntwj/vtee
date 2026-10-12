//! Serving a `Tap` to other processes: the public entry points and the policy types. The Unix
//! socket implementation lives in `sockets.rs`.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::tap::LockExt;
use crate::{Tap, WritePolicy};

/// `Tap::serve`'s policy knobs. `Copy` because it is handed to every per-connection thread.
#[derive(Clone, Copy)]
pub struct ServeOptions {
    pub write_policy: WritePolicy,
    /// Whether the real terminal is managing the pty's size via `Tap::watch_resize`
    /// (see `Command::authorize`).
    pub resize_owned_by_terminal: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        ServeOptions { write_policy: WritePolicy::ReadOnly, resize_owned_by_terminal: false }
    }
}

/// Backs `WritePolicy::Exclusive`: at most one connection id holds write access at a time.
/// Shared by both protocols' connection threads and, under `HumanPriority::Participant`, the
/// stdin-forwarding thread.
#[derive(Default)]
pub(crate) struct ExclusiveLock {
    holder: Mutex<Option<u64>>,
}

impl ExclusiveLock {
    pub(crate) fn try_acquire(&self, id: u64) -> bool {
        let mut holder = self.holder.lock_unpoisoned();
        match *holder {
            Some(h) if h == id => true,
            Some(_) => false,
            None => {
                *holder = Some(id);
                true
            }
        }
    }

    pub(crate) fn release_if_held_by(&self, id: u64) {
        let mut holder = self.holder.lock_unpoisoned();
        if *holder == Some(id) {
            *holder = None;
        }
    }

    pub(crate) fn is_held_by(&self, id: u64) -> bool {
        *self.holder.lock_unpoisoned() == Some(id)
    }

    /// Whether connection `id` may write under `policy`; the one place both protocols decide it.
    pub(crate) fn permits(&self, policy: WritePolicy, id: u64) -> bool {
        match policy {
            WritePolicy::ReadOnly => false,
            WritePolicy::Open => true,
            WritePolicy::Exclusive { .. } => self.is_held_by(id),
        }
    }

    /// `HumanPriority::Participant`: a keystroke revokes whoever holds the lock. Returns whether
    /// anyone did, so the caller can skip logging the common "nobody" case.
    pub(crate) fn revoke(&self) -> bool {
        self.holder.lock_unpoisoned().take().is_some()
    }
}

impl Tap {
    /// Serve the NDJSON command protocol on a Unix socket at `path`: one `Command` per line in,
    /// one `CommandResult` line out, each checked against `opts` first. Binds synchronously so a
    /// bad path fails here; removes a stale socket file left by a `SIGKILL`'d run. Takes
    /// `Arc<Tap>` because the accept loop outlives this call.
    pub fn serve(tap: Arc<Tap>, path: &Path, opts: ServeOptions) -> io::Result<()> {
        crate::sockets::serve(tap, path, opts)
    }

    /// Serve the raw-TTY protocol: a client sees a redraw of the current screen, then live output,
    /// and its keystrokes are gated by `policy` exactly like a JSON client's `Send`.
    pub fn serve_tty(tap: Arc<Tap>, path: &Path, policy: WritePolicy) -> io::Result<()> {
        crate::sockets::serve_tty(tap, path, policy)
    }
}
