use session_tui::sessions::{
    scan_all_sessions, scan_claude_sessions, scan_codex_sessions, Agent, ScanRoots, Scanner,
};

mod fixtures;
use fixtures::{rewrite_preserving_stat, write_claude_session, write_codex_session};

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
fn meta_messages_never_title_a_plain_session() {
    // No slash command involved: an isMeta line (e.g. injected by a
    // hook or automation) must be skipped just like after a command,
    // and the first real human message titles the session.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("22223333-4444-5555-6666-777788889999.jsonl"),
        concat!(
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"injected setup text"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"the real request"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "the real request");
}

#[test]
fn command_only_session_keeps_the_bare_slash_title() {
    // A fire-and-forget slash command run with no follow-up human
    // message (only tool-result blocks, which have no text) keeps the
    // command as its title rather than combining with nothing.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("33334444-5555-6666-7777-888899990000.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<command-message>docs</command-message>\n<command-name>docs</command-name>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Base directory for this skill: ..."},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"ok"}]},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:04.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "/docs");
}

#[test]
fn unparseable_wrapper_never_prefixes_the_human_title() {
    // <local-command-caveat> etc. have no <command-name> tag to parse,
    // so they can never become a "/foo · ..." prefix — only a real
    // parsed command may. The human message titles alone.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("44445555-6666-7777-8888-999900001111.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>some output</local-command-stdout>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"the actual request"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "the actual request");
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
    // inject generated text like "Base directory for this skill: ..."
    // flagged isMeta. With no real human message after the command,
    // the title is just the command.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("eeee1111-2222-3333-4444-555566667777.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<command-message>code-review</command-message>\n<command-name>code-review</command-name>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Base directory for this skill: /Users/brian/.claude/skills/code-review\n\nReview the diff..."},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "/code-review");
}

#[test]
fn slash_command_title_combines_command_and_first_human_message() {
    // When the human follows a slash command with a real message, the
    // title should carry both: the command for context, and what they
    // actually asked for. isMeta skill-injection text in between must
    // not be mistaken for that human message.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("11112222-3333-4444-5555-666677778888.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<command-message>docs</command-message>\n<command-name>docs</command-name>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Base directory for this skill: /Users/brian/.claude/skills/docs\n\nUpdate documentation..."},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"ship these as a new pr"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:04.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "/docs · ship these as a new pr");
}

#[test]
fn interrupted_turn_does_not_steal_the_slash_command_title() {
    // An Escape-interrupted tool call records a plain (non-isMeta) user
    // text block reading "[Request interrupted by user]"; it must be
    // skipped so a later genuine human message still titles the session.
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("55556666-7777-8888-9999-000011112222.jsonl"),
        concat!(
            r#"{"type":"user","message":{"role":"user","content":"<command-message>docs</command-message>\n<command-name>docs</command-name>"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:02.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:03.390Z"}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":"actually do this instead"},"cwd":"/Users/brian/dev/myproj","timestamp":"2026-07-01T15:34:04.390Z"}"#,
            "\n",
        ),
    )
    .unwrap();

    let sessions = scan_claude_sessions(root.path()).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "/docs · actually do this instead");
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
fn scanner_matches_one_shot_scan() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "claude session",
        "2026-07-01T15:34:02.390Z",
    );
    write_codex_session(
        codex_root.path(),
        "2026/06/25",
        "019f01b4-47cd-76c0-9d83-9aa151a3a918",
        "/Users/brian/dev/myproj",
        "user",
        "codex session",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };

    let scanned = Scanner::new().scan(&roots).unwrap();

    let one_shot = scan_all_sessions(&roots).unwrap();
    assert_eq!(scanned.len(), one_shot.len());
    for (s, o) in scanned.iter().zip(&one_shot) {
        assert_eq!(s.id, o.id);
        assert_eq!(s.agent, o.agent);
        assert_eq!(s.cwd, o.cwd);
        assert_eq!(s.title, o.title);
        assert_eq!(s.timestamp, o.timestamp);
    }
}

#[test]
fn rescan_does_not_reparse_unchanged_transcripts() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "original title",
        "2026-07-01T15:34:02.390Z",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let mut scanner = Scanner::new();
    scanner.scan(&roots).unwrap();

    // Same length, restored mtime: indistinguishable from unchanged
    // without reopening the file.
    let path = claude_root
        .path()
        .join("-Users-brian-dev-myproj/aaaa1111-2222-3333-4444-555566667777.jsonl");
    let swapped = std::fs::read_to_string(&path)
        .unwrap()
        .replace("original title", "swapped_title!");
    rewrite_preserving_stat(&path, &swapped);

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(
        sessions[0].title, "original title",
        "an unchanged (mtime, len) transcript must be served from cache"
    );
}

#[test]
fn rescan_picks_up_modified_transcript() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "original title",
        "2026-07-01T15:34:02.390Z",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let mut scanner = Scanner::new();
    let before = scanner.scan(&roots).unwrap();

    // Same length so only the mtime distinguishes it from unchanged.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let path = claude_root
        .path()
        .join("-Users-brian-dev-myproj/aaaa1111-2222-3333-4444-555566667777.jsonl");
    let swapped = std::fs::read_to_string(&path)
        .unwrap()
        .replace("original title", "modified title");
    std::fs::write(&path, swapped).unwrap();

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(sessions[0].title, "modified title");
    assert!(
        sessions[0].timestamp > before[0].timestamp,
        "timestamp must track the new mtime"
    );
}

