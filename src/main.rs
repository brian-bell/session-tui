use std::collections::HashMap;
use std::io;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event as CtEvent, KeyEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use notify::{RecursiveMode, Watcher};
use ratatui::layout::Rect;
use ratatui::prelude::CrosstermBackend;
use ratatui::Terminal;

use session_tui::app::{App, Effect, RunId};
use session_tui::sessions::{scan_all_sessions, ScanRoots, SessionMeta};
use session_tui::term::PtySession;
use session_tui::ui;

enum Event {
    Input(CtEvent),
    Scanned(Vec<SessionMeta>),
}

/// Restores the user's terminal on every exit path — normal return,
/// error, or panic unwind. Raw mode in a dropped-back-to shell is worse
/// than any error we might be exiting with.
struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        if let Err(err) =
            crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
        {
            let _ = disable_raw_mode();
            return Err(err.into());
        }
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn main() -> Result<()> {
    let (tx, rx) = mpsc::channel::<Event>();
    spawn_input_thread(tx.clone());
    spawn_scanner_thread(tx);

    let _guard = TerminalGuard::new()?;
    run(rx)
}

fn run(rx: mpsc::Receiver<Event>) -> Result<()> {
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut app = App::new(Vec::new());
    let mut ptys: HashMap<RunId, PtySession> = HashMap::new();

    loop {
        // Reap children that exited so the UI stops showing them live.
        let exited: Vec<RunId> = ptys
            .iter_mut()
            .filter_map(|(&id, p)| (!p.is_running()).then_some(id))
            .collect();
        for run_id in exited {
            ptys.remove(&run_id);
            app.mark_exited(run_id);
        }

        if let Some(pty) = app.attached_run().and_then(|id| ptys.get(&id)) {
            pty.set_scrollback(app.scroll_offset);
        }
        let statuses: HashMap<RunId, _> =
            ptys.iter().map(|(&id, p)| (id, p.status())).collect();
        terminal.draw(|f| match app.attached_run().and_then(|id| ptys.get(&id)) {
            Some(pty) => pty.with_screen(|screen| ui::render(f, &app, Some(screen), &statuses)),
            None => ui::render(f, &app, None, &statuses),
        })?;

        // Coalesce: block briefly for the next event, then drain the queue.
        let Ok(first) = rx.recv_timeout(Duration::from_millis(50)) else {
            continue;
        };
        for event in std::iter::once(first).chain(rx.try_iter()) {
            let effects = match event {
                Event::Input(CtEvent::Key(k)) if k.kind != KeyEventKind::Release => {
                    app.handle_key(k)
                }
                Event::Input(CtEvent::Paste(text)) => app.handle_paste(&text),
                Event::Input(CtEvent::Resize(w, h)) => {
                    let (rows, cols) = ui::terminal_pane_size(Rect::new(0, 0, w, h));
                    for pty in ptys.values_mut() {
                        let _ = pty.resize(rows.max(2), cols.max(2));
                    }
                    Vec::new()
                }
                Event::Input(_) => Vec::new(),
                Event::Scanned(sessions) => {
                    app.update_sessions(sessions);
                    Vec::new()
                }
            };
            for effect in effects {
                if !execute(effect, &mut app, &mut ptys, &terminal)? {
                    return Ok(());
                }
            }
        }
    }
}

/// Apply one effect; returns false when the app should exit.
fn execute(
    effect: Effect,
    app: &mut App,
    ptys: &mut HashMap<RunId, PtySession>,
    terminal: &Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<bool> {
    match effect {
        Effect::Spawn { run_id, spec } => {
            let size = terminal.size()?;
            let (rows, cols) =
                ui::terminal_pane_size(Rect::new(0, 0, size.width, size.height));
            match PtySession::spawn(&spec, rows.max(2), cols.max(2)) {
                Ok(pty) => {
                    ptys.insert(run_id, pty);
                }
                Err(err) => {
                    app.mark_exited(run_id);
                    app.notice = Some(format!("failed to start {}: {err:#}", spec.program));
                }
            }
        }
        Effect::WriteTerminal { run_id, bytes } => {
            if let Some(pty) = ptys.get_mut(&run_id) {
                let _ = pty.write_input(&bytes);
            }
        }
        Effect::Kill { run_id } => {
            if let Some(mut pty) = ptys.remove(&run_id) {
                let _ = pty.kill();
            }
        }
        Effect::Quit => {
            for (_, mut pty) in ptys.drain() {
                let _ = pty.kill();
            }
            return Ok(false);
        }
    }
    Ok(true)
}

fn spawn_input_thread(tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if tx.send(Event::Input(event)).is_err() {
                break;
            }
        }
    });
}

/// Initial scan plus debounced rescans whenever either session store
/// changes on disk.
fn spawn_scanner_thread(tx: mpsc::Sender<Event>) {
    std::thread::spawn(move || {
        let roots = ScanRoots::default();
        let rescan = |tx: &mpsc::Sender<Event>| {
            if let Ok(sessions) = scan_all_sessions(&roots) {
                let _ = tx.send(Event::Scanned(sessions));
            }
        };
        rescan(&tx);

        let (fs_tx, fs_rx) = mpsc::channel();
        let Ok(mut watcher) = notify::recommended_watcher(move |res: notify::Result<_>| {
            if res.is_ok() {
                let _ = fs_tx.send(());
            }
        }) else {
            return;
        };
        for root in [&roots.claude, &roots.codex] {
            // A store that doesn't exist yet (fresh install, or one of
            // the two CLIs never used) can't be watched; create it so
            // sessions launched later still show up without a restart.
            let _ = std::fs::create_dir_all(root);
            let _ = watcher.watch(root, RecursiveMode::Recursive);
        }
        // Debounce: after a burst of fs events, wait for 500ms of quiet.
        while fs_rx.recv().is_ok() {
            while fs_rx.recv_timeout(Duration::from_millis(500)).is_ok() {}
            rescan(&tx);
        }
    });
}
