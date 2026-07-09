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
fn a_transiently_missing_candidate_does_not_resolve_a_launch_ambiguity() {
    // Same terminal-ambiguity rule as fork adoption: two plausible
    // transcripts for one launch can never be told apart later, even
    // if a rescan transiently loses one of them.
    let mut roster = Roster::new(Vec::new());
    let (run_id, _) = roster.launch(Agent::Claude, "/p");
    let (c1, c2) = (post_launch(meta("c1", "/p")), post_launch(meta("c2", "/p")));
    roster.absorb_scan(vec![c1, c2.clone()]); // ambiguous

    roster.absorb_scan(vec![c2]); // c1 transiently gone

    assert_eq!(roster.rows().iter().filter(|r| r.is_provisional()).count(), 1);
    assert!(roster.is_running(run_id), "the run stays on the placeholder");
    let c2_row = roster.rows().iter().find(|r| r.transcript_id() == Some("c2")).unwrap();
    assert_eq!(c2_row.run_id(), None);
}

#[test]
fn an_ambiguous_launch_never_adopts_even_a_later_unique_transcript() {
    // Once ambiguity was observed, the launch's real transcript is
    // among the candidates already seen — a transcript first appearing
    // later necessarily belongs to someone else, however unique it
    // looks. (Excluding the old candidates and adopting the newcomer
    // would bind the run to a file this process cannot have written.)
    let mut roster = Roster::new(Vec::new());
    let (run_id, _) = roster.launch(Agent::Claude, "/p");
    let (c1, c2) = (post_launch(meta("c1", "/p")), post_launch(meta("c2", "/p")));
    roster.absorb_scan(vec![c1, c2]); // ambiguous

    roster.absorb_scan(vec![post_launch(meta("c3", "/p"))]);

    assert_eq!(roster.rows().iter().filter(|r| r.is_provisional()).count(), 1);
    assert!(roster.is_running(run_id));
    let c3 = roster.rows().iter().find(|r| r.transcript_id() == Some("c3")).unwrap();
    assert_eq!(c3.run_id(), None);
}

#[test]
fn an_ambiguous_launch_blocks_later_launches_until_its_process_exits() {
    // An ambiguous launch can never adopt, but while its process lives
    // any new transcript in the cwd could still be its late write — a
    // later launch adopting one would risk a wrong binding. The block
    // lifts when the contender exits.
    let mut roster = Roster::new(Vec::new());
    let (run_a, _) = roster.launch(Agent::Claude, "/p");
    let (c1, c2) = (post_launch(meta("c1", "/p")), post_launch(meta("c2", "/p")));
    roster.absorb_scan(vec![c1.clone(), c2.clone()]); // first launch goes ambiguous

    let (run_b, _) = roster.launch(Agent::Claude, "/p");
    let fresh = post_launch(meta("fresh", "/p"));
    roster.absorb_scan(vec![fresh.clone(), c1.clone(), c2.clone()]);
    let fresh_row = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh_row.run_id(), None, "fresh could be the contender's late write");

    roster.mark_exited(run_a);
    roster.absorb_scan(vec![fresh, c1, c2]);
    let fresh_row = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh_row.run_id(), Some(run_b));
}

#[test]
fn an_ambiguous_launch_blocks_its_sibling_until_its_process_exits() {
    // Launch A, a stray transcript lands between A and B, launch B:
    // one scan then makes A terminally ambiguous while B's fresh
    // transcript looks unique. But "fresh" is in A's plausible set too
    // — if A wrote it, adopting it for B is a wrong binding. A's live
    // process contaminates the cwd; B adopts only once A is gone.
    let mut roster = Roster::new(Vec::new());
    let (run_a, _) = roster.launch(Agent::Claude, "/p");
    let mut stray = meta("stray", "/p");
    stray.timestamp = roster.rows()[0].timestamp() + Duration::nanoseconds(1);
    let (run_b, _) = roster.launch(Agent::Claude, "/p");
    let fresh = post_launch(meta("fresh", "/p"));

    roster.absorb_scan(vec![fresh.clone(), stray.clone()]);
    assert_eq!(
        roster.rows().iter().filter(|r| r.is_provisional()).count(),
        2,
        "B must not adopt a transcript A might have written"
    );

    roster.mark_exited(run_a);
    roster.absorb_scan(vec![fresh, stray]);
    let fresh_row = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh_row.run_id(), Some(run_b), "the known contender is gone; B adopts");
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
fn absorb_scan_moves_the_run_to_a_forked_transcript_after_resume() {
    // `claude --resume` writes the continued conversation to a NEW
    // transcript id; the original file never updates again (hence the
    // reused meta with its pre-resume mtime). The run must follow the
    // fork or the original row stays "running" forever and the fork
    // row would spawn a second resume of the same conversation.
    let orig = meta("orig", "/p");
    let mut roster = Roster::new(vec![orig.clone()]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![post_launch(meta("fork", "/p")), orig]);

    let fork = roster.rows().iter().find(|r| r.transcript_id() == Some("fork")).unwrap();
    assert_eq!(fork.run_id(), Some(run_id), "the live PTY follows the fork");
    let orig = roster.rows().iter().find(|r| r.transcript_id() == Some("orig")).unwrap();
    assert_eq!(orig.run_id(), None, "the original row is no longer running");
    assert!(roster.is_running(run_id));
}