#[test]
fn rescan_picks_up_same_mtime_append() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    // A rollout with no user message yet: the agent CLIs append events
    // over time, and coarse filesystems can leave the mtime unchanged
    // within the same second.
    let dir = codex_root.path().join("2026/06/25");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-2026-06-25T22-13-39-019f01b4.jsonl");
    std::fs::write(
        &path,
        concat!(
            r#"{"type":"session_meta","payload":{"id":"019f01b4","cwd":"/Users/brian/dev/myproj","thread_source":"user"}}"#,
            "\n",
        ),
    )
    .unwrap();
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let mut scanner = Scanner::new();
    let before = scanner.scan(&roots).unwrap();
    assert_eq!(before[0].title, "");

    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str(concat!(
        r#"{"type":"event_msg","payload":{"type":"user_message","message":"appended prompt"}}"#,
        "\n",
    ));
    std::fs::write(&path, content).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(
        sessions[0].title, "appended prompt",
        "a same-mtime append must still invalidate the cache (len changed)"
    );
}

#[test]
fn rescan_discovers_new_transcript() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "first session",
        "2026-07-01T15:34:02.390Z",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let mut scanner = Scanner::new();
    assert_eq!(scanner.scan(&roots).unwrap().len(), 1);

    std::thread::sleep(std::time::Duration::from_millis(20));
    write_codex_session(
        codex_root.path(),
        "2026/06/25",
        "019f01b4-47cd-76c0-9d83-9aa151a3a918",
        "/Users/brian/dev/myproj",
        "user",
        "second session",
    );

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0].title, "second session", "newest first");
    assert_eq!(sessions[1].title, "first session");
}

#[test]
fn rescan_drops_deleted_transcript() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "kept session",
        "2026-07-01T15:34:02.390Z",
    );
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-otherproj",
        "bbbb1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/otherproj",
        "doomed session",
        "2026-07-01T15:34:02.390Z",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let mut scanner = Scanner::new();
    assert_eq!(scanner.scan(&roots).unwrap().len(), 2);

    // The common case is a whole project dir going away (rm -rf).
    std::fs::remove_dir_all(claude_root.path().join("-Users-brian-dev-otherproj")).unwrap();

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title, "kept session");
}

#[test]
fn deleted_transcripts_cache_entry_dies_with_it() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "original title",
        "2026-07-01T15:34:02.390Z",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let path = claude_root
        .path()
        .join("-Users-brian-dev-myproj/aaaa1111-2222-3333-4444-555566667777.jsonl");
    let content = std::fs::read_to_string(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let mut scanner = Scanner::new();
    scanner.scan(&roots).unwrap();

    // Delete, rescan, then recreate at the same path with the same
    // (mtime, len) but different content. A cache entry that survived
    // the deletion would serve the dead file's title.
    std::fs::remove_file(&path).unwrap();
    assert!(scanner.scan(&roots).unwrap().is_empty());
    std::fs::write(&path, content.replace("original title", "reborn__title!")).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(
        sessions[0].title, "reborn__title!",
        "a deleted file's cache entry must not outlive it"
    );
}

#[test]
fn rescan_does_not_reparse_rejected_transcripts() {
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    let dir = claude_root.path().join("-Users-brian-dev-myproj");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("aaaa1111-2222-3333-4444-555566667777.jsonl");
    // The sidechain and non-sidechain lines are padded to the same
    // byte length via the message text.
    let sidechain = concat!(
        r#"{"type":"user","isSidechain":true,"message":{"role":"user","content":"a sidechain"},"cwd":"/Users/brian/dev/myproj"}"#,
        "\n",
    );
    let valid = concat!(
        r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"a real one"},"cwd":"/Users/brian/dev/myproj"}"#,
        "\n",
    );
    assert_eq!(sidechain.len(), valid.len());
    std::fs::write(&path, sidechain).unwrap();
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    let mut scanner = Scanner::new();
    assert!(scanner.scan(&roots).unwrap().is_empty());

    // Same (mtime, len) but now valid content: only a scanner that
    // cached the rejection can still exclude it without reparsing.
    rewrite_preserving_stat(&path, valid);

    assert!(
        scanner.scan(&roots).unwrap().is_empty(),
        "a parsed-and-rejected file must be cached as rejected, not reparsed"
    );
}

#[test]
fn transiently_unreadable_transcript_is_retried_on_rescan() {
    use std::os::unix::fs::PermissionsExt;
    let claude_root = tempfile::tempdir().unwrap();
    let codex_root = tempfile::tempdir().unwrap();
    write_claude_session(
        claude_root.path(),
        "-Users-brian-dev-myproj",
        "aaaa1111-2222-3333-4444-555566667777",
        "/Users/brian/dev/myproj",
        "hidden then found",
        "2026-07-01T15:34:02.390Z",
    );
    let roots = ScanRoots {
        claude: claude_root.path().to_path_buf(),
        codex: codex_root.path().to_path_buf(),
    };
    // chmod 000: the file still stats (mtime, len unchanged) but can't
    // be opened. Caching that failed parse as a rejection would hide
    // the session forever, because restoring permissions touches
    // neither mtime nor length.
    let path = claude_root
        .path()
        .join("-Users-brian-dev-myproj/aaaa1111-2222-3333-4444-555566667777.jsonl");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    let mut scanner = Scanner::new();
    assert!(scanner.scan(&roots).unwrap().is_empty());

    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let sessions = scanner.scan(&roots).unwrap();

    assert_eq!(sessions.len(), 1, "an I/O failure must be retried, not cached");
    assert_eq!(sessions[0].title, "hidden then found");
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
