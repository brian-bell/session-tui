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
    /// time, which necessarily belong to other sessions. Once a scan
    /// observes an ambiguous plausible set, `ambiguous` is set and the
    /// row can never be adopted: the row lives on until its process
    /// exits, but attribution has been proven unknowable.
    Provisional { run_id: RunId, snapshot: HashSet<String>, ambiguous: bool },
    Transcript(String),
}

/// A live PTY attached to a transcript row, plus the fork-adoption
/// state a resume carries until the agent's first write is scanned.
#[derive(Debug, Clone)]
struct Run {
    id: RunId,
    pending_fork: Option<PendingFork>,
}

/// `claude --resume` forks a NEW transcript id; the original file never
/// updates again. Until the fork appears in a scan, the resumed row
/// remembers what it knew at resume time so the fork can be attributed
/// to this run and no other.
#[derive(Debug, Clone)]
struct PendingFork {
    /// Transcript ids that already existed at resume time (including
    /// the resumed one); they necessarily belong to other sessions.
    snapshot: HashSet<String>,
    since: DateTime<Utc>,
    /// A scan observed two plausible forks: attribution is permanently
    /// unknowable, so this wait can never adopt. The process is still
    /// alive and its fork unattributed, though — it keeps blocking
    /// sibling adoption in its agent+cwd until it exits.
    ambiguous: bool,
}

/// One session in the list: scanned metadata (or launch placeholders)
/// plus the live run, if any.
#[derive(Debug, Clone)]
pub struct Row {
    identity: Identity,
    /// Live PTY of a resumed/adopted transcript row; a provisional
    /// row's run lives in its identity.
    run: Option<Run>,
    agent: Agent,
    cwd: String,
    title: String,
    timestamp: DateTime<Utc>,
}

