use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    /// Whether resuming writes the continuation to a NEW transcript id.
    /// `claude --resume` forks; `codex resume` appends to the same
    /// rollout. Drives fork adoption in the roster — an agent that
    /// appends in place must never wait for (or claim) a new transcript.
    pub fn forks_on_resume(self) -> bool {
        match self {
            Agent::Claude => true,
            Agent::Codex => false,
        }
    }
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

/// Create missing store roots so they can be watched. Transcripts hold
/// prompts and pasted secrets: anything we create must be private
/// (0700), matching what the agent CLIs themselves do.
pub fn ensure_store_roots(roots: &ScanRoots) {
    for root in [&roots.claude, &roots.codex] {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let _ = fs::DirBuilder::new().recursive(true).mode(0o700).create(root);
        }
        #[cfg(not(unix))]
        let _ = fs::create_dir_all(root);
    }
}

/// Incremental scanner: holds parse results across scans so a rescan
/// only reparses transcripts that changed on disk.
#[derive(Default)]
pub struct Scanner {
    cache: HashMap<PathBuf, CacheEntry>,
}

/// Freshness is keyed on (mtime, len): mtime alone can miss a
/// same-second append, and appending is the only mutation the agent
/// CLIs perform. `meta: None` records a parsed-and-rejected file
/// (sidechain, subagent, malformed) so rescans skip it without
/// reparsing.
struct CacheEntry {
    mtime: SystemTime,
    len: u64,
    meta: Option<SessionMeta>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Same contract as `scan_all_sessions`: merged, newest-first.
    pub fn scan(&mut self, roots: &ScanRoots) -> Result<Vec<SessionMeta>> {
        let mut candidates = Vec::new();
        collect_claude_candidates(&roots.claude, &mut candidates);
        collect_codex_candidates(&roots.codex, &mut candidates);
        // Entries for files gone from disk must die now: a file later
        // recreated at the same path with the same (mtime, len) must
        // not be served from its predecessor's parse.
        let seen: HashSet<_> =
            candidates.iter().map(|(path, _)| path.clone()).collect();
        self.cache.retain(|path, _| seen.contains(path));

        let mut sessions = Vec::new();
        for (path, agent) in candidates {
            let Ok(md) = fs::metadata(&path) else {
                continue;
            };
            let Ok(mtime) = md.modified() else {
                continue;
            };
            let len = md.len();
            let fresh = self
                .cache
                .get(&path)
                .is_some_and(|e| e.mtime == mtime && e.len == len);
            if !fresh {
                let timestamp = mtime.into();
                let parsed = match agent {
                    Agent::Claude => parse_claude_transcript(&path, timestamp),
                    Agent::Codex => parse_codex_rollout(&path, timestamp),
                };
                // A transient read failure (Err) must not become a
                // cached rejection — permissions can be restored
                // without touching mtime or len. Serve any stale entry
                // this pass and retry on the next scan.
                if let Ok(meta) = parsed {
                    self.cache.insert(path.clone(), CacheEntry { mtime, len, meta });
                }
            }
            if let Some(meta) = self.cache.get(&path).and_then(|e| e.meta.as_ref()) {
                sessions.push(meta.clone());
            }
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.timestamp));
        Ok(sessions)
    }
}

/// Candidate transcripts in a ~/.claude/projects-style tree:
/// <root>/<encoded-cwd>/<session-id>.jsonl
fn collect_claude_candidates(
    root: &Path,
    candidates: &mut Vec<(PathBuf, Agent)>,
) {
    let Ok(projects) = fs::read_dir(root) else {
        return;
    };
    for project in projects.flatten() {
        let Ok(files) = fs::read_dir(project.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                candidates.push((path, Agent::Claude));
            }
        }
    }
}

/// Candidate rollouts in a ~/.codex/sessions-style tree:
/// <root>/YYYY/MM/DD/rollout-*.jsonl
fn collect_codex_candidates(dir: &Path, candidates: &mut Vec<(PathBuf, Agent)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // DirEntry::file_type does not follow symlinks: a link back to
        // an ancestor must not turn the walk into an infinite loop.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_codex_candidates(&path, candidates);
        } else if file_type.is_file()
            && path.extension().and_then(|e| e.to_str()) == Some("jsonl")
        {
            candidates.push((path, Agent::Codex));
        }
    }
}

/// One-shot scan of both agents' stores, sorted newest-first.
pub fn scan_all_sessions(roots: &ScanRoots) -> Result<Vec<SessionMeta>> {
    Scanner::new().scan(roots)
}

/// Scan a ~/.claude/projects-style tree: <root>/<encoded-cwd>/<session-id>.jsonl
pub fn scan_claude_sessions(root: &Path) -> Result<Vec<SessionMeta>> {
    let mut candidates = Vec::new();
    collect_claude_candidates(root, &mut candidates);
    Ok(candidates
        .iter()
        .filter_map(|(path, _)| parse_claude_transcript(path, file_mtime(path)?).ok()?)
        .collect())
}

/// Scan a ~/.codex/sessions-style tree: <root>/YYYY/MM/DD/rollout-*.jsonl.
/// Subagent rollouts (thread_source == "subagent") are not user sessions
/// and are skipped.
pub fn scan_codex_sessions(root: &Path) -> Result<Vec<SessionMeta>> {
    let mut candidates = Vec::new();
    collect_codex_candidates(root, &mut candidates);
    Ok(candidates
        .iter()
        .filter_map(|(path, _)| parse_codex_rollout(path, file_mtime(path)?).ok()?)
        .collect())
}