#[test]
fn selection_follows_the_run_to_the_forked_transcript() {
    let orig = meta("orig", "/p");
    let mut roster = Roster::new(vec![orig.clone()]);
    roster.resume_selected().unwrap();

    roster.absorb_scan(vec![post_launch(meta("fork", "/p")), orig]);

    assert_eq!(
        roster.selected_row().unwrap().transcript_id(),
        Some("fork"),
        "the user is attached to that terminal; selection follows it"
    );
}

#[test]
fn a_resumed_row_keeps_its_run_when_no_fork_appears() {
    // Agents that append to the same transcript (codex) never produce
    // a fork candidate; the resumed row must stay running as-is.
    let mut roster = Roster::new(vec![meta("orig", "/p")]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![meta("orig", "/p")]);

    assert_eq!(roster.rows()[0].run_id(), Some(run_id));
}

#[test]
fn fork_adoption_skips_transcripts_known_at_resume_time() {
    // A sibling session in the same cwd gets its mtime bumped by work
    // outside the TUI; the resume snapshot says it already existed.
    let orig = meta("orig", "/p");
    let mut roster = Roster::new(vec![orig.clone(), meta("other", "/p")]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![post_launch(meta("other", "/p")), orig]);

    let orig = roster.rows().iter().find(|r| r.transcript_id() == Some("orig")).unwrap();
    assert_eq!(orig.run_id(), Some(run_id), "the run stays home");
    let other = roster.rows().iter().find(|r| r.transcript_id() == Some("other")).unwrap();
    assert_eq!(other.run_id(), None);
}

#[test]
fn fork_adoption_holds_off_with_two_candidate_transcripts() {
    let orig = meta("orig", "/p");
    let mut roster = Roster::new(vec![orig.clone()]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![
        post_launch(meta("c1", "/p")),
        post_launch(meta("c2", "/p")),
        orig,
    ]);

    let orig = roster.rows().iter().find(|r| r.transcript_id() == Some("orig")).unwrap();
    assert_eq!(orig.run_id(), Some(run_id), "mtimes can't say which is the fork");
    assert!(roster
        .rows()
        .iter()
        .filter(|r| r.transcript_id() != Some("orig"))
        .all(|r| r.run_id().is_none()));
}

#[test]
fn fork_adoption_holds_off_with_two_pending_resumes_in_the_same_cwd() {
    // Two resumed sessions in one agent+cwd: a single new transcript
    // cannot be attributed to either process.
    let (r1, r2) = (meta("r1", "/p"), meta("r2", "/p"));
    let mut roster = Roster::new(vec![r1.clone(), r2.clone()]);
    let (run1, _) = roster.resume_selected().unwrap();
    roster.move_selection(1);
    let (run2, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![post_launch(meta("fork", "/p")), r1, r2]);

    let by_id = |id: &str| roster.rows().iter().find(|r| r.transcript_id() == Some(id)).unwrap();
    assert_eq!(by_id("r1").run_id(), Some(run1), "both runs must stay home");
    assert_eq!(by_id("r2").run_id(), Some(run2), "both runs must stay home");
    assert_eq!(by_id("fork").run_id(), None);
}

#[test]
fn a_launch_and_a_pending_resume_in_the_same_cwd_block_each_other() {
    // A provisional launch and a resumed session both wait for a new
    // transcript in the same agent+cwd; one new file can't be
    // attributed to either process, so neither side may adopt it.
    let orig = meta("orig", "/p");
    let mut roster = Roster::new(vec![orig.clone()]);
    let (resume_run, _) = roster.resume_selected().unwrap();
    let (launch_run, _) = roster.launch(Agent::Claude, "/p");

    roster.absorb_scan(vec![post_launch(meta("fresh", "/p")), orig]);

    assert_eq!(
        roster.rows().iter().filter(|r| r.is_provisional()).count(),
        1,
        "the placeholder must survive"
    );
    assert!(roster.is_running(launch_run));
    let orig = roster.rows().iter().find(|r| r.transcript_id() == Some("orig")).unwrap();
    assert_eq!(orig.run_id(), Some(resume_run), "the resume's run stays home");
    let fresh = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh.run_id(), None);
}

