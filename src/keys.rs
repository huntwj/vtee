//! Named keys -> escape sequences, requested by TinyFugue (`docs/roadmap.md`): they have no
//! key-name-to-escape table of their own and asked for one here. Arrow keys and Home/End switch
//! between CSI (`ESC [`) and SS3 (`ESC O`) forms depending on whether application-cursor-key
//! mode (DECCKM) is active, matching xterm's own behaviour - which is exactly why TF asked for
//! this to be mode-aware: it reads that distinction via termcap/terminfo.

/// Resolve a key name to the bytes to send. `app_cursor_mode` should come from
/// `Tap::app_cursor_mode()` for the tap the key is being sent to.
pub fn resolve(name: &str, app_cursor_mode: bool) -> Option<Vec<u8>> {
    let cursor = |final_byte: u8| vec![0x1b, if app_cursor_mode { b'O' } else { b'[' }, final_byte];

    let seq = match name {
        "Up" => cursor(b'A'),
        "Down" => cursor(b'B'),
        "Right" => cursor(b'C'),
        "Left" => cursor(b'D'),
        "Home" => cursor(b'H'),
        "End" => cursor(b'F'),
        "PgUp" => b"\x1b[5~".to_vec(),
        "PgDn" => b"\x1b[6~".to_vec(),
        "F1" => b"\x1bOP".to_vec(),
        "F2" => b"\x1bOQ".to_vec(),
        "F3" => b"\x1bOR".to_vec(),
        "F4" => b"\x1bOS".to_vec(),
        "F5" => b"\x1b[15~".to_vec(),
        "F6" => b"\x1b[17~".to_vec(),
        "F7" => b"\x1b[18~".to_vec(),
        "F8" => b"\x1b[19~".to_vec(),
        "F9" => b"\x1b[20~".to_vec(),
        "F10" => b"\x1b[21~".to_vec(),
        "F11" => b"\x1b[23~".to_vec(),
        "F12" => b"\x1b[24~".to_vec(),
        _ => {
            if let Some(c) = name.strip_prefix("Ctrl-") {
                vec![ctrl_byte(single_char(c)?)?]
            } else {
                let ch = single_char(name.strip_prefix("Alt-")?)?;
                let mut v = vec![0x1b];
                v.extend(ch.to_string().bytes());
                v
            }
        }
    };
    Some(seq)
}

/// "Ctrl-ab" and "Alt-ab" aren't keys; rejecting them beats silently sending two characters.
fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let ch = chars.next()?;
    chars.next().is_none().then_some(ch)
}

/// `Ctrl-<char>` per the standard control-character mapping (letters, and `@ [ \ ] ^ _ ?`).
fn ctrl_byte(ch: char) -> Option<u8> {
    let byte = match ch.to_ascii_uppercase() {
        upper @ 'A'..='Z' => (upper as u8) - b'A' + 1,
        '@' => 0x00,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' => 0x1f,
        '?' => 0x7f,
        _ => return None,
    };
    Some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrows_follow_app_cursor_mode() {
        assert_eq!(resolve("Up", false), Some(b"\x1b[A".to_vec()));
        assert_eq!(resolve("Up", true), Some(b"\x1bOA".to_vec()));
    }

    #[test]
    fn function_keys() {
        assert_eq!(resolve("F1", false), Some(b"\x1bOP".to_vec()));
        assert_eq!(resolve("F12", false), Some(b"\x1b[24~".to_vec()));
    }

    #[test]
    fn ctrl_and_alt() {
        assert_eq!(resolve("Ctrl-c", false), Some(vec![0x03]));
        assert_eq!(resolve("Ctrl-C", false), Some(vec![0x03]));
        assert_eq!(resolve("Alt-x", false), Some(vec![0x1b, b'x']));
    }

    #[test]
    fn navigation_and_function_key_tables() {
        for (name, plain, app) in [
            ("Up", "\x1b[A", "\x1bOA"),
            ("Down", "\x1b[B", "\x1bOB"),
            ("Right", "\x1b[C", "\x1bOC"),
            ("Left", "\x1b[D", "\x1bOD"),
            ("Home", "\x1b[H", "\x1bOH"),
            ("End", "\x1b[F", "\x1bOF"),
        ] {
            assert_eq!(resolve(name, false), Some(plain.as_bytes().to_vec()), "{name}");
            assert_eq!(resolve(name, true), Some(app.as_bytes().to_vec()), "{name} (app mode)");
        }
        assert_eq!(resolve("PgUp", false), Some(b"\x1b[5~".to_vec()));
        assert_eq!(resolve("PgDn", false), Some(b"\x1b[6~".to_vec()));
        for (name, seq) in [
            ("F1", "\x1bOP"),
            ("F2", "\x1bOQ"),
            ("F3", "\x1bOR"),
            ("F4", "\x1bOS"),
            ("F5", "\x1b[15~"),
            ("F6", "\x1b[17~"),
            ("F7", "\x1b[18~"),
            ("F8", "\x1b[19~"),
            ("F9", "\x1b[20~"),
            ("F10", "\x1b[21~"),
            ("F11", "\x1b[23~"),
            ("F12", "\x1b[24~"),
        ] {
            assert_eq!(resolve(name, false), Some(seq.as_bytes().to_vec()), "{name}");
        }
    }

    #[test]
    fn unknown_key_is_none() {
        assert_eq!(resolve("Banana", false), None);
        assert_eq!(resolve("Ctrl-", false), None);
        assert_eq!(resolve("Ctrl-ab", false), None);
        assert_eq!(resolve("Alt-", false), None);
        assert_eq!(resolve("Alt-ab", false), None);
    }
}
