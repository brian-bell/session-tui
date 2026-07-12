use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use session_tui::app::{App, Effect, Focus, Overlay};
use session_tui::sessions::{Agent, SessionMeta};
use session_tui::term::TermModes;

fn meta(id: &str, title: &str) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        agent: Agent::Claude,
        // A real directory: picker launches refuse missing cwds.
        cwd: "/tmp".into(),
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

/// Drive a key through the app with default child modes.
fn press(app: &mut App, k: KeyEvent) -> Vec<Effect> {
    app.handle_key(k, TermModes::default())
}

fn paste(app: &mut App, text: &str, bracketed: bool) -> Vec<Effect> {
    app.handle_paste(text, TermModes { bracketed_paste: bracketed, ..Default::default() })
}

#[test]
fn enter_on_a_historical_session_resumes_it_and_focuses_the_terminal() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    assert_eq!(app.focus, Focus::List);

    let effects = press(&mut app, key(KeyCode::Enter));

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
    press(&mut app, key(KeyCode::Enter)); // resume + focus terminal

    // 'j' in terminal mode goes to the PTY, not list navigation
    let run_id = app.attached_run().unwrap();
    let effects = press(&mut app, key(KeyCode::Char('j')));
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"j".to_vec() }]
    );
    assert_eq!(app.roster().selected(), 0);

    // Ctrl+\ returns to the list; 'j' now moves the selection
    press(&mut app, ctrl('\\'));
    assert_eq!(app.focus, Focus::List);
    let effects = press(&mut app, key(KeyCode::Char('j')));
    assert!(effects.is_empty());
    assert_eq!(app.roster().selected(), 1);

    // Ctrl+\ toggles back into the attached terminal
    press(&mut app, ctrl('\\'));
    assert_eq!(app.focus, Focus::Terminal);

    // Legacy terminals deliver Ctrl+\ (byte 0x1C) as Ctrl+4; it must
    // toggle too, and must NOT leak through to the PTY as a key.
    let effects = press(&mut app, ctrl('4'));
    assert!(effects.is_empty());
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn list_hides_when_terminal_gains_focus_by_default() {
    let mut app = App::new(vec![meta("s1", "one")]);
    assert!(!app.list_hidden(), "list focus: the list is visible");

    press(&mut app, key(KeyCode::Enter)); // resume + focus terminal

    assert_eq!(app.focus, Focus::Terminal);
    assert!(app.list_hidden(), "auto-hide defaults on");
}

#[test]
fn focus_toggle_shows_the_list_again_and_rehides_on_return() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    assert!(app.list_hidden());

    press(&mut app, ctrl('\\'));
    assert!(!app.list_hidden(), "back in the list, the panel shows");

    press(&mut app, ctrl('\\'));
    assert!(app.list_hidden(), "re-entering the terminal hides it again");
}

#[test]
fn h_in_list_focus_toggles_auto_hide_off_and_on() {
    let mut app = App::new(vec![meta("s1", "one")]);

    let effects = press(&mut app, key(KeyCode::Char('h')));
    assert!(effects.is_empty());
    assert_eq!(app.notice.as_deref(), Some("auto-hide off"));

    press(&mut app, key(KeyCode::Enter)); // focus terminal
    assert!(!app.list_hidden(), "auto-hide off: the list stays visible");

    press(&mut app, ctrl('\\'));
    press(&mut app, key(KeyCode::Char('h')));
    assert_eq!(app.notice.as_deref(), Some("auto-hide on"));
    press(&mut app, ctrl('\\'));
    assert!(app.list_hidden(), "auto-hide back on hides the list again");
}

#[test]
fn h_in_terminal_focus_reaches_the_pty_and_does_not_toggle() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    let effects = press(&mut app, key(KeyCode::Char('h')));

    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"h".to_vec() }]
    );
    assert!(app.list_hidden(), "the toggle is list-scoped, not global");
}

#[test]
fn quit_is_immediate_when_idle_but_confirmed_when_sessions_are_running() {
    // No running sessions: 'q' quits immediately.
    let mut app = App::new(vec![meta("s1", "one")]);
    assert_eq!(press(&mut app, key(KeyCode::Char('q'))), vec![Effect::Quit]);

    // With a running session: 'q' asks first; 'y' quits, Esc cancels.
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    press(&mut app, ctrl('\\')); // back to list

    assert!(press(&mut app, key(KeyCode::Char('q'))).is_empty());
    assert_eq!(app.overlay, Overlay::ConfirmQuit);
    assert!(press(&mut app, key(KeyCode::Esc)).is_empty());
    assert_eq!(app.overlay, Overlay::None);

    // The prompt says y/N: Enter must take the safe default and cancel.
    press(&mut app, key(KeyCode::Char('q')));
    assert!(press(&mut app, key(KeyCode::Enter)).is_empty());
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.has_running_sessions(), "Enter must not confirm a kill");

    press(&mut app, key(KeyCode::Char('q')));
    assert_eq!(press(&mut app, key(KeyCode::Char('y'))), vec![Effect::Quit]);
}