#[test]
fn a_codex_resume_never_adopts_a_new_transcript() {
    // codex appends to the resumed rollout in place; a new codex
    // transcript in the same cwd belongs to someone else, even before
    // the first append lands in a scan.
    let mut r = meta("r", "/p");
    r.agent = Agent::Codex;
    let mut roster = Roster::new(vec![r.clone()]);
    let (run_id, _) = roster.resume_selected().unwrap();

    let mut unrelated = post_launch(meta("n", "/p"));
    unrelated.agent = Agent::Codex;
    roster.absorb_scan(vec![unrelated, r]);

    let by_id = |id: &str| roster.rows().iter().find(|r| r.transcript_id() == Some(id)).unwrap();
    assert_eq!(by_id("r").run_id(), Some(run_id), "the run stays on the resumed row");
    assert_eq!(by_id("n").run_id(), None);
}

#[test]
fn an_in_place_append_after_resume_stops_competing_for_new_transcripts() {
    // An append to the resumed transcript itself proves the agent
    // continues in place rather than forking (codex-style). The row
    // must stop competing, or it would block launch adoption in this
    // cwd forever.
    let mut roster = Roster::new(vec![meta("r", "/p")]);
    let (resume_run, _) = roster.resume_selected().unwrap();
    roster.absorb_scan(vec![post_launch(meta("r", "/p"))]); // in-place append observed

    let (launch_run, _) = roster.launch(Agent::Claude, "/p");
    roster.absorb_scan(vec![post_launch(meta("fresh", "/p")), post_launch(meta("r", "/p"))]);

    let fresh = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh.run_id(), Some(launch_run), "the placeholder adopts unhindered");
    let r = roster.rows().iter().find(|r| r.transcript_id() == Some("r")).unwrap();
    assert_eq!(r.run_id(), Some(resume_run));
}

#[test]
fn resuming_a_candidate_does_not_resolve_a_fork_ambiguity() {
    // Two new transcripts appeared post-resume: either could be a's
    // fork. The user then resumes one of them (and it proves
    // in-place) — that says nothing about which file a's process
    // wrote, so treating the other as uniquely attributable would be
    // a guess. The run stays home.
    let a = meta("a", "/p");
    let mut roster = Roster::new(vec![a.clone()]);
    let (run_a, _) = roster.resume_selected().unwrap();
    let (d, e) = (post_launch(meta("d", "/p")), post_launch(meta("e", "/p")));
    roster.absorb_scan(vec![d, e.clone(), a.clone()]);

    roster.move_selection(-2); // scan order d, e, a; selection stayed on "a"
    assert_eq!(roster.selected_row().unwrap().transcript_id(), Some("d"));
    let (run_d, _) = roster.resume_selected().unwrap();
    roster.absorb_scan(vec![post_launch(meta("d", "/p")), e, a]);

    let by_id = |id: &str| roster.rows().iter().find(|r| r.transcript_id() == Some(id)).unwrap();
    assert_eq!(by_id("a").run_id(), Some(run_a), "the ambiguity never resolved");
    assert_eq!(by_id("d").run_id(), Some(run_d));
    assert_eq!(by_id("e").run_id(), None);
}

#[test]
fn a_fork_is_adopted_even_when_the_origin_vanishes_from_the_scan() {
    // The scan that discovers the fork can transiently lose the
    // original transcript (parse failure, deletion). The fork is
    // still the only plausible attribution; the orphaned origin row
    // must not squat on the run.
    let orig = meta("orig", "/p");
    let mut roster = Roster::new(vec![orig.clone()]);
    let (run_id, _) = roster.resume_selected().unwrap();

    roster.absorb_scan(vec![post_launch(meta("fork", "/p"))]);

    let fork = roster.rows().iter().find(|r| r.transcript_id() == Some("fork")).unwrap();
    assert_eq!(fork.run_id(), Some(run_id), "the run follows the fork");
    assert!(
        roster.rows().iter().all(|r| r.transcript_id() != Some("orig")),
        "a runless row missing from the scan has nothing to show"
    );
    assert_eq!(roster.selected_row().unwrap().transcript_id(), Some("fork"));
}

