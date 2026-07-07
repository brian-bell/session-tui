use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::sessions::{Agent, SessionMeta};
use crate::term::CommandSpec;

/// Identifies one live PTY for the lifetime of the app.
pub type RunId = u64;

/// Why a row exists: a transcript on disk, or a process launched here
/// that has not written its transcript yet.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    /// A fresh launch. Adoptable only by a transcript outside
    /// `snapshot` — the transcript ids that already existed at launch
    /// time, which necessarily belong to other sessions.
    Provisional { run_id: RunId, snapshot: HashSet<String> },
    Transcript(String),
}

/// One session in the list: scanned metadata (or launch placeholders)
/// plus the live run, if any.
#[derive(Debug, Clone)]
pub struct Row {
    identity: Identity,
    /// Live PTY of a resumed/adopted transcript row; a provisional
    /// row's run lives in its identity.
    run: Option<RunId>,
    agent: Agent,
    cwd: String,
    title: String,
    timestamp: DateTime<Utc>,
}

impl Row {
    fn from_meta(meta: SessionMeta, run: Option<RunId>) -> Self {
        Self {
            identity: Identity::Transcript(meta.id),
            run,
            agent: meta.agent,
            cwd: meta.cwd,
            title: meta.title,
            timestamp: meta.timestamp,
        }
    }

    pub fn run_id(&self) -> Option<RunId> {
        match &self.identity {
            Identity::Provisional { run_id, .. } => Some(*run_id),
            Identity::Transcript(_) => self.run,
        }
    }

    pub fn is_provisional(&self) -> bool {
        matches!(self.identity, Identity::Provisional { .. })
    }

    pub fn transcript_id(&self) -> Option<&str> {
        match &self.identity {
            Identity::Transcript(id) => Some(id),
            Identity::Provisional { .. } => None,
        }
    }

    pub fn agent(&self) -> Agent {
        self.agent
    }

    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.timestamp
    }
}

/// Selection is tracked by identity, not index: rescans reorder rows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Run(RunId),
    Id(String),
}

fn key_of(row: &Row) -> Key {
    match &row.identity {
        Identity::Provisional { run_id, .. } => Key::Run(*run_id),
        Identity::Transcript(id) => Key::Id(id.clone()),
    }
}

/// The session list and every lifecycle rule around it: launching
/// provisional rows, resuming transcripts, adopting a provisional row's
/// scanned transcript, and dropping rows whose process ended. Never
/// touches the filesystem or a PTY.
pub struct Roster {
    rows: Vec<Row>,
    selected: usize,
    next_run_id: RunId,
}

