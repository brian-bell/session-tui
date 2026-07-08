use session_tui::picker::PickerState;
use session_tui::sessions::Agent;

fn picker(dirs: &[&str]) -> PickerState {
    PickerState::new(dirs.iter().map(|d| d.to_string()).collect())
}

#[test]
fn edit_appends_and_resets_highlight_and_navigation() {
    // A known dir contains the typed text; the user navigates to it,
    // then keeps typing. The old navigation pointed into a now-stale
    // filtered list — the freshly typed path speaks for itself.
    let parent = tempfile::tempdir().unwrap();
    let sub = parent.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let mut p = picker(&[sub.to_str().unwrap()]);

    for c in parent.path().to_str().unwrap().chars() {
        p.edit(c);
    }
    p.move_highlight(1); // single match: clamps to it, but marks navigation
    assert_eq!(p.chosen_dir().as_deref(), Some(sub.to_str().unwrap()));

    p.edit('/'); // editing invalidates the navigation
    assert_eq!(p.highlighted(), 0);
    let typed = format!("{}/", parent.path().to_str().unwrap());
    assert_eq!(
        p.chosen_dir().as_deref(),
        Some(typed.as_str()),
        "the freshly typed existing path wins again"
    );
}

#[test]
fn backspace_removes_and_resets_navigation() {
    let parent = tempfile::tempdir().unwrap();
    let sub = parent.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let mut p = picker(&[sub.to_str().unwrap()]);

    for c in parent.path().to_str().unwrap().chars() {
        p.edit(c);
    }
    p.move_highlight(1);
    p.edit('x');
    p.backspace(); // input is the parent path again, navigation cleared

    assert_eq!(p.input(), parent.path().to_str().unwrap());
    assert_eq!(p.highlighted(), 0);
    assert_eq!(
        p.chosen_dir().as_deref(),
        Some(parent.path().to_str().unwrap()),
        "typed existing path wins once the navigation was invalidated"
    );
}

#[test]
fn paste_filters_control_characters_and_resets() {
    let mut p = picker(&["/proj/a", "/proj/b"]);
    p.move_highlight(1);

    p.paste("/tmp/some\nproj\x1bect");

    assert_eq!(p.input(), "/tmp/someproject");
    assert_eq!(p.highlighted(), 0);
}

#[test]
fn move_highlight_clamps_to_the_filtered_matches() {
    let mut p = picker(&["/proj/a", "/proj/b", "/other"]);
    for c in "/proj".chars() {
        p.edit(c);
    }
    assert_eq!(p.matches().len(), 2);

    p.move_highlight(5);
    assert_eq!(p.highlighted(), 1, "clamped to the last match");
    p.move_highlight(-3);
    assert_eq!(p.highlighted(), 0, "saturates at the top");
}

#[test]
fn move_highlight_on_empty_matches_is_a_no_op() {
    // With no matches, navigation must not mark an explicit choice:
    // chosen_dir keeps returning the raw input either way, and the
    // highlight has nothing to point at.
    let dir = tempfile::tempdir().unwrap();
    let mut p = picker(&["/known"]);
    for c in dir.path().to_str().unwrap().chars() {
        p.edit(c);
    }
    assert!(p.matches().is_empty());

    let before = p.chosen_dir();
    p.move_highlight(1);
    p.move_highlight(-1);

    assert_eq!(p.highlighted(), 0);
    assert_eq!(p.chosen_dir(), before, "empty-match navigation changes nothing");
}

#[test]
fn navigating_prefers_the_highlighted_match_over_the_typed_path() {
    // /parent exists and is typed; /parent/sub is a known dir that
    // contains the typed text. An explicit navigation outranks the
    // typed path.
    let parent = tempfile::tempdir().unwrap();
    let sub = parent.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let mut p = picker(&[sub.to_str().unwrap()]);

    for c in parent.path().to_str().unwrap().chars() {
        p.edit(c);
    }
    assert_eq!(
        p.chosen_dir().as_deref(),
        Some(parent.path().to_str().unwrap()),
        "before navigating, the typed existing path wins"
    );

    p.move_highlight(1);
    assert_eq!(
        p.chosen_dir().as_deref(),
        Some(sub.to_str().unwrap()),
        "after navigating, the highlighted match wins"
    );
}

#[test]
fn toggle_agent_flips_between_claude_and_codex() {
    let mut p = picker(&[]);
    assert_eq!(p.agent(), Agent::Claude);
    p.toggle_agent();
    assert_eq!(p.agent(), Agent::Codex);
    p.toggle_agent();
    assert_eq!(p.agent(), Agent::Claude);
}
