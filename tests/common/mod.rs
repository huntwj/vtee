//! Helpers shared by the integration tests.
#![allow(dead_code)]

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CURSOR_POSITION_QUERY: &[u8] = b"\x1b[6n";
const CURSOR_POSITION_REPLY: &[u8] = b"\x1b[1;1R";

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack.windows(needle.len()).filter(|w| *w == needle).count()
}

/// A command running on a real pty, so `is_terminal()` is true for it on both platforms. Acts as
/// the terminal: collects everything the process prints and answers cursor-position queries
/// (ConPTY sends one at startup). The process is killed when this is dropped.
pub struct PtyProc {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    seen: Arc<Mutex<Vec<u8>>>,
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
}

impl PtyProc {
    /// Runs `cmd` (program, arguments, environment and working directory) on a new pty.
    pub fn spawn(cmd: &Command) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
            .expect("couldn't open a pty");
        let mut builder = CommandBuilder::new(cmd.get_program());
        builder.args(cmd.get_args());
        for (key, value) in cmd.get_envs() {
            match value {
                Some(value) => builder.env(key, value),
                None => builder.env_remove(key),
            }
        }
        // Always set: portable-pty otherwise starts the child in `$HOME`, which tests redirect to a
        // directory that doesn't exist.
        match cmd.get_current_dir() {
            Some(dir) => builder.cwd(dir),
            None => builder.cwd(std::env::current_dir().unwrap()),
        }
        let child = pair.slave.spawn_command(builder).expect("couldn't spawn on the pty");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
        let seen = Arc::new(Mutex::new(Vec::<u8>::new()));
        {
            let (writer, seen) = (writer.clone(), seen.clone());
            thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let mut answered = 0;
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let queries = {
                        let mut seen = seen.lock().unwrap();
                        seen.extend_from_slice(&buf[..n]);
                        count(&seen, CURSOR_POSITION_QUERY)
                    };
                    for _ in answered..queries {
                        let mut writer = writer.lock().unwrap();
                        let _ = writer.write_all(CURSOR_POSITION_REPLY);
                        let _ = writer.flush();
                    }
                    answered = queries;
                }
            });
        }
        PtyProc { writer, seen, child, _master: pair.master }
    }

    /// Everything printed so far, escape sequences included.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.seen.lock().unwrap()).into_owned()
    }

    /// The output with escape sequences removed. A redrawn screen puts a reset before every cell,
    /// so words only appear contiguous once those are stripped.
    pub fn plain_text(&self) -> String {
        let seen = self.seen.lock().unwrap();
        let mut out = Vec::with_capacity(seen.len());
        let mut i = 0;
        while i < seen.len() {
            if seen[i] != 0x1b {
                out.push(seen[i]);
                i += 1;
            } else if seen.get(i + 1) == Some(&b'[') {
                i += 2;
                while i < seen.len() && !(0x40..=0x7e).contains(&seen[i]) {
                    i += 1;
                }
                i += 1;
            } else {
                i += 2;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// Polls until the output, with escape sequences removed, contains `needle`.
    pub fn wait_for_plain(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.plain_text().contains(needle) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.plain_text().contains(needle)
    }

    pub fn send(&self, bytes: &[u8]) {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(bytes).unwrap();
        writer.flush().unwrap();
    }

    /// Polls until the output contains `needle`.
    pub fn wait_for(&self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.text().contains(needle) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.text().contains(needle)
    }

    pub fn has_exited(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_some()
    }
}

impl Drop for PtyProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
