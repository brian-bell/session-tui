use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::sessions::{Agent, SessionMeta};

/// Lines of scrollback kept per session.
pub const SCROLLBACK_LINES: usize = 10_000;

fn program_for(agent: Agent) -> String {
    match agent {
        Agent::Claude => "claude".into(),
        Agent::Codex => "codex".into(),
    }
}

/// What to run in a new PTY: program, args, working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
}

impl CommandSpec {
    pub fn resume(meta: &SessionMeta) -> Self {
        let args = match meta.agent {
            Agent::Claude => vec!["--resume".into(), meta.id.clone()],
            Agent::Codex => vec!["resume".into(), meta.id.clone()],
        };
        Self {
            program: program_for(meta.agent),
            args,
            cwd: meta.cwd.clone(),
        }
    }

    pub fn launch(agent: Agent, cwd: &str) -> Self {
        Self {
            program: program_for(agent),
            args: Vec::new(),
            cwd: cwd.into(),
        }
    }
}

/// Whether a running agent is actively producing output or waiting.
/// Agent CLIs animate a spinner while working, so "any output within
/// the threshold" is a reliable busy signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Busy,
    Idle,
}

impl SessionStatus {
    const IDLE_THRESHOLD: Duration = Duration::from_secs(2);

    pub fn from_idle(idle: Duration) -> Self {
        if idle < Self::IDLE_THRESHOLD {
            Self::Busy
        } else {
            Self::Idle
        }
    }
}

/// A live PTY running a child process, mirrored into a vt100 emulator.
/// A background thread drains PTY output into the parser; rendering and
/// input happen on the caller's thread.
pub struct PtySession {
    parser: Arc<Mutex<vt100::Parser>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    last_output: Arc<Mutex<Instant>>,
}

impl PtySession {
    pub fn spawn(spec: &CommandSpec, rows: u16, cols: u16) -> Result<Self> {
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        let child = pair.slave.spawn_command(cmd).context("spawn in pty")?;
        drop(pair.slave);

        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, SCROLLBACK_LINES)));
        let last_output = Arc::new(Mutex::new(Instant::now()));

        let mut reader = pair.master.try_clone_reader().context("clone pty reader")?;
        let writer = pair.master.take_writer().context("take pty writer")?;
        {
            let parser = Arc::clone(&parser);
            let last_output = Arc::clone(&last_output);
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 {
                        break; // EOF: child exited
                    }
                    parser.lock().unwrap().process(&buf[..n]);
                    *last_output.lock().unwrap() = Instant::now();
                }
            });
        }

        Ok(Self {
            parser,
            writer,
            master: pair.master,
            child,
            last_output,
        })
    }

    /// Plain-text contents of the visible screen (testing/preview aid).
    pub fn screen_text(&self) -> String {
        String::from_utf8_lossy(&self.parser.lock().unwrap().screen().contents_formatted())
            .into_owned()
    }

    /// Run `f` with the emulated screen, for rendering.
    pub fn with_screen<T>(&self, f: impl FnOnce(&vt100::Screen) -> T) -> T {
        f(self.parser.lock().unwrap().screen())
    }

    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser.lock().unwrap().screen_mut().set_size(rows, cols);
        Ok(())
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) -> Result<()> {
        self.child.kill().context("kill child")?;
        // Reap immediately: dropping the handle without waiting leaves
        // a zombie until the app exits (portable-pty wraps
        // std::process::Child on Unix, whose Drop does not wait).
        let _ = self.child.wait();
        Ok(())
    }

    pub fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }
}

/// Losing the handle must not orphan a running agent: error/panic exit
/// paths drop sessions without going through an explicit Quit. The
/// transcript on disk keeps the session resumable.
impl Drop for PtySession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl PtySession {
    /// How long since the child last produced output.
    pub fn idle_for(&self) -> Duration {
        self.last_output.lock().unwrap().elapsed()
    }

    pub fn status(&self) -> SessionStatus {
        SessionStatus::from_idle(self.idle_for())
    }

    /// Scroll the view `rows` back from live output (0 = live).
    pub fn set_scrollback(&self, rows: usize) {
        self.parser.lock().unwrap().screen_mut().set_scrollback(rows);
    }

    /// Whether the child enabled bracketed paste (DECSET 2004).
    pub fn bracketed_paste(&self) -> bool {
        self.parser.lock().unwrap().screen().bracketed_paste()
    }

    /// Whether the child enabled application cursor mode (DECCKM).
    pub fn application_cursor(&self) -> bool {
        self.parser.lock().unwrap().screen().application_cursor()
    }
}
