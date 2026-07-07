use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn is_focus_toggle(key: &KeyEvent) -> bool {
    // Ctrl+\ arrives as Char('\\') under the kitty protocol but as
    // Char('4') from legacy terminals (crossterm maps byte 0x1C that way).
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('\\') | KeyCode::Char('4'))
}

/// Translate a crossterm key event into the bytes a terminal would
/// send. `app_cursor` is the child's DECCKM mode: unmodified arrows
/// become SS3 (ESC O x) instead of CSI (ESC [ x).
fn encode_key(key: &KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Modified arrows use the xterm CSI 1;<mod> scheme:
    // 2=Shift, 3=Alt, 5=Ctrl (and their sums).
    if let Some(arrow) = match key.code {
        KeyCode::Up => Some(b'A'),
        KeyCode::Down => Some(b'B'),
        KeyCode::Right => Some(b'C'),
        KeyCode::Left => Some(b'D'),
        _ => None,
    } {
        let mut modifier = 1;
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            modifier += 1;
        }
        if key.modifiers.contains(KeyModifiers::ALT) {
            modifier += 2;
        }
        if ctrl {
            modifier += 4;
        }
        if modifier > 1 {
            return Some(format!("\x1b[1;{}{}", modifier, arrow as char).into_bytes());
        }
        if app_cursor {
            return Some(vec![0x1b, b'O', arrow]);
        }
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        // Meta convention: ESC-prefix the unmodified encoding.
        let stripped = KeyEvent::new(key.code, key.modifiers - KeyModifiers::ALT);
        return encode_key(&stripped, app_cursor).map(|mut bytes| {
            bytes.insert(0, 0x1b);
            bytes
        });
    }
    Some(match key.code {
        KeyCode::Char(c) if ctrl => {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() {
                // Ctrl+A..Ctrl+Z map to 0x01..0x1a
                vec![c as u8 - b'a' + 1]
            } else {
                // Non-letter control bytes. Legacy terminals deliver
                // 0x1d..0x1f as Ctrl+5..Ctrl+7 (0x1c/Ctrl+4 is the
                // focus toggle and never reaches here).
                match c {
                    ' ' | '@' => vec![0x00],
                    '[' => vec![0x1b],
                    ']' | '5' => vec![0x1d],
                    '^' | '6' => vec![0x1e],
                    '_' | '7' | '/' => vec![0x1f],
                    '?' => vec![0x7f],
                    _ => return None,
                }
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
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n @ 1..=4) => vec![0x1b, b'O', b'P' + n - 1],
        KeyCode::F(n @ 5..=12) => {
            // xterm numbering skips 16 and 22.
            let code = [15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5];
            format!("\x1b[{code}~").into_bytes()
        }
        _ => return None,
    })
}

use crate::roster::Roster;
use crate::sessions::{Agent, SessionMeta};
use crate::term::CommandSpec;

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

/// New-session dialog: choose an agent and a working directory from
/// recently used project dirs, or type a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    pub agent: Agent,
    pub dirs: Vec<String>,
    pub highlighted: usize,
    pub input: String,
    /// True once the user moved the highlight; an explicit choice of a
    /// filtered match then outranks the typed text.
    navigated: bool,
}

impl PickerState {
    fn new(dirs: Vec<String>) -> Self {
        Self {
            agent: Agent::Claude,
            dirs,
            highlighted: 0,
            input: String::new(),
            navigated: false,
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

    /// The cwd a launch would use right now. A typed path that exists
    /// wins over substring matches of known dirs — unless the user
    /// explicitly navigated the match list.
    pub fn chosen_dir(&self) -> Option<String> {
        if !self.navigated
            && !self.input.is_empty()
            && std::path::Path::new(&self.input).is_dir()
        {
            return Some(self.input.clone());
        }
        let matches = self.matches();
        match matches.get(self.highlighted) {
            Some(d) => Some((*d).to_string()),
            None if !self.input.is_empty() => Some(self.input.clone()),
            None => None,
        }
    }
}

pub struct App {
    pub focus: Focus,
    pub overlay: Overlay,
    /// Lines scrolled back from live output; 0 = following live.
    pub scroll_offset: usize,
    /// One-shot user-facing message (e.g. why an action was refused);
    /// cleared on the next keypress.
    pub notice: Option<String>,
    /// Attached child's DECCKM (application cursor) mode, synced from
    /// the emulator by the main loop; arrows encode as SS3 when set.
    pub app_cursor: bool,
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
            app_cursor: false,
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

    /// Forward pasted text to the attached terminal. `bracketed` is
    /// whether the child enabled bracketed paste (DECSET 2004, tracked
    /// by the emulator); without it the delimiters would be literal
    /// garbage in the child's input.
    pub fn handle_paste(&mut self, text: &str, bracketed: bool) -> Vec<Effect> {
        // A paste while the launch picker is open is a project path.
        if let Overlay::LaunchPicker(ref mut picker) = self.overlay {
            picker.input.extend(text.chars().filter(|c| !c.is_control()));
            picker.highlighted = 0;
            picker.navigated = false;
            return Vec::new();
        }
        let (Focus::Terminal, Some(run_id)) = (self.focus, self.attached) else {
            return Vec::new();
        };
        let bytes = if bracketed {
            let mut b = b"\x1b[200~".to_vec();
            b.extend_from_slice(text.as_bytes());
            b.extend_from_slice(b"\x1b[201~");
            b
        } else {
            text.as_bytes().to_vec()
        };
        self.scroll_offset = 0;
        vec![Effect::WriteTerminal { run_id, bytes }]
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        self.notice = None;
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
                    picker.navigated = true;
                }
                Vec::new()
            }
            KeyCode::Up => {
                picker.highlighted = picker.highlighted.saturating_sub(1);
                picker.navigated = true;
                Vec::new()
            }
            KeyCode::Char(c) => {
                picker.input.push(c);
                picker.highlighted = 0;
                // The old navigation pointed into a now-stale filtered
                // list; the freshly typed path speaks for itself.
                picker.navigated = false;
                Vec::new()
            }
            KeyCode::Backspace => {
                picker.input.pop();
                picker.highlighted = 0;
                picker.navigated = false;
                Vec::new()
            }
            KeyCode::Enter => {
                let (agent, Some(cwd)) = (picker.agent, picker.chosen_dir()) else {
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
        match encode_key(&key, self.app_cursor) {
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
