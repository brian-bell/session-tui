use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub agent: Agent,
    pub cwd: String,
    pub title: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ScanRoots {
    pub claude: std::path::PathBuf,
    pub codex: std::path::PathBuf,
}

impl Default for ScanRoots {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        Self {
            claude: home.join(".claude/projects"),
            codex: home.join(".codex/sessions"),
        }
    }
}

/// Scan both agents' stores and return one list sorted newest-first.
pub fn scan_all_sessions(roots: &ScanRoots) -> Result<Vec<SessionMeta>> {
    let mut sessions = scan_claude_sessions(&roots.claude)?;
    sessions.extend(scan_codex_sessions(&roots.codex)?);
    sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
    Ok(sessions)
}

/// Scan a ~/.claude/projects-style tree: <root>/<encoded-cwd>/<session-id>.jsonl
pub fn scan_claude_sessions(root: &Path) -> Result<Vec<SessionMeta>> {
    let mut sessions = Vec::new();
    let Ok(projects) = fs::read_dir(root) else {
        return Ok(sessions);
    };
    for project in projects.flatten() {
        let Ok(files) = fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(meta) = parse_claude_transcript(&path) {
                sessions.push(meta);
            }
        }
    }
    Ok(sessions)
}

/// Scan a ~/.codex/sessions-style tree: <root>/YYYY/MM/DD/rollout-*.jsonl.
/// Subagent rollouts (thread_source == "subagent") are not user sessions
/// and are skipped.
pub fn scan_codex_sessions(root: &Path) -> Result<Vec<SessionMeta>> {
    let mut sessions = Vec::new();
    scan_codex_dir(root, &mut sessions);
    Ok(sessions)
}

fn scan_codex_dir(dir: &Path, sessions: &mut Vec<SessionMeta>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_codex_dir(&path, sessions);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && let Some(meta) = parse_codex_rollout(&path) {
                sessions.push(meta);
            }
    }
}

fn parse_codex_rollout(path: &Path) -> Option<SessionMeta> {
    let timestamp = file_mtime(path)?;
    let reader = BufReader::new(File::open(path).ok()?);
    let mut lines = reader.lines();

    let header: Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
    if header["type"] != "session_meta" {
        return None;
    }
    let payload = &header["payload"];
    if payload["thread_source"] == "subagent"
        || payload["source"].get("subagent").is_some()
    {
        return None;
    }
    let id = payload["id"].as_str()?.to_string();
    let cwd = payload["cwd"].as_str()?.to_string();

    let title = lines
        .map_while(|l| l.ok())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .find(|v| v["payload"]["type"] == "user_message")
        .and_then(|v| v["payload"]["message"].as_str().map(first_line))
        .unwrap_or_default();

    Some(SessionMeta {
        id,
        agent: Agent::Codex,
        cwd,
        title,
        timestamp,
    })
}

fn parse_claude_transcript(path: &Path) -> Option<SessionMeta> {
    let id = path.file_stem()?.to_str()?.to_string();
    let timestamp = file_mtime(path)?;
    let reader = BufReader::new(File::open(path).ok()?);

    let mut cwd: Option<String> = None;
    let mut fallback_title: Option<String> = None;
    for line in reader.lines() {
        let line = line.ok()?;
        let value: Value = serde_json::from_str(&line).ok()?;
        if value["type"] != "user" {
            continue;
        }
        if value["isSidechain"] == true {
            return None;
        }
        if cwd.is_none() {
            cwd = Some(value["cwd"].as_str()?.to_string());
        }
        let Some(title) = title_from_content(&value["message"]["content"]) else {
            continue;
        };
        // Slash commands and shell output are recorded as user messages
        // wrapped in <command-...>/<local-command-...> tags; a session's
        // title should be the first thing the human actually typed.
        if title.starts_with('<') {
            fallback_title.get_or_insert(title);
            continue;
        }
        return Some(SessionMeta {
            id,
            agent: Agent::Claude,
            cwd: cwd?,
            title,
            timestamp,
        });
    }
    Some(SessionMeta {
        id,
        agent: Agent::Claude,
        cwd: cwd?,
        title: fallback_title?,
        timestamp,
    })
}

/// Claude message content is either a plain string or an array of blocks.
fn title_from_content(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.as_str(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|b| b["type"] == "text")
            .and_then(|b| b["text"].as_str())?,
        _ => return None,
    };
    Some(first_line(text))
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    Some(fs::metadata(path).ok()?.modified().ok()?.into())
}
