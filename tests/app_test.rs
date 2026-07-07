use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use session_tui::app::{App, Effect, Focus, Overlay};
use session_tui::sessions::{Agent, SessionMeta};

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

fn alt(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT)
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

    // Legacy terminals deliver Ctrl+\ (byte 0x1C) as Ctrl+4; it must
    // toggle too, and must NOT leak through to the PTY as a key.
    let effects = app.handle_key(ctrl('4'));
    assert!(effects.is_empty());
    assert_eq!(app.focus, Focus::List);
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

    // The prompt says y/N: Enter must take the safe default and cancel.
    app.handle_key(key(KeyCode::Char('q')));
    assert!(app.handle_key(key(KeyCode::Enter)).is_empty());
    assert_eq!(app.overlay, Overlay::None);
    assert!(app.has_running_sessions(), "Enter must not confirm a kill");

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
            assert_eq!(spec.cwd, expected_b);
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.focus, Focus::Terminal);
    // A provisional entry for the new session tops the list.
    assert_eq!(app.sessions[0].cwd, expected_b);
    assert_eq!(app.selected, 0);
}

#[test]
fn launch_picker_accepts_a_typed_path_that_matches_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    let canonical = std::fs::canonicalize(dir.path()).unwrap();
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));
    for c in path.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let effects = app.handle_key(key(KeyCode::Enter));
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
    app.handle_key(key(KeyCode::Char('n')));
    for c in typed.chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    let effects = app.handle_key(key(KeyCode::Enter));

    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.cwd, canonical.to_str().unwrap());
        }
        other => panic!("expected Spawn, got {other:?}"),
    }
    assert_eq!(app.sessions[0].cwd, canonical.to_str().unwrap());
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
            app.handle_key(key(KeyCode::Char(c)));
        }
    };

    let mut app = App::new(vec![known.clone()]);
    app.handle_key(key(KeyCode::Char('n')));
    type_path(&mut app, parent.path().to_str().unwrap());
    let effects = app.handle_key(key(KeyCode::Enter));
    let canonical_parent = std::fs::canonicalize(parent.path()).unwrap();
    match &effects[..] {
        [Effect::Spawn { spec, .. }] => {
            assert_eq!(spec.cwd, canonical_parent.to_str().unwrap());
        }
        other => panic!("expected Spawn, got {other:?}"),
    }

    // But explicitly navigating to a filtered match picks the match.
    let mut app = App::new(vec![known.clone()]);
    app.handle_key(key(KeyCode::Char('n')));
    type_path(&mut app, parent.path().to_str().unwrap());
    app.handle_key(key(KeyCode::Down));
    let effects = app.handle_key(key(KeyCode::Enter));
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
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Down)); // navigate first...
    type_path(&mut app, parent.path().to_str().unwrap()); // ...then type
    let effects = app.handle_key(key(KeyCode::Enter));
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

    app.handle_key(key(KeyCode::Char('n')));
    let effects = app.handle_key(key(KeyCode::Enter));

    assert!(effects.is_empty(), "must not spawn into a missing dir");
    assert!(
        matches!(app.overlay, Overlay::LaunchPicker(_)),
        "picker stays open so the user can pick something else"
    );
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

    // A rescan of only pre-launch transcripts (the new agent hasn't
    // written its own yet) must keep the provisional row on top.
    let old = |id, title| {
        let mut m = meta(id, title);
        m.timestamp = chrono::Utc::now() - chrono::Duration::hours(1);
        m
    };
    app.update_sessions(vec![old("s3", "newest"), old("s1", "one"), old("s2", "two")]);

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
fn resume_refuses_a_session_whose_cwd_no_longer_exists() {
    // portable-pty silently falls back to $HOME for a missing cwd, so
    // the agent would resume against the wrong tree.
    let mut gone = meta("s1", "one");
    gone.cwd = "/tmp/definitely-gone-e2e-dir".into();
    let mut app = App::new(vec![gone]);

    let effects = app.handle_key(key(KeyCode::Enter));

    assert!(effects.is_empty(), "must not spawn into a missing dir");
    assert_eq!(app.focus, Focus::List);
    assert!(app.attached_run().is_none());
    assert!(app.notice.is_some(), "user should see why nothing happened");
}

