//! User input → child bytes. Everything the app writes to a PTY is
//! encoded here; the state machine holds no byte-level knowledge.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::term::TermModes;

/// The reserved list/terminal focus chord; it must never reach the PTY.
pub fn is_focus_toggle(key: &KeyEvent) -> bool {
    // Ctrl+\ arrives as Char('\\') under the kitty protocol but as
    // Char('4') from legacy terminals (crossterm maps byte 0x1C that way).
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('\\') | KeyCode::Char('4'))
}

/// Translate a crossterm key event into the bytes a terminal would
/// send. `modes.app_cursor` is the child's DECCKM mode: unmodified
/// arrows become SS3 (ESC O x) instead of CSI (ESC [ x).
pub fn encode_key(key: &KeyEvent, modes: TermModes) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Modified arrows use the xterm CSI 1;<mod> scheme:
    // 2=Shift, 3=Alt, 5=Ctrl (and their sums).
    if let Some(arrow) = match key.code {
        KeyCode::Up => Some(b'A'),
        KeyCode::Down => Some(b'B'),
        KeyCode::Right => Some(b'C'),
        KeyCode::Left => Some(b'D'),
        _ => None,
    } {
        let mut modifier = 1;
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            modifier += 1;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            modifier += 2;
        }
        if ctrl {
            modifier += 4;
        }
        if modifier > 1 {
            return Some(format!("\x1b[1;{}{}", modifier, arrow as char).into_bytes());
        }
        if modes.app_cursor {
            return Some(vec![0x1b, b'O', arrow]);
        }
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        // Meta convention: ESC-prefix the unmodified encoding.
        let stripped = KeyEvent::new(key.code, key.modifiers - KeyModifiers::ALT);
        return encode_key(&stripped, modes).map(|mut bytes| {
            bytes.insert(0, 0x1b);
            bytes
        });
    }
    Some(match key.code {
        KeyCode::Char(c) if ctrl => {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                // Ctrl+A..Ctrl+Z map to 0x01..0x1a
                vec![c as u8 - b'a' + 1]
            } else {
                // Non-letter control bytes. Legacy terminals deliver
                // 0x1d..0x1f as Ctrl+5..Ctrl+7 (0x1c/Ctrl+4 is the
                // focus toggle and never reaches here).
                match c {
                    ' ' | '@' => vec![0x00],
                    '[' => vec![0x1b],
                    ']' | '5' => vec![0x1d],
                    '^' | '6' => vec![0x1e],
                    '_' | '7' | '/' => vec![0x1f],
                    '?' => vec![0x7f],
                    _ => return None,
                }
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n @ 1..=4) => vec![0x1b, b'O', b'P' + n - 1],
        KeyCode::F(n @ 5..=12) => {
            // xterm numbering skips 16 and 22.
            let code = [15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5];
            format!("\x1b[{code}~").into_bytes()
        }
        _ => return None,
    })
}

/// Encode pasted text for the child. Wrapped in bracketed-paste
/// delimiters only when the child enabled DECSET 2004; without it the
/// delimiters would be literal garbage in the child's input.
pub fn encode_paste(text: &str, modes: TermModes) -> Vec<u8> {
    if modes.bracketed_paste {
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}
