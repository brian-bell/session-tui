use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn is_focus_toggle(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('\\')
}

/// Translate a crossterm key event into the bytes a terminal would send.
fn encode_key(key: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char(c) if ctrl => {
            // Ctrl+A..Ctrl+Z and friends map to 0x01..0x1a
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                vec![c as u8 - b'a' + 1]
            } else {
                return None;
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Esc => b"\x1b".to_vec(),
        KeyCode::Backspace => b"\x7f".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => return None,
    })
}

use crate::sessions::{Agent, SessionMeta};
use crate::term::CommandSpec;

/// Identifies one live PTY for the lifetime of the app.
pub type RunId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    Terminal,
}

/// Side effects for the main loop to execute; the state machine itself
/// never touches a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Spawn { run_id: RunId, spec: CommandSpec },
    WriteTerminal { run_id: RunId, bytes: Vec<u8> },
    Kill { run_id: RunId },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    None,
    ConfirmQuit,
    ConfirmKill { run_id: RunId },
    LaunchPicker(PickerState),
}

/// New-session dialog: choose an agent and a working directory from
/// recently used project dirs, or type a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    pub agent: Agent,
    pub dirs: Vec<String>,
    pub highlighted: usize,
    pub input: String,
}

impl PickerState {
    fn new(dirs: Vec<String>) -> Self {
        Self {
            agent: Agent::Claude,
            dirs,
            highlighted: 0,
            input: String::new(),
        }
    }

    /// Dirs matching the typed input (all dirs when input is empty).
    pub fn matches(&self) -> Vec<&str> {
        self.dirs
            .iter()
            .map(String::as_str)
            .filter(|d| d.contains(self.input.as_str()))
            .collect()
    }

    /// The cwd a launch would use right now.
    pub fn chosen_dir(&self) -> Option<String> {
        let matches = self.matches();
        match matches.get(self.highlighted) {
            Some(d) => Some((*d).to_string()),
            None if !self.input.is_empty() => Some(self.input.clone()),
            None => None,
        }
    }
}

pub struct App {
    pub sessions: Vec<SessionMeta>,
    pub selected: usize,
    pub focus: Focus,
    pub overlay: Overlay,
    /// Lines scrolled back from live output; 0 = following live.
    pub scroll_offset: usize,
    /// session id -> live run
    running: HashMap<String, RunId>,
    attached: Option<RunId>,
    next_run_id: RunId,
}

impl App {
    pub fn new(sessions: Vec<SessionMeta>) -> Self {
        Self {
            sessions,
            selected: 0,
            focus: Focus::List,
            overlay: Overlay::None,
            scroll_offset: 0,
            running: HashMap::new(),
            attached: None,
            next_run_id: 1,
        }
    }

    pub fn is_running(&self, run_id: RunId) -> bool {
        self.running.values().any(|&r| r == run_id)
    }

    pub fn attached_run(&self) -> Option<RunId> {
        self.attached
    }

    pub fn run_id_for(&self, session_id: &str) -> Option<RunId> {
        self.running.get(session_id).copied()
    }

    pub fn has_running_sessions(&self) -> bool {
        !self.running.is_empty()
    }