#[test]
fn provisional_entry_disappears_when_its_process_exits() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter)); // launch in known dir (/tmp)
    let run_id = app.attached_run().unwrap();
    assert!(app.sessions[0].id.starts_with("live-"));

    app.mark_exited(run_id);
    assert!(
        !app.sessions.iter().any(|m| m.id.starts_with("live-")),
        "stale provisional rows must not linger (they can't be resumed)"
    );

    // And a rescan must not resurrect anything either.
    app.update_sessions(vec![meta("s1", "one")]);
    assert!(!app.sessions.iter().any(|m| m.id.starts_with("live-")));
}

#[test]
fn adoption_ignores_transcripts_that_existed_before_the_launch() {
    // Another session in the same cwd, active outside this TUI, gets
    // its mtime bumped after our launch. It must not steal the
    // provisional row: only a transcript we had never seen qualifies.
    let canonical_tmp = std::fs::canonicalize("/tmp")
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut outside = meta("outside-id", "unrelated session");
    outside.cwd = canonical_tmp.clone();

    let mut app = App::new(vec![outside.clone()]);
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter)); // launch in /tmp
    let run_id = app.attached_run().unwrap();

    // Rescan: the pre-existing transcript now has a newer mtime.
    outside.timestamp = chrono::Utc::now() + chrono::Duration::seconds(1);
    app.update_sessions(vec![outside.clone()]);
    assert_eq!(
        app.run_id_for("outside-id"),
        None,
        "a transcript known before launch must not be adopted"
    );
    assert!(app.sessions.iter().any(|m| m.id.starts_with("live-")));

    // A genuinely new transcript still adopts.
    let mut fresh = meta("fresh-id", "the launched session");
    fresh.cwd = canonical_tmp;
    fresh.timestamp = chrono::Utc::now() + chrono::Duration::seconds(1);
    app.update_sessions(vec![fresh, outside]);
    assert_eq!(app.run_id_for("fresh-id"), Some(run_id));
}

#[test]
fn adoption_holds_off_while_the_match_is_ambiguous() {
    let canonical_tmp = std::fs::canonicalize("/tmp")
        .unwrap()
        .to_string_lossy()
        .into_owned();

    // Two provisional launches in the same agent+cwd: a single new
    // transcript cannot be attributed to either process.
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ctrl('\\'));
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ctrl('\\'));

    let mut fresh = meta("fresh-id", "someone's session");
    fresh.cwd = canonical_tmp.clone();
    fresh.timestamp = chrono::Utc::now() + chrono::Duration::seconds(1);
    app.update_sessions(vec![fresh, meta("s1", "one")]);

    assert_eq!(app.run_id_for("fresh-id"), None, "ambiguous: two placeholders");
    assert_eq!(
        app.sessions.iter().filter(|m| m.id.starts_with("live-")).count(),
        2,
        "both placeholders must survive until the match is unambiguous"
    );

    // One placeholder but two candidate transcripts: also ambiguous.
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(ctrl('\\'));
    let mut c1 = meta("cand-1", "a");
    c1.cwd = canonical_tmp.clone();
    c1.timestamp = chrono::Utc::now() + chrono::Duration::seconds(1);
    let mut c2 = meta("cand-2", "b");
    c2.cwd = canonical_tmp;
    c2.timestamp = chrono::Utc::now() + chrono::Duration::seconds(2);
    app.update_sessions(vec![c2, c1, meta("s1", "one")]);

    assert_eq!(app.run_id_for("cand-1"), None);
    assert_eq!(app.run_id_for("cand-2"), None);
    assert_eq!(app.sessions.iter().filter(|m| m.id.starts_with("live-")).count(), 1);
}

#[test]
fn rescan_adopts_the_real_transcript_into_the_provisional_row() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter)); // launch claude in /tmp
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
    assert!(!app.sessions.iter().any(|m| m.id.starts_with("live-")));
    assert_eq!(app.sessions.iter().filter(|m| m.id == "real-id").count(), 1);
    assert_eq!(
        app.run_id_for("real-id"),
        Some(run_id),
        "the live PTY now belongs to the real session id"
    );

    // Selecting the adopted row re-attaches instead of double-resuming.
    app.handle_key(ctrl('\\'));
    let pos = app.sessions.iter().position(|m| m.id == "real-id").unwrap();
    while app.selected < pos {
        app.handle_key(key(KeyCode::Down));
    }
    let effects = app.handle_key(key(KeyCode::Enter));
    assert!(effects.is_empty(), "attach must not spawn a second resume");
    assert_eq!(app.attached_run(), Some(run_id));
}