/// `Ok(None)` is a durable, content-derived rejection (subagent
/// rollout, malformed header) that stays valid for this (mtime, len);
/// `Err` is a transient I/O failure the caller must not cache.
fn parse_codex_rollout(
    path: &Path,
    timestamp: DateTime<Utc>,
) -> io::Result<Option<SessionMeta>> {
    let reader = BufReader::new(File::open(path)?);
    let mut lines = reader.lines();

    let Some(first) = lines.next() else {
        return Ok(None);
    };
    let Ok(header) = serde_json::from_str::<Value>(&first?) else {
        return Ok(None);
    };
    if header["type"] != "session_meta" {
        return Ok(None);
    }
    let payload = &header["payload"];
    if payload["thread_source"] == "subagent"
        || payload["source"].get("subagent").is_some()
    {
        return Ok(None);
    }
    let (Some(id), Some(cwd)) = (payload["id"].as_str(), payload["cwd"].as_str()) else {
        return Ok(None);
    };

    // A read error while hunting for the title degrades to an empty
    // title instead of failing the parse: codex appends events
    // continuously, so the next append re-keys the cache anyway.
    let title = lines
        .map_while(|l| l.ok())
        .filter_map(|l| serde_json::from_str::<Value>(&l).ok())
        .find(|v| v["payload"]["type"] == "user_message")
        .and_then(|v| v["payload"]["message"].as_str().map(first_line))
        .unwrap_or_default();

    Ok(Some(SessionMeta {
        id: id.to_string(),
        agent: Agent::Codex,
        cwd: cwd.to_string(),
        title,
        timestamp,
    }))
}

/// `Ok(None)` is a durable, content-derived rejection (sidechain,
/// malformed line, missing fields) that stays valid for this
/// (mtime, len); `Err` is a transient I/O failure the caller must not
/// cache.
fn parse_claude_transcript(
    path: &Path,
    timestamp: DateTime<Utc>,
) -> io::Result<Option<SessionMeta>> {
    let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
        return Ok(None);
    };
    let reader = BufReader::new(File::open(path)?);

    let mut cwd: Option<String> = None;
    // The slash command that started the session, if any: kept around
    // (rather than returned immediately) so a real human message right
    // after it can be combined into the title.
    let mut command: Option<String> = None;
    let mut fallback_title: Option<String> = None;
    for line in reader.lines() {
        let line = line?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            return Ok(None);
        };
        if value["type"] != "user" {
            continue;
        }
        if value["isSidechain"] == true {
            return Ok(None);
        }
        if cwd.is_none() {
            let Some(c) = value["cwd"].as_str() else {
                return Ok(None);
            };
            cwd = Some(c.to_string());
        }
        // Skills inject generated text (e.g. "Base directory for this
        // skill: ...") as isMeta user messages; they are not something
        // the human said and must never become or extend the title.
        if value["isMeta"] == true {
            continue;
        }
        let Some(title) = title_from_content(&value["message"]["content"]) else {
            continue;
        };
        // Slash commands and shell output are recorded as user messages
        // wrapped in <command-...>/<local-command-...> tags. Only the
        // known wrapper tags are synthetic: humans legitimately start
        // prompts with XML/HTML fragments.
        if title.starts_with("<command-") || title.starts_with("<local-command-") {
            if let Some(name) = command_name(&value["message"]["content"]) {
                command.get_or_insert(name);
            } else {
                fallback_title.get_or_insert(title);
            }
            continue;
        }
        // The first real human message: the title, prefixed with the
        // command that started the session (if any) for context.
        let title = match command {
            Some(c) => format!("/{c} · {title}"),
            None => title,
        };
        return Ok(cwd.map(|cwd| SessionMeta {
            id: id.to_string(),
            agent: Agent::Claude,
            cwd,
            title,
            timestamp,
        }));
    }
    // No real human message ever appeared: fall back to the bare
    // command, or the unparseable wrapper text, in that order.
    let title = command
        .map(|c| format!("/{c}"))
        .or(fallback_title);
    Ok(cwd.zip(title).map(|(cwd, title)| SessionMeta {
        id: id.to_string(),
        agent: Agent::Claude,
        cwd,
        title,
        timestamp,
    }))
}

/// The slash command name from a `<command-name>...</command-name>`
/// wrapper, if this message is one.
fn command_name(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.as_str(),
        Value::Array(blocks) => blocks
            .iter()
            .find(|b| b["type"] == "text")
            .and_then(|b| b["text"].as_str())?,
        _ => return None,
    };
    let (_, rest) = text.split_once("<command-name>")?;
    let (name, _) = rest.split_once("</command-name>")?;
    let name = name.trim().trim_start_matches('/');
    (!name.is_empty() && !name.contains('<')).then(|| first_line(name))
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
    // Transcript content is untrusted: keep the display-only title free
    // of ANSI/OSC control bytes regardless of how the UI renders it.
    text.lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .collect()
}

fn file_mtime(path: &Path) -> Option<DateTime<Utc>> {
    Some(fs::metadata(path).ok()?.modified().ok()?.into())
}
