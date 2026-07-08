use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::input;
use crate::roster::Roster;
pub use crate::picker::PickerState;
use crate::sessions::{Agent, SessionMeta};
use crate::term::{CommandSpec, TermModes};

pub use crate::roster::RunId;

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

pub struct App {
    pub focus: Focus,
    pub overlay: Overlay,
    /// Lines scrolled back from live output; 0 = following live.
    pub scroll_offset: usize,
    /// One-shot user-facing message (e.g. why an action was refused);
    /// cleared on the next keypress.
    pub notice: Option<String>,
    roster: Roster,
    attached: Option<RunId>,
}

impl App {
    pub fn new(sessions: Vec<SessionMeta>) -> Self {
        Self {
            focus: Focus::List,
            overlay: Overlay::None,
            scroll_offset: 0,
            notice: None,
            roster: Roster::new(sessions),
            attached: None,
        }
    }

    pub fn roster(&self) -> &Roster {
        &self.roster
    }

    pub fn is_running(&self, run_id: RunId) -> bool {
        self.roster.is_running(run_id)
    }

    pub fn attached_run(&self) -> Option<RunId> {
        self.attached
    }

    pub fn run_id_for(&self, session_id: &str) -> Option<RunId> {
        self.roster
            .rows()
            .iter()
            .find(|r| r.transcript_id() == Some(session_id))
            .and_then(|r| r.run_id())
    }

    /// Show `run_id` in the terminal pane, always starting at live
    /// output rather than whatever scrollback the previous session had.
    fn attach(&mut self, run_id: RunId) {
        self.attached = Some(run_id);
        self.focus = Focus::Terminal;
        self.scroll_offset = 0;
    }

    pub fn has_running_sessions(&self) -> bool {
        self.roster.has_running()
    }

    /// The child process behind `run_id` ended (exit or kill).
    pub fn mark_exited(&mut self, run_id: RunId) {
        self.roster.mark_exited(run_id);
        if self.attached == Some(run_id) {
            self.attached = None;
            self.focus = Focus::List;
            self.scroll_offset = 0;
        }
    }

    /// Forward pasted text to the attached terminal, encoded for the
    /// child's modes (bracketed paste only when it enabled DECSET 2004).
    pub fn handle_paste(&mut self, text: &str, modes: TermModes) -> Vec<Effect> {
        // A paste while the launch picker is open is a project path.
        if let Overlay::LaunchPicker(ref mut picker) = self.overlay {
            picker.paste(text);
            return Vec::new();
        }
        let (Focus::Terminal, Some(run_id)) = (self.focus, self.attached) else {
            return Vec::new();
        };
        self.scroll_offset = 0;
        vec![Effect::WriteTerminal { run_id, bytes: input::encode_paste(text, modes) }]
    }

    pub fn handle_key(&mut self, key: KeyEvent, modes: TermModes) -> Vec<Effect> {
        self.notice = None;
        if self.overlay != Overlay::None {
            return self.handle_overlay_key(key);
        }
        if input::is_focus_toggle(&key) {
            self.toggle_focus();
            return Vec::new();
        }
        match self.focus {
            Focus::List => self.handle_list_key(key),
            Focus::Terminal => self.handle_terminal_key(key, modes),
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
        // The prompts read "y/N": only an explicit y confirms; Enter
        // takes the safe default and cancels.
        let confirmed = matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y'));
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
                picker.toggle_agent();
                Vec::new()
            }
            KeyCode::Down => {
                picker.move_highlight(1);
                Vec::new()
            }
            KeyCode::Up => {
                picker.move_highlight(-1);
                Vec::new()
            }
            KeyCode::Char(c) => {
                picker.edit(c);
                Vec::new()
            }
            KeyCode::Backspace => {
                picker.backspace();
                Vec::new()
            }
            KeyCode::Enter => {
                let (agent, Some(cwd)) = (picker.agent(), picker.chosen_dir()) else {
                    return Vec::new();
                };
                // Session history often points at deleted temp/worktree
                // dirs; spawning there would silently fall back to $HOME.
                // Canonicalize so the provisional row's cwd matches the
                // resolved cwd the agent will record in its transcript
                // (relative paths, `.` segments, /tmp symlinks).
                let cwd = match std::fs::canonicalize(&cwd) {
                    Ok(c) => c.to_string_lossy().into_owned(),
                    Err(_) => {
                        self.notice = Some(format!("directory no longer exists: {cwd}"));
                        return Vec::new();
                    }
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
        let (run_id, spec) = self.roster.launch(agent, cwd);
        self.attach(run_id);
        vec![Effect::Spawn { run_id, spec }]
    }

    /// Replace the scanned session list (from a rescan); the roster
    /// keeps provisional rows alive and adopts their transcripts.
    pub fn update_sessions(&mut self, scanned: Vec<SessionMeta>) {
        self.roster.absorb_scan(scanned);
    }

    fn handle_list_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Enter => self.activate_selected(),
            KeyCode::Char('q') => {
                if self.roster.has_running() {
                    self.overlay = Overlay::ConfirmQuit;
                    Vec::new()
                } else {
                    vec![Effect::Quit]
                }
            }
            KeyCode::Char('n') => {
                self.overlay = Overlay::LaunchPicker(PickerState::new(self.roster.known_dirs()));
                Vec::new()
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(run_id) = self.roster.selected_row().and_then(|r| r.run_id()) {
                    self.overlay = Overlay::ConfirmKill { run_id };
                }
                Vec::new()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.roster.move_selection(1);
                Vec::new()
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.roster.move_selection(-1);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Lines jumped per PageUp/PageDown while scrolled back.
    const SCROLL_STEP: usize = 20;

    fn handle_terminal_key(&mut self, key: KeyEvent, modes: TermModes) -> Vec<Effect> {
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
        match input::encode_key(&key, modes) {
            Some(bytes) => vec![Effect::WriteTerminal { run_id, bytes }],
            None => Vec::new(),
        }
    }

    /// Resume the selected session (or attach if it's already running).
    fn activate_selected(&mut self) -> Vec<Effect> {
        let Some(row) = self.roster.selected_row() else {
            return Vec::new();
        };
        if let Some(run_id) = row.run_id() {
            self.attach(run_id);
            return Vec::new();
        }
        // Same trap as the launch picker: portable-pty falls back to
        // $HOME for a missing cwd and the agent would resume against
        // the wrong tree. The roster never touches the filesystem, so
        // the check lives here.
        if !std::path::Path::new(row.cwd()).is_dir() {
            let cwd = row.cwd().to_string();
            self.notice = Some(format!("directory no longer exists: {cwd}"));
            return Vec::new();
        }
        let Some((run_id, spec)) = self.roster.resume_selected() else {
            return Vec::new();
        };
        self.attach(run_id);
        vec![Effect::Spawn { run_id, spec }]
    }
}