#[test]
fn ctrl_k_kills_the_selected_running_session_after_confirmation() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();
    press(&mut app, ctrl('\\'));

    // Ctrl+K on a running session asks for confirmation.
    assert!(press(&mut app, ctrl('k')).is_empty());
    assert_eq!(app.overlay, Overlay::ConfirmKill { run_id });

    let effects = press(&mut app, key(KeyCode::Char('y')));
    assert_eq!(effects, vec![Effect::Kill { run_id }]);
    assert_eq!(app.overlay, Overlay::None);
    assert!(!app.is_running(run_id));
    assert_eq!(app.attached_run(), None, "killed session must detach");

    // Ctrl+K on a non-running session does nothing.
    assert!(press(&mut app, ctrl('k')).is_empty());
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn launch_picker_starts_a_fresh_agent_in_a_known_project_dir() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let mut a = meta("s1", "one");
    a.cwd = dir_a.path().to_str().unwrap().into();
    let mut b = meta("s2", "two");
    b.cwd = dir_b.path().to_str().unwrap().into();
    let mut a2 = meta("s3", "three");
    a2.cwd = a.cwd.clone(); // duplicate cwd, should be deduped
    let expected_b = std::fs::canonicalize(dir_b.path())
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut app = App::new(vec![a, b, a2]);

    press(&mut app, key(KeyCode::Char('n')));
    assert!(matches!(app.overlay, Overlay::LaunchPicker(_)));

    // Newest-first, deduped: proj-a then proj-b. Move down to proj-b,
    // toggle agent to codex, launch.
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Tab));
    let effects = press(&mut app, key(KeyCode::Enter));

    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.program, "codex");
            assert_eq!(spec.cwd, expected_b);
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.focus, Focus::Terminal);
    // A provisional entry for the new session tops the list.
    assert_eq!(app.roster().rows()[0].cwd(), expected_b);
    assert!(app.roster().rows()[0].is_provisional());
    assert_eq!(app.roster().selected(), 0);
}

#[test]
fn launch_picker_accepts_a_typed_path_that_matches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Char('n')));
    for c in path.chars() {
        press(&mut app, key(KeyCode::Char(c)));
    }
    let effects = press(&mut app, key(KeyCode::Enter));
    match &effects[..] {
        [Effect::Spawn { spec, .. }] => assert_eq!(spec.cwd, canonical.to_str().unwrap()),
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[test]
fn launch_uses_the_canonical_path_so_adoption_can_match_transcripts() {
    // Transcripts record the agent's resolved getcwd; a symlinked or
    // dot-riddled picker path must be canonicalized or the provisional
    // row never matches the scanned transcript.
    let dir = tempfile::tempdir().unwrap();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    let typed = format!("{}/.", dir.path().display());

    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Char('n')));
    for c in typed.chars() {
        press(&mut app, key(KeyCode::Char(c)));
    }
    let effects = press(&mut app, key(KeyCode::Enter));

    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.cwd, canonical.to_str().unwrap());
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
    assert_eq!(app.roster().rows()[0].cwd(), canonical.to_str().unwrap());
}

#[test]
fn typed_existing_path_beats_a_substring_match_unless_user_navigated() {
    // Known dir /X/sub contains the typed text "/X"; the user typed an
    // exact existing path and expects exactly it.
    let parent = tempfile::tempdir().unwrap();
    let sub = parent.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let mut known = meta("s1", "one");
    known.cwd = sub.to_str().unwrap().into();

    let type_path = |app: &mut App, path: &str| {
        for c in path.chars() {
            press(app, key(KeyCode::Char(c)));
        }
    };

    let mut app = App::new(vec![known.clone()]);
    press(&mut app, key(KeyCode::Char('n')));
    type_path(&mut app, parent.path().to_str().unwrap());
    let effects = press(&mut app, key(KeyCode::Enter));
    let canonical_parent = std::fs::canonicalize(parent.path()).unwrap();
    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.cwd, canonical_parent.to_str().unwrap());
        }
        other => panic!("expected Spawn, got {other:?}"),
    }

    // But explicitly navigating to a filtered match picks the match.
    let mut app = App::new(vec![known.clone()]);
    press(&mut app, key(KeyCode::Char('n')));
    type_path(&mut app, parent.path().to_str().unwrap());
    press(&mut app, key(KeyCode::Down));
    let effects = press(&mut app, key(KeyCode::Enter));
    let canonical_sub = std::fs::canonicalize(&sub).unwrap();
    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.cwd, canonical_sub.to_str().unwrap());
        }
        other => panic!("expected Spawn, got {other:?}"),
    }

    // Editing the input after navigating invalidates the navigation:
    // the freshly typed existing path wins again.
    let mut app = App::new(vec![known]);
    press(&mut app, key(KeyCode::Char('n')));
    press(&mut app, key(KeyCode::Down)); // navigate first...
    type_path(&mut app, parent.path().to_str().unwrap()); // ...then type
    let effects = press(&mut app, key(KeyCode::Enter));
    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.cwd, canonical_parent.to_str().unwrap());
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
}

