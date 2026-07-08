use ratatui::{backend::TestBackend, Terminal};
use session_tui::app::App;
use session_tui::sessions::{Agent, SessionMeta};
use session_tui::ui;

fn meta(id: &str, agent: Agent, title: &str) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        agent,
        cwd: "/Users/brian/dev/myproj".into(),
        title: title.into(),
        timestamp: chrono::Utc::now(),
    }
}

fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

#[test]
fn renders_session_list_left_and_terminal_placeholder_right() {
    let app = App::new(vec![
        meta("s1", Agent::Claude, "fix the login bug"),
        meta("s2", Agent::Codex, "refactor parser"),
    ]);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();

    terminal
        .draw(|f| ui::render(f, &app, None, &Default::default()))
        .unwrap();
    let text = buffer_text(&terminal);

    // Left pane lists both sessions (titles truncated to pane width).
    assert!(text.contains("fix the login"), "missing claude row:\n{text}");
    assert!(text.contains("refactor parse"), "missing codex row:\n{text}");
    // Right pane shows a hint when nothing is attached.
    assert!(text.contains("Enter"), "missing placeholder hint:\n{text}");

    // The vertical split sits at 25% of a 100-col frame: the right
    // pane's top-left corner is the second '┌' on the first line.
    let first_line = text.lines().next().unwrap();
    let split_at = first_line
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '┌')
        .nth(1)
        .map(|(i, _)| i)
        .unwrap();
    assert_eq!(split_at, 25, "left pane should be 25% wide");
}

#[test]
fn drawn_terminal_pane_and_pty_size_agree() {
    // The invariant behind "terminal_pane_size is the single source of
    // PTY dimensions": the size handed to the PTY must equal the inner
    // size of the pane render() actually draws, or the child renders
    // at one size into a pane of another. Expectations are read from
    // the rendered buffer so this pins agreement, not a copy of the
    // layout math.
    for (w, h) in [(100u16, 30u16), (81, 24), (33, 9)] {
        let app = App::new(vec![meta("s1", Agent::Claude, "one")]);
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| ui::render(f, &app, None, &Default::default()))
            .unwrap();
        let text = buffer_text(&terminal);

        // The right pane's top-left corner is the second '┌' on row 0
        // (every cell in that row is a width-1 symbol, so char index
        // == column).
        let right_x = text
            .lines()
            .next()
            .unwrap()
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == '┌')
            .nth(1)
            .map(|(i, _)| i)
            .unwrap() as u16;
        let drawn_outer_w = w - right_x;
        let drawn_outer_h = h - 1; // help bar takes the last row

        let (rows, cols) =
            ui::terminal_pane_size(ratatui::layout::Rect::new(0, 0, w, h));
        assert_eq!(
            (rows, cols),
            (drawn_outer_h - 2, drawn_outer_w - 2),
            "PTY size must match the drawn pane's interior at {w}x{h}"
        );
    }
}

#[test]
fn transcript_derived_strings_render_without_control_characters() {
    // cwd comes straight from JSONL and may contain hostile bytes; no
    // control character may reach a rendered cell (we sanitize at the
    // render boundary rather than relying on ratatui's filtering).
    let mut evil = meta("s1", Agent::Claude, "title");
    evil.cwd = "/tmp/\x1b]0;pwned\x07dir".into();
    let mut app = App::new(vec![evil]);
    // Notices embed transcript-derived cwds too.
    app.notice = Some("directory no longer exists: /tmp/\x1b]0;pwned\x07dir".into());
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| ui::render(f, &app, None, &Default::default()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let area = *buffer.area();
    for y in 0..area.height {
        for x in 0..area.width {
            assert!(
                !buffer[(x, y)].symbol().chars().any(|c| c.is_control()),
                "control char in cell ({x},{y})"
            );
        }
    }
}

#[test]
fn picker_renders_without_panicking_on_a_tiny_terminal() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new(vec![meta("s1", Agent::Claude, "one")]);
    app.handle_key(
        KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        session_tui::term::TermModes::default(),
    );

    for (w, h) in [(20, 6), (10, 3), (5, 1), (80, 7)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| ui::render(f, &app, None, &Default::default()))
            .unwrap();
    }
}