impl Row {
    fn from_meta(meta: SessionMeta, run: Option<Run>) -> Self {
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
            Identity::Transcript(_) => self.run.as_ref().map(|r| r.id),
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

/// The transcripts a waiter could plausibly have written, capped at
/// two: zero means keep waiting, one is a candidate, two is terminal
/// ambiguity — attribution is unknowable the moment it is observed,
/// and later scans add no information.
fn plausible2<'a>(
    scanned: &'a [SessionMeta],
    agent: Agent,
    cwd: &str,
    since: DateTime<Utc>,
    snapshot: &HashSet<String>,
) -> Vec<&'a SessionMeta> {
    scanned
        .iter()
        .filter(|s| {
            s.agent == agent && s.cwd == cwd && s.timestamp >= since && !snapshot.contains(&s.id)
        })
        .take(2)
        .collect()
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
                identity: Identity::Provisional { run_id, snapshot, ambiguous: false },
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
        let agent = row.agent;
        let spec = CommandSpec::resume_id(agent, &id, &row.cwd);
        let run_id = self.alloc_run_id();
        let pending_fork = agent.forks_on_resume().then(|| {
            let snapshot = self
                .rows
                .iter()
                .filter_map(|r| r.transcript_id().map(str::to_string))
                .collect();
            PendingFork { snapshot, since: Utc::now(), ambiguous: false }
        });
        self.rows[self.selected].run = Some(Run { id: run_id, pending_fork });
        Some((run_id, spec))
    }

    /// The process behind `run_id` ended (exit or kill). A provisional
    /// row has no transcript to resume, so it disappears with its
    /// process; a transcript row just loses its run marker. Selection
    /// follows the selected row's identity, not its old index.
    pub fn mark_exited(&mut self, run_id: RunId) {
        let key = self.selected_key();
        for row in &mut self.rows {
            if row.run.as_ref().map(|r| r.id) == Some(run_id) {
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
        // Running transcript rows, by id: their runs carry over to the
        // fresh scan, and a pending fork may hand its run to a newly
        // written transcript below.
        let mut running: HashMap<String, Row> = HashMap::new();
        // A running row whose transcript momentarily vanished from the
        // scan keeps its place: dropping it would orphan the live PTY
        // with no way to reattach when the transcript reappears.
        let mut stragglers: Vec<Row> = Vec::new();
        for row in self.rows.drain(..) {
            match (&row.identity, &row.run) {
                (Identity::Provisional { .. }, _) => provisionals.push(row),
                (Identity::Transcript(id), Some(_)) => {
                    if scanned_ids.contains(id) {
                        running.insert(id.clone(), row);
                    } else {
                        stragglers.push(row);
                    }
                }
                _ => {}
            }
        }

        // Terminal ambiguity is recorded FIRST — before the in-place
        // clearing below and before adoption, so the outcome never
        // depends on which scan delivered which half of the evidence.
        // A waiter that observes two plausible transcripts can never
        // adopt — not in this scan, not later; mtimes will never say
        // which file its process wrote. It does NOT stop blocking,
        // though. Its process is alive and its transcript unattributed,
        // so any new file in its agent+cwd could still be its late
        // write; a sibling adopting one would risk a wrong binding. The
        // block dies with the process (the row drops or loses its run),
        // which is the safe degraded mode everywhere in this module.
        for p in &mut provisionals {
            let (agent, cwd, since) = (p.agent, p.cwd.clone(), p.timestamp);
            if let Identity::Provisional { snapshot, ambiguous, .. } = &mut p.identity
                && !*ambiguous
                && plausible2(&scanned, agent, &cwd, since, snapshot).len() > 1
            {
                *ambiguous = true;
            }
        }
        for row in running.values_mut().chain(stragglers.iter_mut()) {
            let (agent, cwd) = (row.agent, row.cwd.clone());
            if let Some(run) = &mut row.run
                && let Some(pending) = &mut run.pending_fork
                && !pending.ambiguous
                && plausible2(&scanned, agent, &cwd, pending.since, &pending.snapshot).len() > 1
            {
                pending.ambiguous = true;
            }
        }

        // An append to the resumed transcript itself proves the agent
        // continues in place rather than forking: nothing to wait for,
        // and staying a waiter would block adoption in this cwd forever.
        // Not once ambiguity is recorded, though (including by this very
        // scan, above) — fork-like candidates plus an in-place append is
        // contradictory evidence, and only process exit ends the block
        // then.
        for row in running.values_mut() {
            let Identity::Transcript(id) = &row.identity else { continue };
            if let Some(run) = &mut row.run
                && let Some(pending) = &run.pending_fork
                && !pending.ambiguous
                && scanned.iter().any(|s| &s.id == id && s.timestamp > pending.since)
            {
                run.pending_fork = None;
            }
        }

        // Every process that could plausibly write a new transcript in
        // an agent+cwd competes for it: provisional launches and
        // pending-fork resumes alike, ambiguous or not. More than one
        // waiter means a new transcript can't be attributed, so neither
        // pass may adopt.
        let mut siblings: HashMap<(Agent, String), usize> = HashMap::new();
        let waiting = |row: &Row| row.run.as_ref().is_some_and(|r| r.pending_fork.is_some());
        for row in provisionals
            .iter()
            .chain(running.values().filter(|r| waiting(r)))
            .chain(stragglers.iter().filter(|r| waiting(r)))
        {
            *siblings.entry((row.agent, row.cwd.clone())).or_default() += 1;
        }
        let mut adopted: HashMap<String, Run> = HashMap::new();
        // The one transcript a waiting process could have written. A
        // member that is already running or claimed doesn't reveal
        // which file THIS process wrote, so it blocks adoption rather
        // than shrinking the set toward a guess. (Ambiguous sets were
        // already retired above and cannot reach here.)
        let sole_candidate = |adopted: &HashMap<String, Run>,
                              agent: Agent,
                              cwd: &str,
                              since: DateTime<Utc>,
                              snapshot: &HashSet<String>| {
            let [candidate] = plausible2(&scanned, agent, cwd, since, snapshot)[..] else {
                return None;
            };
            if running.contains_key(&candidate.id) || adopted.contains_key(&candidate.id) {
                return None;
            }
            Some(candidate.id.clone())
        };
        provisionals.retain(|p| {
            if siblings[&(p.agent, p.cwd.clone())] > 1 {
                return true;
            }
            let Identity::Provisional { run_id, snapshot, ambiguous: false } = &p.identity else {
                return true;
            };
            let Some(candidate) = sole_candidate(&adopted, p.agent, &p.cwd, p.timestamp, snapshot)
            else {
                return true;
            };
            adopted.insert(candidate.clone(), Run { id: *run_id, pending_fork: None });
            if selected_key == Some(Key::Run(*run_id)) {
                selected_key = Some(Key::Id(candidate));
            }
            false
        });

        // Fork adoption: a resumed row whose agent wrote a NEW
        // transcript hands its run to that transcript, under the same
        // unambiguity rules as launch adoption. No candidate is the
        // normal case for agents that append in place — the row simply
        // keeps its run.
        let forks: Vec<(String, String)> = running
            .values()
            .chain(stragglers.iter())
            .filter_map(|row| {
                let pending = row.run.as_ref()?.pending_fork.as_ref()?;
                if pending.ambiguous || siblings[&(row.agent, row.cwd.clone())] > 1 {
                    return None;
                }
                let candidate = sole_candidate(
                    &adopted,
                    row.agent,
                    &row.cwd,
                    pending.since,
                    &pending.snapshot,
                )?;
                Some((row.transcript_id()?.to_string(), candidate))
            })
            .collect();
        for (origin, fork_id) in forks {
            // An origin missing from the scan loses its row entirely:
            // with the run handed over there is nothing left to show.
            let row = running.remove(&origin).or_else(|| {
                let pos = stragglers.iter().position(|r| r.transcript_id() == Some(&*origin))?;
                Some(stragglers.remove(pos))
            });
            let Some(run) = row.and_then(|r| r.run) else { continue };
            adopted.insert(fork_id.clone(), Run { id: run.id, pending_fork: None });
            if selected_key == Some(Key::Id(origin)) {
                selected_key = Some(Key::Id(fork_id));
            }
        }

        let mut transcript_rows: Vec<Row> = scanned
            .into_iter()
            .map(|m| {
                let run = adopted
                    .remove(&m.id)
                    .or_else(|| running.get(&m.id).and_then(|r| r.run.clone()));
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
