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

    // No terminal is attached, so the list is the main content: it
    // expands to 80% of the 100-col frame. The right pane's top-left
    // corner is the second '┌' on the first line.
    let first_line = text.lines().next().unwrap();
    let split_at = first_line
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '┌')
        .nth(1)
        .map(|(i, _)| i)
        .unwrap();
    assert_eq!(split_at, 80, "browse mode: left pane should be 80% wide");
}

#[test]
fn attaching_a_terminal_returns_the_list_to_a_quarter_of_the_frame() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut resumable = meta("s1", Agent::Claude, "one");
    resumable.cwd = "/tmp".into();
    let mut app = App::new(vec![resumable]);
    let press = |app: &mut App, code: KeyCode| {
        app.handle_key(
            KeyEvent::new(code, KeyModifiers::NONE),
            session_tui::term::TermModes::default(),
        );
    };
    press(&mut app, KeyCode::Char('h')); // auto-hide off so both panes draw
    press(&mut app, KeyCode::Enter); // attach

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| ui::render(f, &app, None, &Default::default()))
        .unwrap();
    let text = buffer_text(&terminal);

    let split_at = text
        .lines()
        .next()
        .unwrap()
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '┌')
        .nth(1)
        .map(|(i, _)| i)
        .unwrap();
    assert_eq!(split_at, 25, "terminal attached: the terminal is the main content");
}

#[test]
fn hidden_list_gives_the_terminal_the_full_width() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut resumable = meta("s1", Agent::Claude, "one");
    resumable.cwd = "/tmp".into(); // resume refuses missing cwds
    let mut app = App::new(vec![resumable]);
    app.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        session_tui::term::TermModes::default(),
    );
    assert!(app.list_hidden());

    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| ui::render(f, &app, None, &Default::default()))
        .unwrap();
    let text = buffer_text(&terminal);

    assert!(!text.contains(" Sessions "), "list pane must not render:\n{text}");
    let first_line = text.lines().next().unwrap();
    let corners: Vec<usize> = first_line
        .chars()
        .enumerate()
        .filter(|(_, c)| *c == '┌')
        .map(|(i, _)| i)
        .collect();
    assert_eq!(corners, vec![0], "one full-width pane starting at column 0:\n{text}");
}

#[test]
fn drawn_terminal_pane_and_pty_size_agree() {
    // The invariant behind "terminal_pane_size is the single source of
    // PTY dimensions": the size handed to the PTY must equal the inner
    // size of the pane render() actually draws, or the child renders
    // at one size into a pane of another. Expectations are read from
    // the rendered buffer so this pins agreement, not a copy of the
    // layout math. Geometry has three states — expanded list (nothing
    // attached), 25/75 split (attached, auto-hide off), terminal-only
    // (attached, auto-hide on) — and every one must agree.
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    for layout in ["expanded", "split", "hidden"] {
        for (w, h) in [(100u16, 30u16), (81, 24), (33, 9)] {
            let mut resumable = meta("s1", Agent::Claude, "one");
            resumable.cwd = "/tmp".into();
            let mut app = App::new(vec![resumable]);
            let press = |app: &mut App, code: KeyCode| {
                app.handle_key(
                    KeyEvent::new(code, KeyModifiers::NONE),
                    session_tui::term::TermModes::default(),
                );
            };
            match layout {
                "expanded" => {}
                "split" => {
                    press(&mut app, KeyCode::Char('h')); // auto-hide off
                    press(&mut app, KeyCode::Enter); // attach
                }
                _ => press(&mut app, KeyCode::Enter), // attach, auto-hide on
            }
            assert_eq!(app.list_hidden(), layout == "hidden");
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            terminal
                .draw(|f| ui::render(f, &app, None, &Default::default()))
                .unwrap();
            let text = buffer_text(&terminal);

            // The terminal pane's top-left corner is the last '┌' on
            // row 0 (every cell in that row is a width-1 symbol, so
            // char index == column): the second corner when the list
            // is drawn, the only one when it is hidden.
            let right_x = text
                .lines()
                .next()
                .unwrap()
                .chars()
                .enumerate()
                .filter(|(_, c)| *c == '┌')
                .last()
                .map(|(i, _)| i)
                .unwrap() as u16;
            assert_eq!(right_x == 0, layout == "hidden", "corners follow visibility");
            let drawn_outer_w = w - right_x;
            let drawn_outer_h = h - 1; // help bar takes the last row

            let (rows, cols) =
                ui::terminal_pane_size(ratatui::layout::Rect::new(0, 0, w, h), &app);
            assert_eq!(
                (rows, cols),
                (drawn_outer_h - 2, drawn_outer_w - 2),
                "PTY size must match the drawn pane's interior at {w}x{h} ({layout})"
            );
        }
    }
}

#[test]
fn help_bar_lists_the_auto_hide_key_in_list_focus() {
    let app = App::new(vec![meta("s1", Agent::Claude, "one")]);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| ui::render(f, &app, None, &Default::default()))
        .unwrap();
    let text = buffer_text(&terminal);
    assert!(text.contains("h auto-hide"), "help bar must advertise the toggle:\n{text}");
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