    /// The child process behind `run_id` ended (exit or kill).
    pub fn mark_exited(&mut self, run_id: RunId) {
        self.running.retain(|_, &mut r| r != run_id);
        if self.attached == Some(run_id) {
            self.attached = None;
            self.focus = Focus::List;
            self.scroll_offset = 0;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.overlay != Overlay::None {
            return self.handle_overlay_key(key);
        }
        if is_focus_toggle(&key) {
            self.toggle_focus();
            return Vec::new();
        }
        match self.focus {
            Focus::List => self.handle_list_key(key),
            Focus::Terminal => self.handle_terminal_key(key),
        }
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            // Only enter terminal mode when there is a terminal to talk to.
            Focus::List if self.attached.is_some() => Focus::Terminal,
            Focus::List => Focus::List,
            Focus::Terminal => Focus::List,
        };
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if let Overlay::LaunchPicker(_) = self.overlay {
            return self.handle_picker_key(key);
        }
        let confirmed = matches!(key.code, KeyCode::Char('y') | KeyCode::Enter);
        let overlay = std::mem::replace(&mut self.overlay, Overlay::None);
        match overlay {
            Overlay::ConfirmQuit if confirmed => vec![Effect::Quit],
            Overlay::ConfirmKill { run_id } if confirmed => {
                self.mark_exited(run_id);
                vec![Effect::Kill { run_id }]
            }
            _ => Vec::new(), // any other key cancels
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Overlay::LaunchPicker(ref mut picker) = self.overlay else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            KeyCode::Tab => {
                picker.agent = match picker.agent {
                    Agent::Claude => Agent::Codex,
                    Agent::Codex => Agent::Claude,
                };
                Vec::new()
            }
            KeyCode::Down => {
                let count = picker.matches().len();
                if count > 0 {
                    picker.highlighted = (picker.highlighted + 1).min(count - 1);
                }
                Vec::new()
            }
            KeyCode::Up => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
                Vec::new()
            }
            KeyCode::Char(c) => {
                picker.input.push(c);
                picker.highlighted = 0;
                Vec::new()
            }
            KeyCode::Backspace => {
                picker.input.pop();
                picker.highlighted = 0;
                Vec::new()
            }
            KeyCode::Enter => {
                let (agent, Some(cwd)) = (picker.agent, picker.chosen_dir()) else {
                    return Vec::new();
                };
                self.overlay = Overlay::None;
                self.launch_new(agent, &cwd)
            }
            _ => Vec::new(),
        }
    }

    /// Spawn a fresh agent and put a provisional entry on top of the
    /// list; the real transcript will appear via rescans later.
    fn launch_new(&mut self, agent: Agent, cwd: &str) -> Vec<Effect> {
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        let meta = SessionMeta {
            id: format!("live-{run_id}"),
            agent,
            cwd: cwd.to_string(),
            title: "(new session)".to_string(),
            timestamp: chrono::Utc::now(),
        };
        let spec = CommandSpec::launch(agent, cwd);
        self.running.insert(meta.id.clone(), run_id);
        self.sessions.insert(0, meta);
        self.selected = 0;
        self.attached = Some(run_id);
        self.focus = Focus::Terminal;
        vec![Effect::Spawn { run_id, spec }]
    }

    /// Recently used project dirs, newest first, deduped.
    fn known_dirs(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.sessions
            .iter()
            .filter(|m| !m.id.starts_with("live-"))
            .map(|m| m.cwd.clone())
            .filter(|d| seen.insert(d.clone()))
            .collect()
    }

    /// Replace the scanned session list (from a rescan), keeping
    /// provisional live entries on top and following the selected
    /// session by identity rather than position.
    pub fn update_sessions(&mut self, scanned: Vec<SessionMeta>) {
        let selected_id = self.sessions.get(self.selected).map(|m| m.id.clone());
        let mut next: Vec<SessionMeta> = self
            .sessions
            .iter()
            .filter(|m| m.id.starts_with("live-"))
            .cloned()
            .collect();
        next.extend(scanned);
        self.sessions = next;
        if let Some(id) = selected_id
            && let Some(pos) = self.sessions.iter().position(|m| m.id == id) {
                self.selected = pos;
            }
        self.selected = self.selected.min(self.sessions.len().saturating_sub(1));
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Char('q') => {
                if self.running.is_empty() {
                    vec![Effect::Quit]
                } else {
                    self.overlay = Overlay::ConfirmQuit;
                    Vec::new()
                }
            }
            KeyCode::Char('n') => {
                self.overlay = Overlay::LaunchPicker(PickerState::new(self.known_dirs()));
                Vec::new()
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(&run_id) = self
                    .sessions
                    .get(self.selected)
                    .and_then(|m| self.running.get(&m.id))
                {
                    self.overlay = Overlay::ConfirmKill { run_id };
                }
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.sessions.is_empty() {
            return;
        }
        let last = self.sessions.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    /// Lines jumped per PageUp/PageDown while scrolled back.
    const SCROLL_STEP: usize = 20;

    fn handle_terminal_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let Some(run_id) = self.attached else {
            return Vec::new();
        };
        // PageUp/PageDown browse scrollback; anything else snaps back
        // to live output and goes to the PTY.
        match key.code {
            KeyCode::PageUp => {
                self.scroll_offset += Self::SCROLL_STEP;
                return Vec::new();
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(Self::SCROLL_STEP);
                return Vec::new();
            }
            _ => self.scroll_offset = 0,
        }
        match encode_key(&key) {
            Some(bytes) => vec![Effect::WriteTerminal { run_id, bytes }],
            None => Vec::new(),
        }
    }

    /// Resume the selected session (or attach if it's already running).
    fn activate_selected(&mut self) -> Vec<Effect> {
        let Some(meta) = self.sessions.get(self.selected) else {
            return Vec::new();
        };
        if let Some(&run_id) = self.running.get(&meta.id) {
            self.attached = Some(run_id);
            self.focus = Focus::Terminal;
            return Vec::new();
        }
        let run_id = self.next_run_id;
        self.next_run_id += 1;
        let spec = CommandSpec::resume(meta);
        self.running.insert(meta.id.clone(), run_id);
        self.attached = Some(run_id);
        self.focus = Focus::Terminal;
        vec![Effect::Spawn { run_id, spec }]
    }
}
