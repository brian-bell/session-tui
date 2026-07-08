//! The new-session dialog: choose an agent and a working directory
//! from recently used project dirs, or type a path. Every mutation
//! enforces the picker's one invariant internally: editing the input
//! resets the highlight and cancels any explicit navigation, because
//! the old navigation pointed into a now-stale filtered list.

use crate::sessions::Agent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerState {
    agent: Agent,
    dirs: Vec<String>,
    highlighted: usize,
    input: String,
    /// True once the user moved the highlight; an explicit choice of a
    /// filtered match then outranks the typed text.
    navigated: bool,
}

impl PickerState {
    pub fn new(dirs: Vec<String>) -> Self {
        Self {
            agent: Agent::Claude,
            dirs,
            highlighted: 0,
            input: String::new(),
            navigated: false,
        }
    }

    pub fn agent(&self) -> Agent {
        self.agent
    }

    pub fn input(&self) -> &str {
        &self.input
    }

    pub fn highlighted(&self) -> usize {
        self.highlighted
    }

    pub fn toggle_agent(&mut self) {
        self.agent = match self.agent {
            Agent::Claude => Agent::Codex,
            Agent::Codex => Agent::Claude,
        };
    }

    pub fn edit(&mut self, c: char) {
        self.input.push(c);
        self.reset_navigation();
    }

    pub fn backspace(&mut self) {
        self.input.pop();
        self.reset_navigation();
    }

    /// Pasted text is a project path; control characters (including
    /// newlines) never belong in one.
    pub fn paste(&mut self, text: &str) {
        self.input.extend(text.chars().filter(|c| !c.is_control()));
        self.reset_navigation();
    }

    /// Move the highlight through the filtered matches, clamped. A
    /// move marks explicit navigation; with no matches there is
    /// nothing to choose, so nothing changes.
    pub fn move_highlight(&mut self, delta: isize) {
        let count = self.matches().len();
        if count == 0 {
            return;
        }
        self.highlighted = self.highlighted.saturating_add_signed(delta).min(count - 1);
        self.navigated = true;
    }

    fn reset_navigation(&mut self) {
        self.highlighted = 0;
        self.navigated = false;
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