#[test]
fn selection_follows_the_session_when_a_live_row_above_it_exits() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    // Launch a provisional row (lands on top as index 0).
    app.handle_key(key(KeyCode::Char('n')));
    app.handle_key(key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();
    app.handle_key(ctrl('\\'));

    // Select s2 (index 2, below the provisional row).
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Down));
    assert_eq!(app.sessions[app.selected].id, "s2");

    // The provisional process exits; its row above is removed.
    app.mark_exited(run_id);
    assert_eq!(
        app.sessions[app.selected].id, "s2",
        "selection must track the session, not the index"
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
fn paste_is_bracketed_only_when_the_child_enabled_bracketed_paste() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Enter));
    let run_id = app.attached_run().unwrap();

    // Child requested bracketed paste (DECSET 2004): wrap.
    let effects = app.handle_paste("hello\nworld", true);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal {
            run_id,
            bytes: b"\x1b[200~hello\nworld\x1b[201~".to_vec()
        }]
    );

    // Child did not: the delimiters would arrive as literal input.
    let effects = app.handle_paste("hello", false);
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"hello".to_vec() }]
    );

    // Pasting while the list is focused does nothing.
    app.handle_key(ctrl('\\'));
    assert!(app.handle_paste("x", true).is_empty());
}

#[test]
fn pasting_into_the_launch_picker_fills_the_path_field() {
    let mut app = App::new(vec![meta("s1", "one")]);
    app.handle_key(key(KeyCode::Char('n')));

    let effects = app.handle_paste("/tmp/some\nproject", false);

    assert!(effects.is_empty());
    let Overlay::LaunchPicker(picker) = &app.overlay else {
        panic!("picker should still be open");
    };
    // Control chars (including the newline) must not survive.
    assert_eq!(picker.input, "/tmp/someproject");
}

#[test]
fn switching_sessions_always_lands_on_live_output_not_old_scrollback() {
    let mut app = App::new(vec![meta("s1", "one"), meta("s2", "two")]);
    app.handle_key(key(KeyCode::Enter)); // resume s1
    app.handle_key(key(KeyCode::PageUp));
    assert!(app.scroll_offset > 0);

    // Resuming another session must start at live output.
    app.handle_key(ctrl('\\'));
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter)); // resume s2 (fresh spawn)
    assert_eq!(app.scroll_offset, 0, "fresh spawn must not inherit scrollback");

    // Same when re-attaching an already-running session.
    app.handle_key(key(KeyCode::PageUp));
    app.handle_key(ctrl('\\'));
    app.handle_key(key(KeyCode::Up));
    app.handle_key(key(KeyCode::Enter)); // attach running s1
    assert_eq!(app.scroll_offset, 0, "attach must not inherit scrollback");
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
        // Function keys, Insert, and modified arrows must pass through.
        (key(KeyCode::F(1)), b"\x1bOP"),
        (key(KeyCode::F(5)), b"\x1b[15~"),
        (key(KeyCode::F(12)), b"\x1b[24~"),
        (key(KeyCode::Insert), b"\x1b[2~"),
        (KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL), b"\x1b[1;5C"),
        (KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT), b"\x1b[1;2D"),
    ];
    for (k, want) in cases {
        let effects = app.handle_key(k);
        assert_eq!(
            effects,
            vec![Effect::WriteTerminal { run_id, bytes: want.to_vec() }],
            "for key {k:?}"
        );
    }

    // With DECCKM (application cursor mode) set by the child,
    // unmodified arrows switch to SS3 sequences.
    app.app_cursor = true;
    let effects = app.handle_key(key(KeyCode::Up));
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"\x1bOA".to_vec() }]
    );
    // Modified arrows keep the CSI 1;<mod> form even in DECCKM.
    let effects = app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));
    assert_eq!(
        effects,
        vec![Effect::WriteTerminal { run_id, bytes: b"\x1b[1;3A".to_vec() }]
    );
}
