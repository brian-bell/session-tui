use chrono::{Duration, Utc};
use session_tui::roster::Roster;
use session_tui::sessions::{Agent, SessionMeta};

fn meta(id: &str, cwd: &str) -> SessionMeta {
    SessionMeta {
        id: id.into(),
        agent: Agent::Claude,
        cwd: cwd.into(),
        title: format!("title for {id}"),
        timestamp: Utc::now(),
    }
}

/// A transcript written after any launch in the test body.
fn post_launch(mut m: SessionMeta) -> SessionMeta {
    m.timestamp = Utc::now() + Duration::seconds(1);
    m
}

#[test]
fn new_roster_mirrors_scan_order_and_selects_the_top() {
    let roster = Roster::new(vec![meta("a", "/p1"), meta("b", "/p2")]);

    assert_eq!(roster.rows().len(), 2);
    assert_eq!(roster.rows()[0].transcript_id(), Some("a"));
    assert_eq!(roster.rows()[1].transcript_id(), Some("b"));
    assert!(!roster.rows()[0].is_provisional());
    assert_eq!(roster.rows()[0].run_id(), None);
    assert_eq!(roster.selected(), 0);
    assert!(!roster.has_running());
}

#[test]
fn move_selection_clamps_to_the_list() {
    let mut roster = Roster::new(vec![meta("a", "/p"), meta("b", "/q")]);
    roster.move_selection(-1);
    assert_eq!(roster.selected(), 0);
    roster.move_selection(5);
    assert_eq!(roster.selected(), 1);

    let mut empty = Roster::new(Vec::new());
    empty.move_selection(1);
    assert_eq!(empty.selected(), 0);
}

#[test]
fn launch_puts_a_selected_provisional_row_on_top_with_a_fresh_run_id() {
    let mut roster = Roster::new(vec![meta("a", "/p1")]);

    let (run_id, spec) = roster.launch(Agent::Codex, "/p2");

    assert_eq!(spec.program, "codex");
    assert!(spec.args.is_empty());
    assert_eq!(spec.cwd, "/p2");
    let row = &roster.rows()[0];
    assert!(row.is_provisional());
    assert_eq!(row.transcript_id(), None);
    assert_eq!(row.run_id(), Some(run_id));
    assert_eq!(row.title(), "(new session)");
    assert_eq!(row.cwd(), "/p2");
    assert_eq!(roster.selected(), 0);
    assert!(roster.is_running(run_id));
    assert!(roster.has_running());

    let (second, _) = roster.launch(Agent::Claude, "/p3");
    assert_ne!(run_id, second, "every launch gets its own run id");
}

#[test]
fn resume_selected_spawns_once_and_never_double_spawns() {
    let mut roster = Roster::new(vec![meta("a", "/proj")]);

    let (run_id, spec) = roster.resume_selected().expect("first resume spawns");
    assert_eq!(spec.program, "claude");
    assert_eq!(spec.args, vec!["--resume", "a"]);
    assert_eq!(spec.cwd, "/proj");
    assert_eq!(roster.rows()[0].run_id(), Some(run_id));

    assert!(
        roster.resume_selected().is_none(),
        "a running row must not spawn a second resume"
    );
}

#[test]
fn absorb_scan_keeps_the_provisional_row_when_only_pre_launch_transcripts_return() {
    // A pre-existing transcript in the same agent+cwd gets its mtime
    // bumped after our launch (someone works on it outside the TUI).
    // The launch snapshot says it was already known: no adoption.
    let mut roster = Roster::new(vec![meta("a", "/p")]);
    let (run_id, _) = roster.launch(Agent::Claude, "/p");

    roster.absorb_scan(vec![post_launch(meta("a", "/p"))]);

    assert!(roster.rows()[0].is_provisional(), "provisional row stays on top");
    assert_eq!(roster.rows()[0].run_id(), Some(run_id));
    let a = roster.rows().iter().find(|r| r.transcript_id() == Some("a")).unwrap();
    assert_eq!(a.run_id(), None, "a pre-launch transcript must not be adopted");
    assert_eq!(roster.selected(), 0, "selection follows the provisional row");
}

#[test]
fn absorb_scan_adopts_a_unique_post_launch_transcript() {
    let mut roster = Roster::new(vec![meta("a", "/p")]);
    let (run_id, _) = roster.launch(Agent::Claude, "/p");

    roster.absorb_scan(vec![post_launch(meta("fresh", "/p")), meta("a", "/p")]);

    assert!(
        !roster.rows().iter().any(|r| r.is_provisional()),
        "the placeholder became the real row"
    );
    let fresh: Vec<_> = roster
        .rows()
        .iter()
        .filter(|r| r.transcript_id() == Some("fresh"))
        .collect();
    assert_eq!(fresh.len(), 1, "the session shows exactly once");
    assert_eq!(fresh[0].run_id(), Some(run_id), "the live PTY moved over");
    assert_eq!(
        roster.selected_row().unwrap().transcript_id(),
        Some("fresh"),
        "selection follows the adopted identity"
    );
}

