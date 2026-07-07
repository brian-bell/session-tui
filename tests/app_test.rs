use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use session_tui::app::{App, Effect, Focus, Overlay};
use session_tui::sessions::{Agent, SessionMeta};

fn meta(id: &str, title: &str) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        agent: Agent::Claude,
        cwd: "/Users/brian/dev/myproj".into(),
        title: title.into(),
        timestamp: chrono::Utc::now(),
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[test]
fn enter_on_a_historical_session_resumes_it_and_focuses_the_terminal() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    assert_eq!(app.focus, Focus::List);

    let effects = app.handle_key(key(KeyCode::Enter));

    match &effects[..] {
        [Effect::Spawn { run_id, spec }] => {
            assert_eq!(spec.program, "claude");
            assert_eq!(spec.args, vec!["--resume", "s1"]);
            assert!(app.is_running(*run_id));
        }
        other => panic!("expected one Spawn effect, got {other:?}"),
    }
    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.attached_run().is_some());
}

#[test]
fn ctrl_backslash_toggles_focus_and_terminal_mode_passes_keys_through() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    app.handle_key(key(KeyCode::Enter)); // resume + focus terminal

    // 'j' in terminal mode goes to the PTY, not list navigation
    let run_id = app.attached_run().unwrap();
    let effects = app.handle_key(key(KeyCode::Char('j')));
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"j".to_vec() }]
    );
    assert_eq!(app.selected, 0);

    // Ctrl+\ returns to the list; 'j' now moves the selection
    app.handle_key(ctrl('\\'));
    assert_eq!(app.focus, Focus::List);
    let effects = app.handle_key(key(KeyCode::Char('j')));
    assert!(effects.is_empty());
    assert_eq!(app.selected, 1);

    // Ctrl+\ toggles back into the attached terminal
    app.handle_key(ctrl('\\'));
    assert_eq!(app.focus, Focus::Terminal);
}

#[test]
fn quit_is_immediate_when_idle_but_confirmed_when_sessions_are_running() {
    // No running sessions: 'q' quits immediately.
    let mut app = App::new(vec![meta("s1", "one")]);
    assert_eq!(app.handle_key(key(KeyCode::Char('q'))), vec![Effect::Quit]);

    // With a running session: 'q' asks first; 'y' quits, Esc cancels.
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ctrl('\\')); // back to list

    assert!(app.handle_key(key(KeyCode::Char('q'))).is_empty());
    assert_eq!(app.overlay, Overlay::ConfirmQuit);
    assert!(app.handle_key(key(KeyCode::Esc)).is_empty());
    assert_eq!(app.overlay, Overlay::None);

    app.handle_key(key(KeyCode::Char('q')));
    assert_eq!(app.handle_key(key(KeyCode::Char('y'))), vec![Effect::Quit]);
}

#[test]
fn ctrl_k_kills_the_selected_running_session_after_confirmation() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();
    app.handle_key(ctrl('\\'));

    // Ctrl+K on a running session asks for confirmation.
    assert!(app.handle_key(ctrl('k')).is_empty());
    assert_eq!(app.overlay, Overlay::ConfirmKill { run_id });

    let effects = app.handle_key(key(KeyCode::Char('y')));
    assert_eq!(effects, vec![Effect::Kill { run_id }]);
    assert_eq!(app.overlay, Overlay::None);
    assert!(!app.is_running(run_id));
    assert_eq!(app.attached_run(), None, "killed session must detach");

    // Ctrl+K on a non-running session does nothing.
    assert!(app.handle_key(ctrl('k')).is_empty());
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn launch_picker_starts_a_fresh_agent_in_a_known_project_dir() {
    let mut a = meta("s1", "one");
    a.cwd = "/dev/proj-a".into();
    let mut b = meta("s2", "two");
    b.cwd = "/dev/proj-b".into();
    let mut a2 = meta("s3", "three");
    a2.cwd = "/dev/proj-a".into(); // duplicate cwd, should be deduped
    let mut app = App::new(vec![a, b, a2]);

    app.handle_key(key(KeyCode::Char('n')));
    assert!(matches!(app.overlay, Overlay::LaunchPicker(_)));

    // Newest-first, deduped: proj-a then proj-b. Move down to proj-b,
    // toggle agent to codex, launch.
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Tab));
    let effects = app.handle_key(key(KeyCode::Enter));

    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.program, "codex");
            assert_eq!(spec.cwd, "/dev/proj-b");
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.focus, Focus::Terminal);
    // A provisional entry for the new session tops the list.
    assert_eq!(app.sessions[0].cwd, "/dev/proj-b");
    assert_eq!(app.selected, 0);
}

#[test]
fn launch_picker_accepts_a_typed_path_that_matches_nothing() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));
    for c in "/tmp/brand-new".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let effects = app.handle_key(key(KeyCode::Enter));
    match &effects[..] {
        [Effect::Spawn { spec, .. }] => assert_eq!(spec.cwd, "/tmp/brand-new"),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[test]
fn rescan_preserves_selection_and_provisional_live_entries() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    app.handle_key(key(KeyCode::Down)); // select s2

    // A launch adds a provisional entry on top.
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ctrl('\\'));
    let provisional_id = app.sessions[0].id.clone();

    // Fresh scan arrives with a new session on top.
    app.update_sessions(vec![meta("s3", "newest"), meta("s1", "one"), meta("s2", "two")]);

    assert_eq!(app.sessions[0].id, provisional_id, "live entry stays on top");
    assert_eq!(app.sessions[1].id, "s3");
    // Selection follows the provisional session we launched, not an index.
    assert_eq!(app.sessions[app.selected].id, provisional_id);
}

#[test]
fn page_up_scrolls_back_and_any_other_key_snaps_to_live() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    assert_eq!(app.scroll_offset, 0);
    app.handle_key(key(KeyCode::PageUp));
    let offset = app.scroll_offset;
    assert!(offset > 0);
    app.handle_key(key(KeyCode::PageUp));
    assert!(app.scroll_offset > offset);
    app.handle_key(key(KeyCode::PageDown));
    assert_eq!(app.scroll_offset, offset);

    // Any non-scroll key snaps back to live and still reaches the PTY.
    let effects = app.handle_key(key(KeyCode::Char('x')));
    assert_eq!(app.scroll_offset, 0);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"x".to_vec() }]
    );
}

#[test]
fn a_session_whose_child_exits_detaches_and_stops_showing_as_running() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    app.mark_exited(run_id);

    assert!(!app.is_running(run_id));
    assert_eq!(app.attached_run(), None);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn terminal_mode_encodes_special_keys_as_ansi_sequences() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    let cases: Vec<(KeyEvent, &[u8])> = vec![
        (key(KeyCode::Enter), b"\r"),
        (key(KeyCode::Esc), b"\x1b"),
        (key(KeyCode::Backspace), b"\x7f"),
        (key(KeyCode::Tab), b"\t"),
        (key(KeyCode::Up), b"\x1b[A"),
        (key(KeyCode::Down), b"\x1b[B"),
        (key(KeyCode::Right), b"\x1b[C"),
        (key(KeyCode::Left), b"\x1b[D"),
        (ctrl('c'), b"\x03"),
        (ctrl('d'), b"\x04"),
    ];
    for (k, want) in cases {
        let effects = app.handle_key(k);
        assert_eq!(
            effects,
            vec![Effect::WriteTerminal { run_id, bytes: want.to_vec() }],
            "for key {k:?}"
        );
    }
}
