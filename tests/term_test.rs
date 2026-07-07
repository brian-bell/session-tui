use std::time::{Duration, Instant};

use session_tui::sessions::{Agent, SessionMeta};
use session_tui::term::{CommandSpec, PtySession};

/// Poll until `pred` is true or the timeout elapses.
fn wait_for(mut pred: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn sh(script: &str) -> CommandSpec {
    CommandSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: "/tmp".into(),
    }
}

fn meta(agent: Agent) -> SessionMeta {
    SessionMeta {
        id: "abc-123".into(),
        agent,
        cwd: "/Users/brian/dev/myproj".into(),
        title: "t".into(),
        timestamp: chrono::Utc::now(),
    }
}

#[test]
fn resume_command_targets_the_right_agent_cli_in_session_cwd() {
    let claude = CommandSpec::resume(&meta(Agent::Claude));
    assert_eq!(claude.program, "claude");
    assert_eq!(claude.args, vec!["--resume", "abc-123"]);
    assert_eq!(claude.cwd, "/Users/brian/dev/myproj");

    let codex = CommandSpec::resume(&meta(Agent::Codex));
    assert_eq!(codex.program, "codex");
    assert_eq!(codex.args, vec!["resume", "abc-123"]);
    assert_eq!(codex.cwd, "/Users/brian/dev/myproj");
}

#[test]
fn launch_command_starts_a_fresh_agent_in_a_chosen_cwd() {
    let claude = CommandSpec::launch(Agent::Claude, "/tmp/proj");
    assert_eq!(claude.program, "claude");
    assert!(claude.args.is_empty());
    assert_eq!(claude.cwd, "/tmp/proj");

    let codex = CommandSpec::launch(Agent::Codex, "/tmp/proj");
    assert_eq!(codex.program, "codex");
    assert!(codex.args.is_empty());
}

#[test]
fn spawned_command_output_appears_on_the_emulated_screen() {
    let session = PtySession::spawn(&sh("printf 'hello-pty\\n'"), 24, 80).unwrap();

    assert!(
        wait_for(|| session.screen_text().contains("hello-pty"), Duration::from_secs(5)),
        "screen never showed output: {:?}",
        session.screen_text()
    );
}

#[test]
fn input_written_to_the_session_reaches_the_child() {
    let mut session = PtySession::spawn(&sh("read line; echo \"got:$line\""), 24, 80).unwrap();
    session.write_input(b"ping\r").unwrap();

    assert!(
        wait_for(|| session.screen_text().contains("got:ping"), Duration::from_secs(5)),
        "child never echoed input: {:?}",
        session.screen_text()
    );
}

#[test]
fn session_reports_busy_after_recent_output_and_idle_when_quiet() {
    use session_tui::term::SessionStatus;

    assert_eq!(SessionStatus::from_idle(Duration::from_millis(300)), SessionStatus::Busy);
    assert_eq!(SessionStatus::from_idle(Duration::from_secs(3)), SessionStatus::Idle);

    // A live session that just produced output reports Busy.
    let mut session = PtySession::spawn(&sh("printf 'x'; sleep 100"), 24, 80).unwrap();
    assert!(wait_for(|| session.screen_text().contains('x'), Duration::from_secs(5)));
    assert_eq!(session.status(), SessionStatus::Busy);
    session.kill().unwrap();
}

#[test]
fn modes_reflect_the_dec_private_modes_the_child_sets() {
    use session_tui::term::TermModes;

    // A child that never touches DEC modes reports the defaults.
    let plain = PtySession::spawn(&sh("printf 'up'; sleep 100"), 24, 80).unwrap();
    assert!(wait_for(|| plain.screen_text().contains("up"), Duration::from_secs(5)));
    assert_eq!(plain.modes(), TermModes::default());

    // DECCKM (?1h) and bracketed paste (?2004h) show up once the
    // emulator has processed the escape sequences.
    let session =
        PtySession::spawn(&sh("printf '\\033[?1h\\033[?2004h'; printf 'ready'; sleep 100"), 24, 80)
            .unwrap();
    assert!(wait_for(|| session.screen_text().contains("ready"), Duration::from_secs(5)));
    assert_eq!(
        session.modes(),
        TermModes { app_cursor: true, bracketed_paste: true }
    );
}

#[test]
fn kill_terminates_and_reaps_the_child() {
    let mut session = PtySession::spawn(&sh("sleep 100"), 24, 80).unwrap();
    assert!(session.is_running());
    let pid = session.child_pid().expect("child should have a pid");

    session.kill().unwrap();

    assert!(
        wait_for(|| !session.is_running(), Duration::from_secs(5)),
        "child still running after kill"
    );
    // The child must be reaped, not left as a zombie until app exit.
    let reaped = wait_for(
        || {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            // reaped: ps finds nothing; or at least not a zombie
            out.stdout.is_empty() || !String::from_utf8_lossy(&out.stdout).contains('Z')
        },
        Duration::from_secs(5),
    );
    assert!(reaped, "killed child left as a zombie");
}

#[test]
fn dropping_a_session_kills_its_child() {
    // If the TUI exits on an error path, PTY handles are dropped
    // without an explicit Quit; agents must not keep acting detached.
    let pid = {
        let session = PtySession::spawn(&sh("sleep 100"), 24, 80).unwrap();
        session.child_pid().expect("child should have a pid")
    }; // dropped here

    let gone = wait_for(
        || {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            // killed and reaped: ps finds no such process at all
            out.stdout.is_empty()
        },
        Duration::from_secs(5),
    );
    assert!(gone, "child survived PtySession drop");
}

#[test]
fn spawn_honors_the_requested_working_directory() {
    let session = PtySession::spawn(&sh("pwd"), 24, 80).unwrap();
    assert!(
        wait_for(|| {
            let t = session.screen_text();
            t.contains("/tmp") || t.contains("/private/tmp")
        }, Duration::from_secs(5)),
        "pwd output: {:?}",
        session.screen_text()
    );
}