#[test]
fn launch_picker_refuses_a_directory_that_does_not_exist() {
    // A session whose cwd has since been deleted (temp dirs are common
    // in history) must not silently launch the agent in $HOME.
    let mut gone = meta("s1", "one");
    gone.cwd = "/tmp/definitely-gone-e2e-dir".into();
    let mut app = App::new(vec![gone]);

    press(&mut app, key(KeyCode::Char('n')));
    let effects = press(&mut app, key(KeyCode::Enter));

    assert!(effects.is_empty(), "must not spawn into a missing dir");
    assert!(
        matches!(app.overlay, Overlay::LaunchPicker(_)),
        "picker stays open so the user can pick something else"
    );
}

#[test]
fn page_up_scrolls_back_and_any_other_key_snaps_to_live() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    assert_eq!(app.scroll_offset, 0);
    press(&mut app, key(KeyCode::PageUp));
    let offset = app.scroll_offset;
    assert!(offset > 0);
    press(&mut app, key(KeyCode::PageUp));
    assert!(app.scroll_offset > offset);
    press(&mut app, key(KeyCode::PageDown));
    assert_eq!(app.scroll_offset, offset);

    // Any non-scroll key snaps back to live and still reaches the PTY.
    let effects = press(&mut app, key(KeyCode::Char('x')));
    assert_eq!(app.scroll_offset, 0);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"x".to_vec() }]
    );
}

#[test]
fn resume_refuses_a_session_whose_cwd_no_longer_exists() {
    // portable-pty silently falls back to $HOME for a missing cwd, so
    // the agent would resume against the wrong tree.
    let mut gone = meta("s1", "one");
    gone.cwd = "/tmp/definitely-gone-e2e-dir".into();
    let mut app = App::new(vec![gone]);

    let effects = press(&mut app, key(KeyCode::Enter));

    assert!(effects.is_empty(), "must not spawn into a missing dir");
    assert_eq!(app.focus, Focus::List);
    assert!(app.attached_run().is_none());
    assert!(app.notice.is_some(), "user should see why nothing happened");
}

// The pure adoption permutations (snapshot exclusion, ambiguity
// hold-offs, provisional-row lifecycle, selection-by-identity) are
// covered directly in roster_test.rs; this test proves the wiring
// from picker launch through rescan to re-attach.
#[test]
fn rescan_adopts_the_real_transcript_into_the_provisional_row() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Char('n')));
    press(&mut app, key(KeyCode::Enter)); // launch claude in /tmp
    let run_id = app.attached_run().unwrap();

    // The watcher discovers the transcript the new agent just created:
    // same agent + cwd, written after the launch. Transcripts record
    // the resolved getcwd, which is what the launch stored too.
    let mut real = meta("real-id", "the actual prompt");
    real.cwd = std::fs::canonicalize("/tmp")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    real.timestamp = chrono::Utc::now() + chrono::Duration::seconds(1);
    app.update_sessions(vec![real, meta("s1", "one")]);

    // One row, not two: the provisional row became the real one.
    assert!(!app.roster().rows().iter().any(|r| r.is_provisional()));
    assert_eq!(
        app.roster()
            .rows()
            .iter()
            .filter(|r| r.transcript_id() == Some("real-id"))
            .count(),
        1
    );
    assert_eq!(
        app.run_id_for("real-id"),
        Some(run_id),
        "the live PTY now belongs to the real session id"
    );

    // Selecting the adopted row re-attaches instead of double-resuming.
    press(&mut app, ctrl('\\'));
    let pos = app
        .roster()
        .rows()
        .iter()
        .position(|r| r.transcript_id() == Some("real-id"))
        .unwrap();
    while app.roster().selected() < pos {
        press(&mut app, key(KeyCode::Down));
    }
    let effects = press(&mut app, key(KeyCode::Enter));
    assert!(effects.is_empty(), "attach must not spawn a second resume");
    assert_eq!(app.attached_run(), Some(run_id));
}

