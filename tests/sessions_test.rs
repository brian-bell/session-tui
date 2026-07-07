use session_tui::sessions::{
    scan_all_sessions, scan_claude_sessions, scan_codex_sessions, Agent, ScanRoots,
};

mod fixtures;
use fixtures::{write_claude_session, write_codex_session};

#[test]
fn scans_claude_sessions_with_metadata() {
    let root = tempfile::tempdir().unwrap();
    write_claude_session(
        root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "Fix the login bug\nIt crashes on empty password.",
        "2026-07-01T15:34:02.390Z",
    );

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.id, "aaaa1111-2222-3333-4444-555566667777");
    assert_eq!(s.agent, Agent::Claude);
    assert_eq!(s.cwd, "/Users/brian/dev/myproj");
    assert_eq!(s.title, "Fix the login bug");
    // timestamp reflects last activity (file mtime), which for a
    // freshly written fixture is "now"
    let age = chrono::Utc::now() - s.timestamp;
    assert!(age.num_seconds() < 60, "timestamp should be recent: {age}");
}

#[test]
fn scans_codex_sessions_skipping_subagent_rollouts() {
    let root = tempfile::tempdir().unwrap();
    write_codex_session(
        root.path(),
        "2026/06/25",
        "019f01b4-47cd-76c0-9d83-9aa151a3a918",
        "/Users/brian/dev/myproj",
        "user",
        "Refactor the parser module",
    );
    write_codex_session(
        root.path(),
        "2026/06/25",
        "019f01b4-0000-0000-0000-000000000000",
        "/Users/brian/dev/myproj",
        "subagent",
        "You are a critical reviewer...",
    );

    let sessions = scan_codex_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    let s = &sessions[0];
    assert_eq!(s.id, "019f01b4-47cd-76c0-9d83-9aa151a3a918");
    assert_eq!(s.agent, Agent::Codex);
    assert_eq!(s.cwd, "/Users/brian/dev/myproj");
    assert_eq!(s.title, "Refactor the parser module");
}

#[test]
fn tolerates_leading_non_user_lines_and_skips_sidechain_transcripts() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    // Real transcripts often open with queue-operation lines before any
    // user message.
    std::fs::write(
        dir.join("bbbb1111-2222-3333-4444-555566667777.jsonl"),
        concat!(
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-07-01T15:34:02.378Z"}"#,
            "\n",
            r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"real session"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();
    // A subagent transcript is not a resumable user session.
    std::fs::write(
        dir.join("agent-ac8794b425715d8f8.jsonl"),
        concat!(
            r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"sidechain task"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "real session");
}

#[test]
fn title_skips_synthetic_command_messages() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("cccc1111-2222-3333-4444-555566667777.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<local-command-caveat>Caveat: generated output</local-command-caveat>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"the actual human request"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:04.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "the actual human request");
}

#[test]
fn human_prompts_starting_with_angle_brackets_still_title_the_session() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("ffff1111-2222-3333-4444-555566667777.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<div> in the header is broken"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"a later message"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions[0].title, "<div> in the header is broken");
}

#[test]
fn slash_command_sessions_are_titled_by_their_command() {
    // A session started by a slash command records the human's action
    // as a <command-message>/<command-name> wrapper, and skills then
    // inject generated text like "Base directory for this skill: ...".
    // The title must be the command, not the injected text.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("eeee1111-2222-3333-4444-555566667777.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<command-message>code-review</command-message>\n<command-name>code-review</command-name>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"Base directory for this skill: /Users/brian/.claude/skills/code-review\n\nReview the diff..."},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "/code-review");
}

#[test]
fn store_roots_are_created_private() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let roots = session_tui::sessions::ScanRoots {
        claude: tmp.path().join("claude/projects"),
        codex: tmp.path().join("codex/sessions"),
    };

    session_tui::sessions::ensure_store_roots(&roots);

    for dir in [&roots.claude, &roots.codex] {
        let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "{dir:?} must not be world-accessible");
        let parent_mode = std::fs::metadata(dir.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(parent_mode, 0o700, "created parents must be private too");
    }
}

#[test]
fn codex_scan_survives_symlink_cycles() {
    let root = tempfile::tempdir().unwrap();
    write_codex_session(
        root.path(),
        "2026/06/25",
        "019f01b4-47cd-76c0-9d83-9aa151a3a918",
        "/Users/brian/dev/myproj",
        "user",
        "real session",
    );
    // A symlink pointing back at an ancestor must not hang the walk.
    std::os::unix::fs::symlink(root.path(), root.path().join("2026/loop")).unwrap();

    let sessions = scan_codex_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "real session");
}

#[test]
fn titles_never_carry_control_characters() {
    // Transcript content is untrusted; a prompt full of ANSI/OSC bytes
    // must not survive into the (display-only) title.
    let root = tempfile::tempdir().unwrap();
    write_claude_session(
        root.path(),
        "-Users-brian-dev-myproj",
        "dddd1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "evil \x1b]0;pwned\x07 title \x1b[31mred\tend",
        "2026-07-01T15:34:02.390Z",
    );

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert!(
        !sessions[0].title.chars().any(|c| c.is_control()),
        "title still has control chars: {:?}",
        sessions[0].title
    );
}

#[test]
fn merged_list_is_sorted_newest_first_across_agents() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_codex_session(
        codex_root.path(),
        "2026/06/25",
        "019f01b4-47cd-76c0-9d83-9aa151a3a918",
        "/Users/brian/dev/myproj",
        "user",
        "older codex session",
    );
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "newer claude session",
        "2026-07-01T15:34:02.390Z",
    );

    let sessions = scan_all_sessions(&ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    })
    .unwrap();

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].title, "newer claude session");
    assert_eq!(sessions[1].title, "older codex session");
}
