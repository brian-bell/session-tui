use std::fs;
use std::path::Path;

/// Write a minimal Codex rollout mimicking ~/.codex/sessions layout:
/// <root>/YYYY/MM/DD/rollout-<ts>-<id>.jsonl with a session_meta header
/// and a user_message event.
pub fn write_codex_session(
    root: &Path,
    date_path: &str,
    session_id: &str,
    cwd: &str,
    thread_source: &str,
    first_user_message: &str,
) {
    let dir = root.join(date_path);
    fs::create_dir_all(&dir).unwrap();
    let meta = serde_json::json!({
        "timestamp": "2026-06-26T02:13:40.068Z",
        "type": "session_meta",
        "payload": {
            "id": session_id,
            "timestamp": "2026-06-26T02:13:39.959Z",
            "cwd": cwd,
            "originator": "codex_cli",
            "cli_version": "0.142.2",
            "thread_source": thread_source,
        }
    });
    let user = serde_json::json!({
        "timestamp": "2026-06-26T02:13:40.902Z",
        "type": "event_msg",
        "payload": {"type": "user_message", "message": first_user_message}
    });
    fs::write(
        dir.join(format!("rollout-2026-06-25T22-13-39-{session_id}.jsonl")),
        format!("{meta}\n{user}\n"),
    )
    .unwrap();
}

/// Rewrite a file in place while preserving its observable stat: the
/// new content must have the same byte length, and the original mtime
/// is restored after the write. A scanner keyed on (mtime, len) must
/// treat the file as unchanged — so if the old parse result is still
/// returned, the file was provably not reparsed.
pub fn rewrite_preserving_stat(path: &Path, content: &str) {
    let meta = fs::metadata(path).unwrap();
    assert_eq!(
        meta.len(),
        content.len() as u64,
        "helper misuse: replacement content must keep the file length"
    );
    let mtime = meta.modified().unwrap();
    fs::write(path, content).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

/// Write a minimal Claude Code transcript mimicking ~/.claude/projects layout:
/// <root>/<encoded-cwd>/<session-id>.jsonl with a first user message line.
pub fn write_claude_session(
    root: &Path,
    project_dir: &str,
    session_id: &str,
    cwd: &str,
    first_user_message: &str,
    timestamp: &str,
) {
    let dir = root.join(project_dir);
    fs::create_dir_all(&dir).unwrap();
    let line = serde_json::json!({
        "parentUuid": null,
        "isSidechain": false,
        "type": "user",
        "message": {"role": "user", "content": first_user_message},
        "uuid": "fefde5ee-8225-4fb2-a95b-4a67a19f69ae",
        "timestamp": timestamp,
        "cwd": cwd,
        "sessionId": session_id,
    });
    fs::write(
        dir.join(format!("{session_id}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
}