#[test]
fn after_fork_adoption_the_original_row_resumes_fresh() {
    // `claude --resume` forked: the run followed the new transcript id.
    // The original row is historical again — Enter on it must be a
    // fresh resume, not an attach to the fork's terminal (and never a
    // second resume of the fork's conversation).
    let s1 = meta("s1", "one");
    let mut app = App::new(vec![s1.clone()]);
    press(&mut app, key(KeyCode::Enter)); // resume s1
    let run_id = app.attached_run().unwrap();

    let mut fork = meta("fork", "one (forked)");
    fork.timestamp = chrono::Utc::now() + chrono::Duration::seconds(1);
    app.update_sessions(vec![fork, s1]);

    assert_eq!(app.run_id_for("fork"), Some(run_id), "the run followed the fork");
    assert_eq!(app.run_id_for("s1"), None);

    press(&mut app, ctrl('\\')); // back to the list; selection sits on the fork
    press(&mut app, key(KeyCode::Down)); // original row
    let effects = press(&mut app, key(KeyCode::Enter));
    match &effects[..] {
        [Effect::Spawn { run_id: new_run, spec }] => {
            assert_eq!(spec.args, vec!["--resume", "s1"]);
            assert_ne!(*new_run, run_id, "a fresh run, distinct from the fork's");
        }
        other => panic!("expected one Spawn effect, got {other:?}"),
    }
}

#[test]
fn a_session_whose_child_exits_detaches_and_stops_showing_as_running() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    app.mark_exited(run_id);

    assert!(!app.is_running(run_id));
    assert_eq!(app.attached_run(), None);
    assert_eq!(app.focus, Focus::List);
}

#[test]
fn paste_is_bracketed_only_when_the_child_enabled_bracketed_paste() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    // Child requested bracketed paste (DECSET 2004): wrap.
    let effects = paste(&mut app, "hello\nworld", true);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal {
            run_id,
            bytes: b"\x1b[200~hello\nworld\x1b[201~".to_vec()
        }]
    );

    // Child did not: the delimiters would arrive as literal input.
    let effects = paste(&mut app, "hello", false);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"hello".to_vec() }]
    );

    // Pasting while the list is focused does nothing.
    press(&mut app, ctrl('\\'));
    assert!(paste(&mut app, "x", true).is_empty());
}

#[test]
fn pasting_into_the_launch_picker_fills_the_path_field() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Char('n')));

    let effects = paste(&mut app, "/tmp/some\nproject", false);

    assert!(effects.is_empty());
    let Overlay::LaunchPicker(picker) = &app.overlay else {
        panic!("picker should still be open");
    };
    // Control chars (including the newline) must not survive.
    assert_eq!(picker.input(), "/tmp/someproject");
}

#[test]
fn switching_sessions_always_lands_on_live_output_not_old_scrollback() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    press(&mut app, key(KeyCode::Enter)); // resume s1
    press(&mut app, key(KeyCode::PageUp));
    assert!(app.scroll_offset > 0);

    // Resuming another session must start at live output.
    press(&mut app, ctrl('\\'));
    press(&mut app, key(KeyCode::Down));
    press(&mut app, key(KeyCode::Enter)); // resume s2 (fresh spawn)
    assert_eq!(app.scroll_offset, 0, "fresh spawn must not inherit scrollback");

    // Same when re-attaching an already-running session.
    press(&mut app, key(KeyCode::PageUp));
    press(&mut app, ctrl('\\'));
    press(&mut app, key(KeyCode::Up));
    press(&mut app, key(KeyCode::Enter)); // attach running s1
    assert_eq!(app.scroll_offset, 0, "attach must not inherit scrollback");
}

// The full key-encoding table (control keys, F-keys, xterm modifiers,
// legacy Ctrl quirks) is covered in tests/input_test.rs against the
// input module; this test proves the wiring — encoded bytes reach the
// attached PTY, and the child's modes arrive via the parameter.
#[test]
fn terminal_keys_are_encoded_for_the_childs_modes_and_written_to_the_pty() {
    let mut app = App::new(vec![meta("s1", "one")]);
    press(&mut app, key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    let effects = press(&mut app, key(KeyCode::Up));
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"\x1b[A".to_vec() }]
    );

    // With DECCKM set by the child, the same key encodes as SS3.
    let decckm = TermModes { app_cursor: true, ..Default::default() };
    let effects = app.handle_key(key(KeyCode::Up), decckm);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"\x1bOA".to_vec() }]
    );
}
