//! Socket access control, matching tmux's level (`docs/guide/security.md`): sockets are mode 0600
//! whatever the umask, and a connection from another user is closed. Unix only; the other-user
//! test needs root (the Docker test image runs as root) and skips otherwise.

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_vtee");

fn unique(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Readable by every user so the other-user test can reach the socket path inside it.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    dir
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A holder that is killed when the test ends, even if an assertion fails first.
struct Holder(Child);

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a headless holder running `cat`, in a private `TMPDIR`, under the given umask.
fn spawn_holder(tmp: &Path, name: &str, umask: &str, flags: &[&str]) -> Holder {
    let child = Command::new("sh")
        .arg("-c")
        .arg(format!("umask {umask}; exec \"$0\" \"$@\""))
        .arg(BIN)
        .env("TMPDIR", tmp)
        .arg("--headless")
        .args(["--name", name])
        .args(flags)
        .args(["--", "cat"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    Holder(child)
}

fn session_dir(tmp: &Path) -> PathBuf {
    // SAFETY: getuid has no preconditions.
    tmp.join(format!("vtee-{}", unsafe { libc::getuid() }))
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn snapshot_works(path: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(path) else { return false };
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.write_all(b"{\"cmd\":\"snapshot\"}\n").unwrap();
    let mut buf = [0u8; 4096];
    matches!(stream.read(&mut buf), Ok(n) if n > 0)
}

#[test]
fn every_socket_is_mode_0600_even_under_umask_000() {
    let tmp = unique("vtee-sec-mode");
    let dir = session_dir(&tmp);

    // Default session sockets.
    let _default = spawn_holder(&tmp, "plain", "000", &[]);
    wait_until("the default sockets", || dir.join("plain.sock").exists() && dir.join("plain.tty.sock").exists());
    assert_eq!(mode(&dir.join("plain.sock")), 0o600);
    assert_eq!(mode(&dir.join("plain.tty.sock")), 0o600);

    // An explicit --listen path, which sits outside the hardened directory.
    let explicit_dir = unique("vtee-sec-explicit");
    let explicit = explicit_dir.join("custom.sock");
    let listen = format!("--listen={}", explicit.display());
    let _explicit = spawn_holder(&tmp, "custom", "000", &[&listen]);
    wait_until("the explicit socket", || explicit.exists());
    assert_eq!(mode(&explicit), 0o600);

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&explicit_dir);
}

const OTHER_USER_CLIENT: &str = r#"
import socket, sys
s = socket.socket(socket.AF_UNIX)
s.settimeout(5)
try:
    s.connect(sys.argv[1])
except PermissionError:
    print("DENIED")
    sys.exit(0)
except OSError as e:
    print("ERROR", e)
    sys.exit(0)
try:
    s.sendall(b'{"cmd":"snapshot"}\n')
    print("DATA" if s.recv(65536) else "CLOSED")
except (ConnectionResetError, BrokenPipeError):
    print("CLOSED")
except socket.timeout:
    print("TIMEOUT")
"#;

fn required() -> bool {
    std::env::var("VTEE_REQUIRE_TOOLS").is_ok_and(|v| v == "1")
}

#[test]
fn a_connection_from_another_user_is_closed() {
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert!(!required(), "VTEE_REQUIRE_TOOLS=1 but this test needs root to switch users");
        eprintln!("skipping: needs root to run a client as another user");
        return;
    }
    if Command::new("python3").arg("--version").stdout(Stdio::null()).status().is_err() {
        assert!(!required(), "VTEE_REQUIRE_TOOLS=1 but python3 is missing");
        eprintln!("skipping: python3 is not installed");
        return;
    }

    let tmp = unique("vtee-sec-peer");
    let socket_dir = unique("vtee-sec-peer-sock");
    let socket = socket_dir.join("open.sock");
    let listen = format!("--listen={}", socket.display());
    let _holder = spawn_holder(&tmp, "peer", "022", &[&listen]);
    wait_until("the socket", || socket.exists());

    // Open the file mode on purpose so the kernel lets the other user connect; the only thing
    // left between that user and the session is the peer check under test.
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o666)).unwrap();

    assert!(snapshot_works(&socket), "the owning user must still be served");

    let out =
        Command::new("python3").args(["-c", OTHER_USER_CLIENT]).arg(&socket).uid(54321).gid(54321).output().unwrap();
    let verdict = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(verdict, "CLOSED", "another user must be refused, got {verdict:?} ({:?})", out.stderr);

    assert!(snapshot_works(&socket), "a refused client must not disturb the session");

    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&socket_dir);
}
