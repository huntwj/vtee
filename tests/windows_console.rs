//! Console-mode behaviour of `RawModeGuard` on Windows (`docs/guide/windows.md`, "VT console flags").
//!
//! The guard turns on `ENABLE_VIRTUAL_TERMINAL_INPUT` / `ENABLE_VIRTUAL_TERMINAL_PROCESSING` for its
//! lifetime and puts the original modes back. Both `attach` and the interactive spawn path create
//! this guard, so testing it covers them. The test binary re-runs itself as the "probe" inside a
//! ConPTY (the same pty-in-pty approach as `windows_conpty.rs`): the probe reports the console modes
//! before, during and after the guard, writes a coloured string, and echoes the first bytes it reads
//! from stdin. The parent plays the terminal, answers ConPTY's cursor query and sends an Up-arrow.

#![cfg(windows)]

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vtee::RawModeGuard;

const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
const STD_INPUT_HANDLE: u32 = -10i32 as u32;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(std_handle: u32) -> isize;
    fn GetConsoleMode(handle: isize, mode: *mut u32) -> i32;
}

fn console_modes() -> (u32, u32) {
    let mut input = 0u32;
    let mut output = 0u32;
    unsafe {
        GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut input);
        GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &mut output);
    }
    (input, output)
}

fn report(label: &str) {
    let (input, output) = console_modes();
    println!("PROBE {label} in={input:x} out={output:x}");
    let _ = std::io::stdout().flush();
}

/// Not a real test: the child half of `the_guard_enables_vt_flags_and_restores_them`. Does nothing
/// unless the parent set the environment variable.
#[test]
fn probe_child() {
    if std::env::var_os("VTEE_VT_PROBE").is_none() {
        return;
    }
    report("before");
    {
        let _guard = RawModeGuard::new();
        report("raw");
        print!("PROBE colour \x1b[31mRED\x1b[0m end\r\n");
        let _ = std::io::stdout().flush();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16];
            if let Ok(n) = std::io::stdin().read(&mut buf) {
                let _ = tx.send(buf[..n].to_vec());
            }
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(bytes) => {
                let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                println!("PROBE keys {hex}");
            }
            Err(_) => println!("PROBE keys none"),
        }
        let _ = std::io::stdout().flush();
    }
    report("after");
    std::process::exit(0);
}

/// Text with terminal control sequences removed, so assertions match what a person would read.
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            match bytes.get(i) {
                Some(b'[') => {
                    i += 1;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                Some(b']') => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != 0x07 && !(bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\'))
                    {
                        i += 1;
                    }
                    i += if bytes.get(i) == Some(&0x1b) { 2 } else { 1 };
                }
                _ => i += 1,
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn wait_for(raw: &Arc<Mutex<Vec<u8>>>, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if strip_ansi(&raw.lock().unwrap()).contains(needle) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Parses the `in=`/`out=` hex values of the `PROBE <label>` line.
fn modes_of(text: &str, label: &str) -> (u32, u32) {
    let line = text
        .lines()
        .find(|l| l.contains(&format!("PROBE {label} in=")))
        .unwrap_or_else(|| panic!("no 'PROBE {label}' line in:\n{text}"));
    let field = |name: &str| {
        let start = line.find(name).unwrap() + name.len();
        let hex: String = line[start..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        u32::from_str_radix(&hex, 16).unwrap()
    };
    (field("in="), field("out="))
}

#[test]
fn the_guard_enables_vt_flags_and_restores_them() {
    let pair = native_pty_system()
        .openpty(PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 })
        .expect("failed to open a ConPTY");
    let mut cmd = CommandBuilder::new(std::env::current_exe().unwrap());
    cmd.args(["probe_child", "--exact", "--nocapture", "--test-threads=1"]);
    cmd.env("VTEE_VT_PROBE", "1");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn the probe");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let raw = Arc::new(Mutex::new(Vec::<u8>::new()));
    {
        let writer = writer.clone();
        let raw = raw.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut answered = 0usize;
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let mut out = raw.lock().unwrap();
                out.extend_from_slice(&buf[..n]);
                // Answer every cursor-position query ConPTY sends, as a real terminal would.
                let queries = out.windows(4).filter(|w| *w == b"\x1b[6n").count();
                while answered < queries {
                    let mut w = writer.lock().unwrap();
                    let _ = w.write_all(b"\x1b[1;1R");
                    let _ = w.flush();
                    answered += 1;
                }
            }
        });
    }

    assert!(wait_for(&raw, "PROBE colour", Duration::from_secs(30)), "probe never reached raw mode");
    // An Up-arrow as a terminal sends it.
    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1b[A").unwrap();
        w.flush().unwrap();
    }
    assert!(wait_for(&raw, "PROBE after", Duration::from_secs(30)), "probe never finished");
    let _ = child.kill();

    let text = strip_ansi(&raw.lock().unwrap());
    let (in_before, out_before) = modes_of(&text, "before");
    let (in_raw, out_raw) = modes_of(&text, "raw");
    let (in_after, out_after) = modes_of(&text, "after");

    assert!(in_raw & ENABLE_VIRTUAL_TERMINAL_INPUT != 0, "VT input not enabled: in={in_raw:x}\n{text}");
    assert!(out_raw & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0, "VT output not enabled: out={out_raw:x}\n{text}");
    assert_eq!(in_after, in_before, "input mode not restored\n{text}");
    assert_eq!(out_after, out_before, "output mode not restored\n{text}");

    // Input side: without ENABLE_VIRTUAL_TERMINAL_INPUT the arrow key never reaches `ReadFile` at all
    // (measured: `keys none`), so this assertion fails when the flag is not set. Output side: a
    // ConPTY console already starts with VT processing on (`out=7`), so the colour checks below hold
    // either way here; the flag matters for a legacy `conhost` window, which this harness cannot create.
    assert!(text.contains("PROBE keys 1b5b41"), "Up-arrow did not arrive as ESC [ A:\n{text}");
    assert!(text.contains("PROBE colour RED end"), "colour string was not interpreted:\n{text}");
    assert!(!text.contains("[31m"), "escape sequence was printed as text:\n{text}");
}