impl Roster {
    pub fn new(scanned: Vec<SessionMeta>) -> Self {
        Self {
            rows: scanned.into_iter().map(|m| Row::from_meta(m, None)).collect(),
            selected: 0,
            next_run_id: 1,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn is_running(&self, run_id: RunId) -> bool {
        self.rows.iter().any(|r| r.run_id() == Some(run_id))
    }

    pub fn has_running(&self) -> bool {
        self.rows.iter().any(|r| r.run_id().is_some())
    }

    /// Recently used project dirs, newest first, deduped. Provisional
    /// rows are skipped: their cwd came from the picker to begin with.
    pub fn known_dirs(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.rows
            .iter()
            .filter(|r| !r.is_provisional())
            .map(|r| r.cwd.clone())
            .filter(|d| seen.insert(d.clone()))
            .collect()
    }

    /// Spawn a fresh agent: a provisional row goes on top, selected.
    /// The row snapshots the transcript ids known right now; only a
    /// transcript first seen after this launch may adopt it.
    pub fn launch(&mut self, agent: Agent, cwd: &str) -> (RunId, CommandSpec) {
        let run_id = self.alloc_run_id();
        let snapshot = self
            .rows
            .iter()
            .filter_map(|r| r.transcript_id().map(str::to_string))
            .collect();
        self.rows.insert(
            0,
            Row {
                identity: Identity::Provisional { run_id, snapshot },
                run: None,
                agent,
                cwd: cwd.to_string(),
                title: "(new session)".to_string(),
                timestamp: Utc::now(),
            },
        );
        self.selected = 0;
        (run_id, CommandSpec::launch(agent, cwd))
    }

    /// Give the selected transcript row a live run. Returns None when
    /// there is nothing to spawn (no row, already running, or
    /// provisional). The caller verifies the cwd still exists first —
    /// this module never touches the filesystem.
    pub fn resume_selected(&mut self) -> Option<(RunId, CommandSpec)> {
        let row = self.rows.get(self.selected)?;
        if row.run_id().is_some() {
            return None;
        }
        let id = row.transcript_id()?.to_string();
        let spec = CommandSpec::resume_id(row.agent, &id, &row.cwd);
        let run_id = self.alloc_run_id();
        self.rows[self.selected].run = Some(run_id);
        Some((run_id, spec))
    }

    /// The process behind `run_id` ended (exit or kill). A provisional
    /// row has no transcript to resume, so it disappears with its
    /// process; a transcript row just loses its run marker. Selection
    /// follows the selected row's identity, not its old index.
    pub fn mark_exited(&mut self, run_id: RunId) {
        let key = self.selected_key();
        for row in &mut self.rows {
            if row.run == Some(run_id) {
                row.run = None;
            }
        }
        self.rows.retain(
            |r| !matches!(&r.identity, Identity::Provisional { run_id: rid, .. } if *rid == run_id),
        );
        self.restore_selection(key);
    }

    /// Replace the transcript rows with a fresh scan. Provisional rows
    /// survive (their process still runs) and are adopted by a scanned
    /// transcript only when the match is unambiguous: same agent + cwd,
    /// written after the launch, not already running, outside the
    /// launch snapshot, exactly one candidate, and no sibling
    /// placeholder in the same agent + cwd. With two placeholders or
    /// two new transcripts, mtimes can't say which process wrote which
    /// file, and guessing would bind kill/attach to the wrong terminal.
    pub fn absorb_scan(&mut self, scanned: Vec<SessionMeta>) {
        let mut selected_key = self.selected_key();

        let scanned_ids: HashSet<String> = scanned.iter().map(|m| m.id.clone()).collect();
        let mut provisionals: Vec<Row> = Vec::new();
        let mut prev_runs: HashMap<String, RunId> = HashMap::new();
        // A running row whose transcript momentarily vanished from the
        // scan keeps its place: dropping it would orphan the live PTY
        // with no way to reattach when the transcript reappears.
        let mut stragglers: Vec<Row> = Vec::new();
        for row in self.rows.drain(..) {
            match (&row.identity, row.run) {
                (Identity::Provisional { .. }, _) => provisionals.push(row),
                (Identity::Transcript(id), Some(run)) => {
                    prev_runs.insert(id.clone(), run);
                    if !scanned_ids.contains(id) {
                        stragglers.push(row);
                    }
                }
                _ => {}
            }
        }

        let mut siblings: HashMap<(Agent, String), usize> = HashMap::new();
        for p in &provisionals {
            *siblings.entry((p.agent, p.cwd.clone())).or_default() += 1;
        }
        let mut adopted: HashMap<String, RunId> = HashMap::new();
        provisionals.retain(|p| {
            if siblings[&(p.agent, p.cwd.clone())] > 1 {
                return true;
            }
            let Identity::Provisional { run_id, snapshot } = &p.identity else {
                return true;
            };
            let candidates: Vec<&SessionMeta> = scanned
                .iter()
                .filter(|s| {
                    s.agent == p.agent
                        && s.cwd == p.cwd
                        && s.timestamp >= p.timestamp
                        && !prev_runs.contains_key(&s.id)
                        && !adopted.contains_key(&s.id)
                        && !snapshot.contains(&s.id)
                })
                .collect();
            let [candidate] = candidates[..] else {
                return true;
            };
            adopted.insert(candidate.id.clone(), *run_id);
            if selected_key == Some(Key::Run(*run_id)) {
                selected_key = Some(Key::Id(candidate.id.clone()));
            }
            false
        });

        let mut transcript_rows: Vec<Row> = scanned
            .into_iter()
            .map(|m| {
                let run = adopted.get(&m.id).or_else(|| prev_runs.get(&m.id)).copied();
                Row::from_meta(m, run)
            })
            .collect();
        for straggler in stragglers {
            let pos = transcript_rows
                .iter()
                .position(|r| r.timestamp < straggler.timestamp)
                .unwrap_or(transcript_rows.len());
            transcript_rows.insert(pos, straggler);
        }

        let mut rows = provisionals;
        rows.extend(transcript_rows);
        self.rows = rows;
        self.restore_selection(selected_key);
    }

    fn selected_key(&self) -> Option<Key> {
        self.selected_row().map(key_of)
    }

    fn restore_selection(&mut self, key: Option<Key>) {
        if let Some(key) = key
            && let Some(pos) = self.rows.iter().position(|r| key_of(r) == key)
        {
            self.selected = pos;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn alloc_run_id(&mut self) -> RunId {
        let id = self.next_run_id;
        self.next_run_id += 1;
        id
    }
}