#[test]
fn adoption_requires_the_same_agent_and_cwd() {
    let mut roster = Roster::new(Vec::new());
    roster.launch(Agent::Claude, "/p");

    let mut codex_same_cwd = post_launch(meta("x", "/p"));
    codex_same_cwd.agent = Agent::Codex;
    let claude_other_cwd = post_launch(meta("y", "/q"));
    roster.absorb_scan(vec![codex_same_cwd, claude_other_cwd]);

    assert!(roster.rows().iter().any(|r| r.is_provisional()));
    assert!(roster
        .rows()
        .iter()
        .all(|r| r.is_provisional() || r.run_id().is_none()));
}

#[test]
fn adoption_holds_off_with_two_sibling_placeholders() {
    // Two provisional launches in the same agent+cwd: a single new
    // transcript cannot be attributed to either process.
    let mut roster = Roster::new(Vec::new());
    roster.launch(Agent::Claude, "/p");
    roster.launch(Agent::Claude, "/p");

    roster.absorb_scan(vec![post_launch(meta("fresh", "/p"))]);

    assert_eq!(
        roster.rows().iter().filter(|r| r.is_provisional()).count(),
        2,
        "both placeholders must survive until the match is unambiguous"
    );
    let fresh = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh.run_id(), None);
}

#[test]
fn adoption_holds_off_with_two_candidate_transcripts() {
    let mut roster = Roster::new(Vec::new());
    roster.launch(Agent::Claude, "/p");

    roster.absorb_scan(vec![post_launch(meta("c1", "/p")), post_launch(meta("c2", "/p"))]);

    assert_eq!(roster.rows().iter().filter(|r| r.is_provisional()).count(), 1);
    assert!(roster
        .rows()
        .iter()
        .all(|r| r.is_provisional() || r.run_id().is_none()));
}

#[test]
fn mark_exited_drops_the_provisional_row_and_selection_follows_identity() {
    let mut roster = Roster::new(vec![meta("a", "/p"), meta("b", "/q")]);
    let (run_id, _) = roster.launch(Agent::Claude, "/p"); // on top, selected
    roster.move_selection(2); // select "b", below the provisional row

    roster.mark_exited(run_id);

    assert!(
        !roster.rows().iter().any(|r| r.is_provisional()),
        "stale provisional rows must not linger (they can't be resumed)"
    );
    assert_eq!(
        roster.selected_row().unwrap().transcript_id(),
        Some("b"),
        "selection tracks the session, not the index"
    );
    assert!(!roster.is_running(run_id));

    // And a later rescan must not resurrect anything.
    roster.absorb_scan(vec![meta("a", "/p"), meta("b", "/q")]);
    assert!(!roster.rows().iter().any(|r| r.is_provisional()));
}

#[test]
fn mark_exited_clears_the_run_marker_but_keeps_a_transcript_row() {
    let mut roster = Roster::new(vec![meta("a", "/p")]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.mark_exited(run_id);

    assert_eq!(roster.rows().len(), 1, "a transcript row survives its process");
    assert_eq!(roster.rows()[0].run_id(), None);
    assert!(!roster.has_running());
}

#[test]
fn a_resumed_row_keeps_its_run_across_rescans() {
    let mut roster = Roster::new(vec![meta("a", "/p")]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![post_launch(meta("a", "/p")), meta("b", "/q")]);

    let a = roster.rows().iter().find(|r| r.transcript_id() == Some("a")).unwrap();
    assert_eq!(a.run_id(), Some(run_id));
}

#[test]
fn a_running_row_missing_from_a_scan_is_not_dropped() {
    // A transient parse failure can make a transcript vanish from one
    // scan. Dropping the row would orphan its live PTY with no way to
    // reattach; the row must ride out the gap.
    let mut roster = Roster::new(vec![meta("a", "/p")]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![meta("b", "/q")]);

    let a = roster.rows().iter().find(|r| r.transcript_id() == Some("a")).unwrap();
    assert_eq!(a.run_id(), Some(run_id));
    assert!(roster.is_running(run_id));
}

#[test]
fn selection_follows_a_transcript_when_a_rescan_reorders_rows() {
    let mut roster = Roster::new(vec![meta("a", "/p"), meta("b", "/q")]);
    roster.move_selection(1); // select "b"

    roster.absorb_scan(vec![meta("c", "/r"), meta("b", "/q"), meta("a", "/p")]);

    assert_eq!(roster.selected_row().unwrap().transcript_id(), Some("b"));
}

#[test]
fn known_dirs_dedupe_newest_first_and_skip_provisional_rows() {
    let mut roster = Roster::new(vec![meta("a", "/p1"), meta("b", "/p2"), meta("c", "/p1")]);
    roster.launch(Agent::Claude, "/px");

    assert_eq!(roster.known_dirs(), vec!["/p1".to_string(), "/p2".to_string()]);
}
