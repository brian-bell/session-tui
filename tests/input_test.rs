use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use session_tui::input::{encode_key, encode_paste, is_focus_toggle};
use session_tui::term::TermModes;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
}

#[test]
fn keys_encode_as_the_ansi_sequences_a_terminal_would_send() {
    let cases: Vec<(KeyEvent, &[u8])> = vec![
        (key(KeyCode::Enter), b"\r"),
        (key(KeyCode::Esc), b"\x1b"),
        (key(KeyCode::Backspace), b"\x7f"),
        (key(KeyCode::Tab), b"\t"),
        (key(KeyCode::BackTab), b"\x1b[Z"),
        (key(KeyCode::Up), b"\x1b[A"),
        (key(KeyCode::Down), b"\x1b[B"),
        (key(KeyCode::Right), b"\x1b[C"),
        (key(KeyCode::Left), b"\x1b[D"),
        (key(KeyCode::Home), b"\x1b[H"),
        (key(KeyCode::End), b"\x1b[F"),
        (key(KeyCode::Delete), b"\x1b[3~"),
        (key(KeyCode::Insert), b"\x1b[2~"),
        (key(KeyCode::Char('x')), b"x"),
        (ctrl('c'), b"\x03"),
        (ctrl('d'), b"\x04"),
        // Meta/Alt: ESC-prefixed for chars (readline Alt+f/Alt+b),
        // CSI 1;3 modifiers for arrows.
        (alt('f'), b"\x1bf"),
        (alt('b'), b"\x1bb"),
        (KeyEvent::new(KeyCode::Up, KeyModifiers::ALT), b"\x1b[1;3A"),
        (KeyEvent::new(KeyCode::Down, KeyModifiers::ALT), b"\x1b[1;3B"),
        // Non-letter control keys: Ctrl+Space (NUL), Ctrl+] (GS),
        // Ctrl+^ (RS), Ctrl+_ (US, readline undo), Ctrl+/ (US too).
        // Legacy terminals deliver the 0x1d..0x1f bytes as Ctrl+5..7.
        (ctrl(' '), b"\x00"),
        (ctrl('@'), b"\x00"),
        (ctrl('['), b"\x1b"),
        (ctrl(']'), b"\x1d"),
        (ctrl('5'), b"\x1d"),
        (ctrl('^'), b"\x1e"),
        (ctrl('6'), b"\x1e"),
        (ctrl('_'), b"\x1f"),
        (ctrl('7'), b"\x1f"),
        (ctrl('/'), b"\x1f"),
        // Function keys and modified arrows.
        (key(KeyCode::F(1)), b"\x1bOP"),
        (key(KeyCode::F(5)), b"\x1b[15~"),
        (key(KeyCode::F(12)), b"\x1b[24~"),
        (KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C"),
        (KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT), b"\x1b[1;2D"),
    ];
    for (k, want) in cases {
        assert_eq!(
            encode_key(&k, TermModes::default()),
            Some(want.to_vec()),
            "for key {k:?}"
        );
    }
}

#[test]
fn decckm_switches_unmodified_arrows_to_ss3() {
    let decckm = TermModes { app_cursor: true, ..Default::default() };

    assert_eq!(encode_key(&key(KeyCode::Up), decckm), Some(b"\x1bOA".to_vec()));
    assert_eq!(encode_key(&key(KeyCode::Left), decckm), Some(b"\x1bOD".to_vec()));
    // Modified arrows keep the CSI 1;<mod> form even in DECCKM.
    assert_eq!(
        encode_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::ALT), decckm),
        Some(b"\x1b[1;3A".to_vec())
    );
}

#[test]
fn keys_with_no_terminal_encoding_produce_nothing() {
    assert_eq!(encode_key(&key(KeyCode::F(13)), TermModes::default()), None);
    assert_eq!(encode_key(&key(KeyCode::CapsLock), TermModes::default()), None);
    assert_eq!(encode_key(&ctrl('1'), TermModes::default()), None);
}

#[test]
fn focus_toggle_matches_kitty_and_legacy_encodings_only() {
    // Ctrl+\ arrives as Char('\\') under the kitty protocol but as
    // Char('4') from legacy terminals (crossterm maps byte 0x1C).
    assert!(is_focus_toggle(&ctrl('\\')));
    assert!(is_focus_toggle(&ctrl('4')));
    assert!(!is_focus_toggle(&key(KeyCode::Char('\\'))));
    assert!(!is_focus_toggle(&key(KeyCode::Char('4'))));
    assert!(!is_focus_toggle(&ctrl('c')));
}

#[test]
fn paste_is_bracketed_only_when_the_child_asked_for_it() {
    let bracketed = TermModes { bracketed_paste: true, ..Default::default() };
    assert_eq!(
        encode_paste("hello\nworld", bracketed),
        b"\x1b[200~hello\nworld\x1b[201~".to_vec()
    );
    // Without DECSET 2004 the delimiters would arrive as literal input.
    assert_eq!(encode_paste("hello", TermModes::default()), b"hello".to_vec());
}