#[test]
fn a_transiently_missing_candidate_does_not_resolve_a_fork_ambiguity() {
    // One scan saw two plausible forks: attribution is unknowable, and
    // no later scan can change that. A rescan that transiently loses
    // one candidate (parse failure, deletion) must not make the other
    // look uniquely attributable.
    let a = meta("a", "/p");
    let mut roster = Roster::new(vec![a.clone()]);
    let (run_a, _) = roster.resume_selected().unwrap();
    let (d, e) = (post_launch(meta("d", "/p")), post_launch(meta("e", "/p")));
    roster.absorb_scan(vec![d, e.clone(), a.clone()]); // ambiguous

    roster.absorb_scan(vec![e.clone(), a.clone()]); // d transiently gone

    let by_id = |id: &str| roster.rows().iter().find(|r| r.transcript_id() == Some(id)).unwrap();
    assert_eq!(by_id("a").run_id(), Some(run_a), "the run stays home");
    assert_eq!(by_id("e").run_id(), None);
}

#[test]
fn a_reappearing_candidate_does_not_revive_an_ambiguous_fork_wait() {
    let a = meta("a", "/p");
    let mut roster = Roster::new(vec![a.clone()]);
    let (run_a, _) = roster.resume_selected().unwrap();
    let (d, e) = (post_launch(meta("d", "/p")), post_launch(meta("e", "/p")));
    roster.absorb_scan(vec![d.clone(), e.clone(), a.clone()]); // ambiguous: wait retired
    roster.absorb_scan(vec![e.clone(), a.clone()]); // d transiently gone

    roster.absorb_scan(vec![d, e, a]); // d is back

    let by_id = |id: &str| roster.rows().iter().find(|r| r.transcript_id() == Some(id)).unwrap();
    assert_eq!(by_id("a").run_id(), Some(run_a));
    assert_eq!(by_id("d").run_id(), None);
    assert_eq!(by_id("e").run_id(), None);
}

#[test]
fn an_ambiguous_fork_wait_blocks_launches_until_its_process_exits() {
    // An ambiguous fork wait can never adopt, but the resumed process
    // is alive and its fork is unattributed — a new transcript in the
    // cwd could still be its late write, so launches here stay blocked
    // until the contender exits.
    let a = meta("a", "/p");
    let mut roster = Roster::new(vec![a.clone()]);
    let (run_a, _) = roster.resume_selected().unwrap();
    let (d, e) = (post_launch(meta("d", "/p")), post_launch(meta("e", "/p")));
    roster.absorb_scan(vec![d.clone(), e.clone(), a.clone()]); // ambiguous: wait retired

    let (launch_run, _) = roster.launch(Agent::Claude, "/p");
    let fresh = post_launch(meta("fresh", "/p"));
    roster.absorb_scan(vec![fresh.clone(), d.clone(), e.clone(), a.clone()]);
    let fresh_row = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh_row.run_id(), None, "fresh could be the contender's late fork");

    roster.mark_exited(run_a);
    roster.absorb_scan(vec![fresh, d, e, a]);
    let fresh_row = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh_row.run_id(), Some(launch_run));
}

#[test]
fn an_origin_append_does_not_lift_an_ambiguous_fork_block() {
    // Fork-like candidates followed by an in-place append is
    // contradictory evidence about the agent; contradiction is still
    // uncertainty. Once ambiguity is recorded, only process exit ends
    // the block.
    let a = meta("a", "/p");
    let mut roster = Roster::new(vec![a.clone()]);
    roster.resume_selected().unwrap();
    let (d, e) = (post_launch(meta("d", "/p")), post_launch(meta("e", "/p")));
    roster.absorb_scan(vec![d.clone(), e.clone(), a.clone()]); // ambiguous: wait retired

    roster.absorb_scan(vec![post_launch(meta("a", "/p")), d.clone(), e.clone()]); // origin bumped

    roster.launch(Agent::Claude, "/p");
    roster.absorb_scan(vec![
        post_launch(meta("fresh", "/p")),
        post_launch(meta("a", "/p")),
        d,
        e,
    ]);
    let fresh = roster.rows().iter().find(|r| r.transcript_id() == Some("fresh")).unwrap();
    assert_eq!(fresh.run_id(), None, "the ambiguous contender still blocks");
}

#[test]
fn a_fork_scanned_after_exit_is_a_plain_idle_row() {
    // The process died before its fork hit a scan: nothing to adopt.
    let mut roster = Roster::new(vec![meta("orig", "/p")]);
    let (run_id, _) = roster.resume_selected().unwrap();
    roster.mark_exited(run_id);

    roster.absorb_scan(vec![post_launch(meta("fork", "/p")), meta("orig", "/p")]);

    assert!(!roster.has_running());
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
