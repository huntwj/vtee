//! Follow-up checks beyond the T1-T14 matrix: does alacritty_terminal give the same result when fed in small
//! chunks instead of all at once (the real vtee usage pattern, reading a pty in arbitrary OS-sized reads), and
//! does it panic on malformed/garbage input instead of degrading gracefully?
use std::fs;
use std::panic;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term};
use vte::ansi::Processor;

const COLS: usize = 40;
const ROWS: usize = 12;
const TESTS: &[&str] = &[
    "T1", "T2", "T3", "T4", "T5", "T6", "T7a", "T7b", "T8", "T9", "T10", "T11", "T12", "T13", "T14",
];

#[derive(Clone)]
struct NullListener;
impl EventListener for NullListener {
    fn send_event(&self, _event: Event) {}
}

struct Size {
    columns: usize,
    screen_lines: usize,
}
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

fn lines_of(term: &Term<NullListener>) -> Vec<String> {
    let grid = term.grid();
    (0..ROWS)
        .map(|r| {
            (0..COLS)
                .map(|c| grid[Point::new(Line(r as i32), Column(c))].c)
                .collect::<String>()
        })
        .collect()
}

fn cursor_of(term: &Term<NullListener>) -> (i32, usize) {
    let p = term.grid().cursor.point;
    (p.line.0, p.column.0)
}

fn new_term() -> Term<NullListener> {
    let size = Size { columns: COLS, screen_lines: ROWS };
    Term::new(Config::default(), &size, NullListener)
}

/// Feed `data` to `term` in chunks of `chunk_size` bytes, each via its own `advance()` call.
fn feed_chunked(term: &mut Term<NullListener>, data: &[u8], chunk_size: usize) {
    let mut parser = Processor::<vte::ansi::StdSyncHandler>::new();
    for chunk in data.chunks(chunk_size.max(1)) {
        parser.advance(term, chunk);
    }
}

fn feed_whole(term: &mut Term<NullListener>, data: &[u8]) {
    let mut parser = Processor::<vte::ansi::StdSyncHandler>::new();
    parser.advance(term, data);
}

fn check_chunking() {
    println!("== chunked-feed equivalence (same bytes, split across multiple advance() calls) ==");
    let mut mismatches = 0;
    for name in TESTS {
        let path = format!("../../{}.bin", name);
        let data = fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {}", path, e));

        let mut whole = new_term();
        feed_whole(&mut whole, &data);
        let whole_lines = lines_of(&whole);
        let whole_cursor = cursor_of(&whole);

        for &chunk_size in &[1usize, 3, 7, 64] {
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                let mut t = new_term();
                feed_chunked(&mut t, &data, chunk_size);
                (lines_of(&t), cursor_of(&t))
            }));
            match result {
                Ok((lines, cursor)) => {
                    if lines != whole_lines || cursor != whole_cursor {
                        mismatches += 1;
                        println!(
                            "  {} chunk_size={}: MISMATCH vs single-shot\n    whole : {:?} cursor={:?}\n    chunked: {:?} cursor={:?}",
                            name, chunk_size, whole_lines, whole_cursor, lines, cursor
                        );
                    } else {
                        println!("  {} chunk_size={}: identical", name, chunk_size);
                    }
                }
                Err(e) => {
                    mismatches += 1;
                    println!("  {} chunk_size={}: PANICKED: {:?}", name, chunk_size, e);
                }
            }
        }
    }
    println!("chunking mismatches: {}", mismatches);
}

fn check_garbage() {
    println!("== malformed / garbage input robustness ==");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("truncated_csi", b"hello \x1b[".to_vec()),
        ("truncated_csi_params", b"hello \x1b[38;5".to_vec()),
        ("truncated_osc_no_terminator", b"hello \x1b]0;untitled".to_vec()),
        ("truncated_utf8_2byte", b"hello \xc3".to_vec()),
        ("truncated_utf8_3byte", b"hello \xe4\xbd".to_vec()),
        ("truncated_utf8_4byte", b"hello \xf0\x9f\x98".to_vec()),
        ("invalid_utf8_continuation", b"hello \x80\x80\x80 world".to_vec()),
        ("lone_esc_then_null", vec![b'h', b'i', 0x1b, 0x00, 0x00]),
        ("bare_high_bytes", (0u8..=255u8).collect()),
        (
            "pseudo_random_4k",
            (0u32..4096)
                .map(|i| ((i.wrapping_mul(2654435761)) >> 24) as u8)
                .collect(),
        ),
        ("csi_huge_param", b"\x1b[999999999999999999999999999999999999999999999999999999m".to_vec()),
        ("deeply_nested_escapes", std::iter::repeat(0x1bu8).take(10_000).collect()),
    ];

    let mut panics = 0;
    for (name, data) in &cases {
        let data = data.clone();
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            let mut t = new_term();
            feed_whole(&mut t, &data);
            // also exercise the byte-at-a-time path, since that's closer to a pty's actual delivery pattern
            let mut t2 = new_term();
            feed_chunked(&mut t2, &data, 1);
            (lines_of(&t), lines_of(&t2))
        }));
        match result {
            Ok(_) => println!("  {}: ok, no panic ({} bytes)", name, data.len()),
            Err(e) => {
                panics += 1;
                println!("  {}: PANICKED: {:?}", name, e);
            }
        }
    }
    println!("garbage-input panics: {}", panics);
}

fn main() {
    check_chunking();
    println!();
    check_garbage();
}
